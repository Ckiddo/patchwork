use actix::Actor;
use actix_web::{App, HttpServer, dev::Service, middleware::DefaultHeaders, web};
use backend::{AppState, api, audit, config::Config, game::LobbyManager, instance::InstanceGuard};
use std::{io, net::TcpListener};

#[actix_web::main]
async fn main() -> io::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let config_path = match (args.next(), args.next(), args.next()) {
        (Some(flag), Some(path), None) if flag == "--config" => path,
        _ => {
            eprintln!("usage: patchwork-server --config <path-to-config.toml>");
            std::process::exit(2);
        }
    };
    let config = Config::load(std::path::Path::new(&config_path))
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    audit::init(&config.log_level);
    let _instance = InstanceGuard::acquire()?;
    let secret = config.load_jwt_secret()?;
    let lobby = LobbyManager::default().start();
    let mut state = AppState::new(&secret, lobby)
        .with_auth(config.auth.clone(), config.allowed_origins.clone())
        .with_recovery(config.recovery.clone());
    let database = if let Some(db_config) = &config.database {
        let db = backend::persistence::Database::connect(db_config)
            .await
            .map_err(io::Error::other)?;
        db.check_runtime_role().await.map_err(io::Error::other)?;
        db.check_schema().await.map_err(io::Error::other)?;
        state = state.with_database(db.clone());
        Some(db)
    } else {
        None
    };
    let state = web::Data::new(state);
    let listener = TcpListener::bind(config.listen)?;
    tracing::info!(target: "patchwork_audit", event = "listening", address = %listener.local_addr()?, database_configured = database.is_some());
    let origins = config.allowed_origins.clone();
    let mut server = HttpServer::new(move || {
        App::new()
            .app_data(state.clone())
            .app_data(api::json_config())
            .wrap(DefaultHeaders::new().add(("Cache-Control", "no-store")))
            .wrap(api::cors(&origins))
            .wrap_fn(|req, srv| {
                let future = srv.call(req);
                async move {
                    let response = future.await?;
                    audit::http_response(response.status().as_u16());
                    Ok(response)
                }
            })
            .configure(api::configure)
    })
    .workers(config.workers)
    .shutdown_timeout(config.shutdown_timeout_secs)
    .disable_signals()
    .listen(listener)?
    .run();
    let handle = server.handle();
    let deadline = std::time::Duration::from_secs(config.shutdown_timeout_secs);
    tokio::select! {
        result = &mut server => result?,
        signal = backend::shutdown::wait(config.shutdown_signal_file) => {
            if signal.is_err() {
                tracing::error!(target: "patchwork_audit", event = "shutdown_signal_unavailable");
            }
            tracing::info!(target: "patchwork_audit", event = "draining");
            // Upgraded sockets can outlive Actix's HTTP worker drain. Bound the
            // whole operation, not only the worker's individual request timeout.
            match tokio::time::timeout(deadline, async {
                handle.stop(true).await;
                (&mut server).await
            }).await {
                Ok(result) => result?,
                Err(_) => {
                    tracing::warn!(target: "patchwork_audit", event = "http_drain_deadline");
                    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle.stop(false)).await;
                }
            }
        }
    }
    if let Some(db) = database {
        // Worker-runtime teardown can abandon asynchronous pool-return work.
        // Closing sockets on process exit releases any remaining DB transaction.
        if tokio::time::timeout(deadline, db.close()).await.is_err() {
            tracing::warn!(target: "patchwork_audit", event = "database_close_deadline");
        }
    }
    audit::flush_pool_timings();
    tracing::info!(target: "patchwork_audit", event = "stopped");
    Ok(())
}
