pub mod api;
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::HeaderMap,
    routing::{post, put},
};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use reqwest::{Method, StatusCode};
use serde::{Serialize};
use tower_http::cors::{Any, CorsLayer};
use util_lib::{Claims, UserIdentity};
use uuid::Uuid;

use crate::api::auth::{create::CreateRsp, update::{UpdateNicknameReq, UpdateRsp}};

// 应用状态
#[derive(Clone)]
pub struct AppState {
    jwt_secret: String,
}

impl AppState {
    pub fn new(jwt_secret: String) -> Self {
        Self { jwt_secret }
    }
}


// 验证响应
#[derive(Serialize)]
pub struct VerifyRsp {
    pub identity: UserIdentity,
}

// 错误响应
#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

// 设置路由
pub fn auth_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/auth/create", post(create_identity))
        .route("/api/auth/verify", post(verify_identity))
        .route("/api/auth/nickname", put(update_nickname))
}

// 创建新身份
async fn create_identity(
    State(state): State<Arc<AppState>>,
) -> Result<Json<CreateRsp>, (StatusCode, Json<ErrorResponse>)> {
    let user_id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();

    // 默认昵称为 "玩家_" + 随机数
    let random_suffix: String = (0..4)
        .map(|_| rand::random::<u8>() % 10)
        .map(|n| char::from_digit(n as u32, 10).unwrap())
        .collect();
    let nickname = format!("玩家_{}", random_suffix);

    let claims = Claims {
        sub: user_id.clone(),
        nickname: nickname.clone(),
        iat: now,
        exp: now + 24 * 3600,
    };

    let jwt = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(state.jwt_secret.as_bytes()),
    )
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("生成 JWT 失败: {}", e),
            }),
        )
    })?;

    let identity = UserIdentity {
        user_id,
        nickname,
        created_at: now,
    };

    Ok(Json(CreateRsp { jwt, identity }))
}

// 验证身份
async fn verify_identity(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<VerifyRsp>, (StatusCode, Json<ErrorResponse>)> {
    let claims = extract_and_verify_jwt(&state, &headers)?;

    let identity = UserIdentity {
        user_id: claims.sub,
        nickname: claims.nickname,
        created_at: claims.iat,
    };

    Ok(Json(VerifyRsp { identity }))
}

// 更新昵称
async fn update_nickname(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<UpdateNicknameReq>,
) -> Result<Json<UpdateRsp>, (StatusCode, Json<ErrorResponse>)> {
    // 验证昵称
    let nickname = payload.nickname.trim().to_string();

    if nickname.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "昵称不能为空".to_string(),
            }),
        ));
    }

    if nickname.len() > 20 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "昵称不能超过20个字符".to_string(),
            }),
        ));
    }

    // 验证当前 JWT
    let old_claims = extract_and_verify_jwt(&state, &headers)?;

    // 创建新的 JWT（保持相同的 user_id 和 created_at）
    let now = chrono::Utc::now().timestamp();
    let new_claims = Claims {
        sub: old_claims.sub.clone(),
        nickname: nickname.clone(),
        iat: old_claims.iat,
        exp: now + 24 * 3600, // 重新设置 10 年过期时间
    };

    let new_jwt = encode(
        &Header::default(),
        &new_claims,
        &EncodingKey::from_secret(state.jwt_secret.as_bytes()),
    )
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("生成新 JWT 失败: {}", e),
            }),
        )
    })?;

    let identity = UserIdentity {
        user_id: old_claims.sub,
        nickname,
        created_at: old_claims.iat,
    };

    Ok(Json(UpdateRsp {
        jwt: new_jwt,
        identity,
    }))
}

// 从 Header 中提取并验证 JWT
fn extract_and_verify_jwt(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Claims, (StatusCode, Json<ErrorResponse>)> {
    let auth_header = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or((
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "缺少 Authorization header".to_string(),
            }),
        ))?;

    let jwt = auth_header.strip_prefix("Bearer ").ok_or((
        StatusCode::UNAUTHORIZED,
        Json(ErrorResponse {
            error: "Authorization header 格式错误".to_string(),
        }),
    ))?;

    let token_data = decode::<Claims>(
        jwt,
        &DecodingKey::from_secret(state.jwt_secret.as_bytes()),
        &Validation::default(),
    )
    .map_err(|e| {
        (
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: format!("JWT 验证失败: {}", e),
            }),
        )
    })?;

    Ok(token_data.claims)
}

#[tokio::main]
async fn main() {
    // 从环境变量或配置文件读取 JWT secret
    let jwt_secret = std::env::var("JWT_SECRET")
        .expect("fail to load JWT_SECRET");

    let state = Arc::new(AppState::new(jwt_secret));

    let cors = CorsLayer::new()
        .allow_origin([
            "http://127.0.0.1:8080".parse().unwrap(),
            "http://localhost:8080".parse().unwrap(),
        ])
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers(Any);

    let app = Router::new()
        .merge(auth_routes())
        .with_state(state)
        .layer(cors);

    // 启动
    let listener = tokio::net::TcpListener::bind("0.0.0.0:5380").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
