use serde::Deserialize;
use std::{
    fs, io,
    net::SocketAddr,
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub recovery: crate::recovery::RecoveryConfig,
    #[serde(default)]
    pub auth: crate::identity::AuthConfig,
    pub database: Option<crate::persistence::config::DatabaseConfig>,
    pub listen: SocketAddr,
    pub jwt_secret_file: PathBuf,
    pub allowed_origins: Vec<String>,
    pub workers: usize,
    pub shutdown_timeout_secs: u64,
    /// Optional administrator-controlled stop signal for a Windows scheduled task.
    pub shutdown_signal_file: Option<PathBuf>,
    pub log_level: LogLevel,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    Warn,
    Info,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, &'static str> {
        let content = fs::read_to_string(path).map_err(|_| "cannot read config file")?;
        // TOML errors may echo the source line: never return their Display.
        let mut config: Self = toml::from_str(&content).map_err(|_| "invalid config file")?;
        if config.workers == 0
            || !config.recovery.valid()
            || !(1..=30).contains(&config.auth.websocket_auth_timeout_secs)
            || config.workers > 64
            || config.shutdown_timeout_secs == 0
            || config.shutdown_timeout_secs > 300
            || config.allowed_origins.is_empty()
        {
            return Err("invalid server limits");
        }
        for origin in &config.allowed_origins {
            let uri = origin
                .parse::<actix_web::http::Uri>()
                .map_err(|_| "invalid allowed origin")?;
            if !matches!(uri.scheme_str(), Some("http" | "https"))
                || uri.authority().is_none()
                || uri.authority().is_some_and(|a| a.as_str().contains('@'))
                || uri.path() != "/"
                || uri.query().is_some()
                || origin.ends_with('/')
            {
                return Err("allowed origin must contain only scheme and authority");
            }
        }
        if config.jwt_secret_file.is_relative() {
            config.jwt_secret_file = path
                .parent()
                .unwrap_or(Path::new("."))
                .join(&config.jwt_secret_file);
        }
        if let Some(signal) = &mut config.shutdown_signal_file
            && signal.is_relative()
        {
            *signal = path.parent().unwrap_or(Path::new(".")).join(&*signal);
        }
        if let Some(db) = &mut config.database {
            db.validate()
                .map_err(|_| "invalid database configuration")?;
            if db.password_file.is_relative() {
                db.password_file = path
                    .parent()
                    .unwrap_or(Path::new("."))
                    .join(&db.password_file);
            }
        }
        Ok(config)
    }

    pub fn load_jwt_secret(&self) -> io::Result<Vec<u8>> {
        let secret = fs::read_to_string(&self.jwt_secret_file)
            .map_err(|_| io::Error::other("cannot read JWT secret file"))?;
        let secret = secret.trim().as_bytes().to_vec();
        if secret.len() < 32 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "JWT secret must contain at least 32 bytes",
            ));
        }
        Ok(secret)
    }
}
