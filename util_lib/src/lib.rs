use serde::{Deserialize, Serialize};

// JWT Claims
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    pub sub: String,      // user_id
    pub nickname: String, // 用户昵称
    pub iat: i64,         // issued at
    pub exp: i64,         // expiration
}

// 用户身份信息
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UserIdentity {
    pub user_id: String,
    pub nickname: String,
    pub created_at: i64,
}