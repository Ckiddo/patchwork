//! Explicit migration command. Runtime servers never execute DDL.
use backend::{
    config::Config,
    persistence::{Database, MIGRATOR},
};
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let path = match (args.next(), args.next(), args.next()) {
        (Some(flag), Some(path), None) if flag == "--config" => path,
        _ => return Err("usage: patchwork-migrate --config <migration-config.toml>".into()),
    };
    let config = Config::load(std::path::Path::new(&path))?;
    let db = Database::connect(
        config
            .database
            .as_ref()
            .ok_or("database configuration required")?,
    )
    .await?;
    // Role membership and schema ownership are provisioned separately by the DBA.
    let mut conn = db
        .pool()
        .acquire()
        .await
        .map_err(|_| "migration connection unavailable")?;
    sqlx::query("SET ROLE patchwork_owner")
        .execute(&mut *conn)
        .await
        .map_err(|_| "migration role required")?;
    MIGRATOR
        .run(&mut *conn)
        .await
        .map_err(|_| "migration failed; inspect schema with migration account")?;
    drop(conn);
    db.check_schema().await?;
    db.close().await;
    println!("schema migrations verified");
    Ok(())
}
