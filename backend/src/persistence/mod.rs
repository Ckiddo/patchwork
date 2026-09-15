//! Database boundary: no Actor addresses, HTTP responses or broadcasts.
pub mod config;
pub mod friends;
pub mod gameplay;
pub mod identity;
pub mod recovery;
pub mod repository;
pub mod transaction;

use config::DatabaseConfig;
use sqlx::{ConnectOptions, PgPool, Row, postgres::PgPoolOptions};
use std::time::Duration;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Stable errors only. PostgreSQL error details can contain private row values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreError {
    InvalidInput,
    Unavailable,
    SchemaMismatch,
    Permission,
    Conflict,
    NotFound,
    VersionConflict,
    RequestIdConflict,
    CommitUnknown,
    RoomFull,
    RoomNotJoinable,
    NotEnoughPlayers,
    NotReady,
    PlayerBusy,
    BadPassword,
    RulesNotImplemented,
    SyncRequired,
    GameNotRunning,
    NotYourTurn,
    InvalidPlacement,
    InsufficientButtons,
    WrongActionPhase,
}
impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "database operation failed: {self:?}")
    }
}
impl std::error::Error for StoreError {}

#[derive(Clone)]
pub struct Database {
    pool: PgPool,
}
impl Database {
    pub async fn connect(config: &DatabaseConfig) -> Result<Self, StoreError> {
        config.validate()?;
        let options = config.options()?.disable_statement_logging();
        let (lock, statement, idle) = (
            config.lock_timeout_ms,
            config.statement_timeout_ms,
            config.idle_transaction_timeout_ms,
        );
        let pool = PgPoolOptions::new()
            .min_connections(config.min_connections).max_connections(config.max_connections)
            .acquire_time_level(if config.log_pool_acquire { "info" } else { "off" }.parse().expect("constant log level"))
            .acquire_timeout(Duration::from_millis(config.acquire_timeout_ms))
            .after_connect(move |conn, _| Box::pin(async move {
                sqlx::query("SELECT set_config('search_path','patchwork,pg_catalog',false), set_config('timezone','UTC',false), set_config('lock_timeout',$1,false), set_config('statement_timeout',$2,false), set_config('idle_in_transaction_session_timeout',$3,false), set_config('synchronous_commit','on',false)")
                    .bind(format!("{lock}ms")).bind(format!("{statement}ms")).bind(format!("{idle}ms"))
                    .execute(conn).await?;
                Ok(())
            }))
            .connect_with(options).await.map_err(|_| StoreError::Unavailable)?;
        Ok(Self { pool })
    }
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
    pub async fn close(&self) {
        self.pool.close().await;
    }
    pub async fn check_schema(&self) -> Result<(), StoreError> {
        let rows = sqlx::query(
            "SELECT version,success,checksum FROM patchwork._sqlx_migrations ORDER BY version",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|_| StoreError::SchemaMismatch)?;
        let expected: Vec<_> = MIGRATOR.iter().collect();
        if rows.len() != expected.len() {
            return Err(StoreError::SchemaMismatch);
        }
        for (row, migration) in rows.iter().zip(expected) {
            if row.get::<i64, _>("version") != migration.version
                || !row.get::<bool, _>("success")
                || row.get::<Vec<u8>, _>("checksum").as_slice() != migration.checksum.as_ref()
            {
                return Err(StoreError::SchemaMismatch);
            }
        }
        Ok(())
    }
    pub async fn check_runtime_role(&self) -> Result<(), StoreError> {
        let unsafe_role: bool = sqlx::query_scalar("SELECT rolsuper OR rolcreatedb OR rolcreaterole OR has_schema_privilege(current_user,'patchwork','CREATE') FROM pg_roles WHERE rolname=current_user")
            .fetch_one(&self.pool).await.map_err(|_| StoreError::Unavailable)?;
        if unsafe_role {
            return Err(StoreError::Permission);
        }
        Ok(())
    }
    /// A rolled-back DML probe detects read-only databases as well as connectivity.
    pub async fn healthy(&self) -> bool {
        let probe = async {
            self.check_schema().await?;
            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|_| StoreError::Unavailable)?;
            let ok =
                sqlx::query("UPDATE patchwork.users SET auth_version=auth_version WHERE false")
                    .execute(&mut *tx)
                    .await
                    .is_ok();
            let rolled_back = tx.rollback().await.is_ok();
            if ok && rolled_back {
                Ok(())
            } else {
                Err(StoreError::Unavailable)
            }
        };
        matches!(
            tokio::time::timeout(Duration::from_secs(1), probe).await,
            Ok(Ok(()))
        )
    }
}
