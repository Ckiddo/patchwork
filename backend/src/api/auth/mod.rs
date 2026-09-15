pub mod create;
pub mod update;
use crate::{AppState, api::error::ApiError, identity as auth};
use actix_web::{HttpRequest, web};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/create", web::post().to(create_identity))
        .route("/verify", web::post().to(verify_identity))
        .route("/nickname", web::put().to(update_nickname))
        .route("/session", web::post().to(exchange_legacy))
        .route("/refresh", web::post().to(refresh))
        .route("/logout", web::post().to(logout));
}
pub fn bearer(req: &HttpRequest) -> Result<&str, ApiError> {
    req.headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| ApiError::unauthorized("缺少或无效的 Authorization header"))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionCandidate {
    pub session_id: Uuid,
    pub refresh_token: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefreshRequest {
    pub session_id: Uuid,
    pub refresh_token: String,
    pub next_refresh_token: String,
    pub rotation_id: Uuid,
}
fn session_response(
    state: &AppState,
    p: &crate::persistence::identity::Profile,
    c: SessionCandidate,
) -> Result<web::Json<create::CreateRsp>, ApiError> {
    Ok(web::Json(create::CreateRsp {
        jwt: auth::sign(state, p, Some(c.session_id), None)?,
        identity: auth::public_profile(p),
        session_id: c.session_id.to_string(),
        refresh_token: c.refresh_token,
    }))
}
async fn create_identity(
    state: web::Data<AppState>,
    body: web::Bytes,
) -> Result<web::Json<create::CreateRsp>, ApiError> {
    if body.len() > 4096 {
        return Err(ApiError::bad_request("请求过大"));
    }
    let c = if body.is_empty() {
        SessionCandidate {
            session_id: Uuid::new_v4(),
            refresh_token: auth::random_token(),
        }
    } else {
        serde_json::from_slice(&body).map_err(|_| ApiError::bad_request("无效的会话参数"))?
    };
    let hash = auth::token_hash(&c.refresh_token)?;
    let p = auth::database(&state)?
        .create_identity_session(c.session_id, hash, None)
        .await
        .map_err(auth::db_error)?;
    session_response(&state, &p, c)
}
#[derive(Serialize)]
struct VerifyRsp {
    identity: util_lib::UserIdentity,
}
async fn verify_identity(
    state: web::Data<AppState>,
    req: HttpRequest,
) -> Result<web::Json<VerifyRsp>, ApiError> {
    let p = auth::authenticate(&state, bearer(&req)?).await?;
    Ok(web::Json(VerifyRsp {
        identity: auth::public_profile(&p.profile),
    }))
}
pub async fn me(
    state: web::Data<AppState>,
    req: HttpRequest,
) -> Result<web::Json<util_lib::UserIdentity>, ApiError> {
    let p = auth::authenticate(&state, bearer(&req)?).await?;
    Ok(web::Json(auth::public_profile(&p.profile)))
}
async fn update_nickname(
    state: web::Data<AppState>,
    req: HttpRequest,
    payload: web::Json<update::UpdateNicknameReq>,
) -> Result<web::Json<update::UpdateRsp>, ApiError> {
    let p = auth::authenticate(&state, bearer(&req)?).await?;
    let nickname = payload.nickname.trim();
    if nickname.is_empty() || nickname.chars().count() > 20 {
        return Err(ApiError::bad_request("昵称必须为 1 到 20 个字符"));
    }
    let profile = auth::database(&state)?
        .rename_identity(
            p.profile.id,
            p.session,
            p.profile.auth_version,
            nickname.into(),
        )
        .await
        .map_err(auth::db_error)?;
    Ok(web::Json(update::UpdateRsp {
        jwt: auth::sign(&state, &profile, p.session, Some(p.expires))?,
        identity: auth::public_profile(&profile),
    }))
}
async fn exchange_legacy(
    state: web::Data<AppState>,
    req: HttpRequest,
    c: web::Json<SessionCandidate>,
) -> Result<web::Json<create::CreateRsp>, ApiError> {
    let claims = auth::decode_token(&state, bearer(&req)?)?;
    let legacy = auth::legacy(&state, &claims)?;
    let hash = auth::token_hash(&c.refresh_token)?;
    let p = auth::database(&state)?
        .create_identity_session(c.session_id, hash, Some(legacy))
        .await
        .map_err(auth::db_error)?;
    session_response(&state, &p, c.into_inner())
}
async fn refresh(
    state: web::Data<AppState>,
    r: web::Json<RefreshRequest>,
) -> Result<web::Json<create::CreateRsp>, ApiError> {
    let old = auth::token_hash(&r.refresh_token)?;
    let next = auth::token_hash(&r.next_refresh_token)?;
    let p = auth::database(&state)?
        .rotate_session(r.session_id, old, next, r.rotation_id)
        .await
        .map_err(auth::db_error)?;
    session_response(
        &state,
        &p,
        SessionCandidate {
            session_id: r.session_id,
            refresh_token: r.next_refresh_token.clone(),
        },
    )
}
async fn logout(
    state: web::Data<AppState>,
    req: HttpRequest,
) -> Result<web::Json<serde_json::Value>, ApiError> {
    let p = auth::authenticate(&state, bearer(&req)?).await?;
    let sid = p
        .session
        .ok_or_else(|| ApiError::unauthorized("请先迁入持久会话"))?;
    auth::database(&state)?
        .revoke_session(p.profile.id, sid)
        .await
        .map_err(auth::db_error)?;
    state
        .registry
        .send(crate::sessions::Revoke {
            user: p.profile.id,
            session: sid,
        })
        .await
        .map_err(|_| ApiError::unavailable())?;
    Ok(web::Json(serde_json::json!({"ok":true})))
}
