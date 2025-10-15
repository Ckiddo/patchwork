use serde::Serialize;
use util_lib::UserIdentity;


// 创建新身份的响应
#[derive(Serialize)]
pub struct CreateRsp {
    pub jwt: String,
    pub identity: UserIdentity,
}