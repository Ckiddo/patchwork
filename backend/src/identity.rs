use crate::{
    AppState,
    api::error::ApiError,
    persistence::{
        Database, StoreError,
        identity::{LegacyProfile, Profile},
    },
};
use jsonwebtoken::{Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use util_lib::{Claims, UserIdentity};
use uuid::Uuid;

#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuthConfig {
    pub allow_legacy_migration: bool,
    pub websocket_auth_timeout_secs: u64,
}
impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            allow_legacy_migration: false,
            websocket_auth_timeout_secs: 5,
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct AccessClaims {
    #[serde(flatten)]
    pub claims: Claims,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sid: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_version: Option<i64>,
}
pub struct Principal {
    pub profile: Profile,
    pub session: Option<Uuid>,
    pub expires: i64,
}
pub fn database(state: &AppState) -> Result<&Database, ApiError> {
    state.database.as_ref().ok_or_else(ApiError::unavailable)
}
pub fn db_error(error: StoreError) -> ApiError {
    match error {
        StoreError::Permission => ApiError::unauthorized("会话无效或已过期"),
        StoreError::InvalidInput => ApiError::bad_request("无效的会话参数"),
        _ => ApiError::unavailable(),
    }
}
pub fn token_hash(token: &str) -> Result<Vec<u8>, ApiError> {
    if token.len() != 64
        || !token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(ApiError::bad_request("恢复凭据格式无效"));
    }
    Ok(Sha256::digest(token.as_bytes()).to_vec())
}
pub fn random_token() -> String {
    let bytes: [u8; 32] = rand::random();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn public_profile(p: &Profile) -> UserIdentity {
    UserIdentity {
        user_id: p.id.to_string(),
        nickname: p.nickname.clone(),
        created_at: p.created_at,
    }
}
pub fn sign(
    state: &AppState,
    p: &Profile,
    sid: Option<Uuid>,
    expires: Option<i64>,
) -> Result<String, ApiError> {
    let now = chrono::Utc::now().timestamp();
    let claims = AccessClaims {
        claims: Claims {
            sub: p.id.to_string(),
            nickname: p.nickname.clone(),
            iat: now,
            exp: expires.unwrap_or(now + 15 * 60),
        },
        sid,
        auth_version: sid.map(|_| p.auth_version),
    };
    encode(&Header::default(), &claims, &state.encoding_key).map_err(|_| ApiError::internal())
}
pub fn decode_token(state: &AppState, token: &str) -> Result<AccessClaims, ApiError> {
    if token.len() > 4096 {
        return Err(ApiError::unauthorized("访问令牌无效"));
    }
    let mut validation = Validation::default();
    validation.leeway = 0;
    let claims = decode::<AccessClaims>(token, &state.decoding_key, &validation)
        .map_err(|_| ApiError::unauthorized("访问令牌无效或已过期"))?
        .claims;
    if claims.claims.iat > chrono::Utc::now().timestamp() + 30
        || claims.claims.iat < 0
        || claims.claims.exp <= claims.claims.iat
        || claims.sid.is_some() != claims.auth_version.is_some()
        || Uuid::parse_str(&claims.claims.sub).is_err()
    {
        return Err(ApiError::unauthorized("访问令牌无效"));
    }
    Ok(claims)
}
pub fn legacy(state: &AppState, c: &AccessClaims) -> Result<LegacyProfile, ApiError> {
    if !state.auth.allow_legacy_migration || c.sid.is_some() || c.auth_version.is_some() {
        return Err(ApiError::unauthorized("旧身份迁入未启用"));
    }
    let nickname = c.claims.nickname.trim();
    if nickname.is_empty() || nickname.chars().count() > 20 {
        return Err(ApiError::unauthorized("旧身份资料无效"));
    }
    Ok(LegacyProfile {
        id: Uuid::parse_str(&c.claims.sub).map_err(|_| ApiError::unauthorized("访问令牌无效"))?,
        nickname: nickname.into(),
        created_at: c.claims.iat,
    })
}
pub async fn authenticate(state: &AppState, token: &str) -> Result<Principal, ApiError> {
    let c = decode_token(state, token)?;
    let db = database(state)?;
    let profile = if let (Some(sid), Some(version)) = (c.sid, c.auth_version) {
        db.session_profile(
            Uuid::parse_str(&c.claims.sub).map_err(|_| ApiError::unauthorized("访问令牌无效"))?,
            sid,
            version,
        )
        .await
        .map_err(db_error)?
    } else {
        db.legacy_profile(legacy(state, &c)?)
            .await
            .map_err(db_error)?
    };
    Ok(Principal {
        profile,
        session: c.sid,
        expires: c.claims.exp,
    })
}
