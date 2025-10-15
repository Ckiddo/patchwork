use serde::{Deserialize, Serialize};
use util_lib::UserIdentity;

// 更新昵称请求
#[derive(Deserialize)]
pub struct UpdateNicknameReq {
    pub nickname: String,
}

// 更新昵称响应
#[derive(Serialize)]
pub struct UpdateRsp {
    pub jwt: String,
    pub identity:UserIdentity,
}

