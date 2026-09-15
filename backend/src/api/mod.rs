pub mod auth;
pub mod error;
pub mod ws;

use crate::{
    AppState,
    game::{Probe, RoomsReady},
};
use actix_cors::Cors;
use actix_web::{HttpResponse, http::Method, web};
use serde::Serialize;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(web::scope("/api/auth").configure(auth::configure))
        .app_data(web::PayloadConfig::new(4096))
        .route("/api/me", web::get().to(auth::me))
        .route("/api/ws", web::get().to(ws::connect))
        .route("/healthz", web::get().to(health))
        .route("/readyz", web::get().to(ready));
}
pub fn cors(origins: &[String]) -> Cors {
    let mut cors = Cors::default()
        .allowed_methods([Method::GET, Method::POST, Method::PUT, Method::OPTIONS])
        .allowed_headers(["Authorization", "Content-Type"])
        .max_age(600);
    for origin in origins {
        cors = cors.allowed_origin(origin);
    }
    cors
}
pub fn json_config() -> web::JsonConfig {
    web::JsonConfig::default()
        .limit(4096)
        .error_handler(|_, _| error::ApiError::bad_request("无效的 JSON 请求").into())
}
async fn health() -> HttpResponse {
    HttpResponse::Ok().json(serde_json::json!({"status": "ok"}))
}
#[derive(Serialize)]
struct Readiness {
    status: &'static str,
    lobby: &'static str,
    database: &'static str,
    rooms: &'static str,
}
async fn ready(state: web::Data<AppState>) -> HttpResponse {
    let reachable = matches!(
        tokio::time::timeout(
            std::time::Duration::from_millis(250),
            state.lobby.send(Probe)
        )
        .await,
        Ok(Ok(()))
    );
    let database = match &state.database {
        None => "not_configured",
        Some(db) if db.healthy().await => "ok",
        Some(_) => "unavailable",
    };
    let rooms_ready = matches!(
        tokio::time::timeout(
            std::time::Duration::from_millis(250),
            state.lobby.send(RoomsReady)
        )
        .await,
        Ok(Ok(true))
    );
    let ready = reachable && database == "ok" && rooms_ready;
    let mut response = if ready {
        HttpResponse::Ok()
    } else {
        HttpResponse::ServiceUnavailable()
    };
    response.json(Readiness {
        status: if ready { "ready" } else { "not_ready" },
        lobby: if reachable { "ok" } else { "unavailable" },
        database,
        rooms: if rooms_ready { "ok" } else { "recovering" },
    })
}
