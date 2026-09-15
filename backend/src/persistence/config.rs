use super::StoreError;
use serde::Deserialize;
use sqlx::postgres::{PgConnectOptions, PgSslMode};
use std::path::PathBuf;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseConfig {
    pub host: String,
    pub port: u16,
    pub name: String,
    pub user: String,
    pub password_file: PathBuf,
    #[serde(default = "one")]
    pub min_connections: u32,
    #[serde(default = "eight")]
    pub max_connections: u32,
    #[serde(default = "acquire")]
    pub acquire_timeout_ms: u64,
    #[serde(default = "lock")]
    pub lock_timeout_ms: u64,
    #[serde(default = "statement")]
    pub statement_timeout_ms: u64,
    #[serde(default = "idle")]
    pub idle_transaction_timeout_ms: u64,
    /// Numeric pool-acquisition timings only; leave off outside bounded load runs.
    #[serde(default)]
    pub log_pool_acquire: bool,
}
fn one() -> u32 {
    1
}
fn eight() -> u32 {
    8
}
fn acquire() -> u64 {
    3000
}
fn lock() -> u64 {
    2000
}
fn statement() -> u64 {
    5000
}
fn idle() -> u64 {
    10000
}
impl DatabaseConfig {
    pub fn validate(&self) -> Result<(), StoreError> {
        // Current architecture is a local database. Never silently use remote plaintext.
        if self.host != "127.0.0.1"
            || self.port == 0
            || self.name.is_empty()
            || self.user.is_empty()
            || self.max_connections == 0
            || self.max_connections > 32
            || self.min_connections > self.max_connections
            || [
                self.acquire_timeout_ms,
                self.lock_timeout_ms,
                self.statement_timeout_ms,
                self.idle_transaction_timeout_ms,
            ]
            .iter()
            .any(|n| *n == 0 || *n > 300_000)
        {
            return Err(StoreError::InvalidInput);
        }
        Ok(())
    }
    pub fn options(&self) -> Result<PgConnectOptions, StoreError> {
        let password =
            std::fs::read_to_string(&self.password_file).map_err(|_| StoreError::InvalidInput)?;
        if password.trim().is_empty() {
            return Err(StoreError::InvalidInput);
        }
        Ok(PgConnectOptions::new()
            .host(&self.host)
            .port(self.port)
            .database(&self.name)
            .username(&self.user)
            .password(password.trim())
            .ssl_mode(PgSslMode::Disable)
            .application_name("patchwork"))
    }
}
