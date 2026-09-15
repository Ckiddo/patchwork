pub mod api;
pub mod audit;
pub mod config;
pub mod game;
pub mod identity;
pub mod instance;
pub mod persistence;
pub mod recovery;
pub mod sessions;
pub mod shutdown;

use actix::Addr;
use jsonwebtoken::{DecodingKey, EncodingKey};

/// Deliberately has no Debug implementation: signing keys are never log fields.
pub struct AppState {
    pub(crate) recovery: recovery::RecoveryConfig,
    pub(crate) websocket_slots: std::sync::Arc<tokio::sync::Semaphore>,
    pub(crate) auth: identity::AuthConfig,
    pub(crate) allowed_origins: Vec<String>,
    pub registry: Addr<sessions::SessionRegistry>,
    pub(crate) database: Option<persistence::Database>,
    pub(crate) encoding_key: EncodingKey,
    pub(crate) decoding_key: DecodingKey,
    pub(crate) lobby: Addr<game::LobbyManager>,
    room_fingerprint_key: Vec<u8>,
}

impl AppState {
    pub fn new(secret: &[u8], lobby: Addr<game::LobbyManager>) -> Self {
        use actix::Actor;
        Self {
            recovery: recovery::RecoveryConfig::default(),
            websocket_slots: std::sync::Arc::new(tokio::sync::Semaphore::new(1024)),
            auth: identity::AuthConfig::default(),
            allowed_origins: Vec::new(),
            registry: sessions::SessionRegistry::with_lobby(lobby.clone()).start(),
            database: None,
            encoding_key: EncodingKey::from_secret(secret),
            decoding_key: DecodingKey::from_secret(secret),
            lobby,
            room_fingerprint_key: {
                use sha2::{Digest, Sha256};
                let mut hash = Sha256::new();
                hash.update(b"patchwork-friend-rooms-v1");
                hash.update(secret);
                hash.finalize().to_vec()
            },
        }
    }
    pub fn with_auth(mut self, auth: identity::AuthConfig, origins: Vec<String>) -> Self {
        self.auth = auth;
        self.allowed_origins = origins;
        self
    }
    pub fn with_database(mut self, database: persistence::Database) -> Self {
        self.lobby.do_send(game::Configure {
            database: database.clone(),
            registry: self.registry.clone(),
            fingerprint_key: self.room_fingerprint_key.clone(),
            password_workers: std::sync::Arc::new(tokio::sync::Semaphore::new(2)),
            recovery: self.recovery.clone(),
            epoch: uuid::Uuid::new_v4(),
        });
        self.database = Some(database);
        self
    }
    pub fn with_recovery(mut self, config: recovery::RecoveryConfig) -> Self {
        self.recovery = config;
        self
    }
}
