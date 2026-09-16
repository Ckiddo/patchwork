use prost::Message;
use serde::Deserialize;
use util_lib::protocol::{
    self,
    v1::{self, client_envelope, server_envelope},
};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/src/browser_session.mjs")]
extern "C" {
    #[wasm_bindgen(js_name=reconnectDelay)]
    pub async fn reconnect_delay(attempt: u32);
    #[wasm_bindgen(catch,js_name=initializeSession)]
    async fn initialize_session(base: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch,js_name=startNewSession)]
    async fn start_new_session(base: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch,js_name=openSessionSocket)]
    async fn open_socket(url: &str, auth: &[u8], ping: &[u8]) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(js_name=closeSessionSocket)]
    fn close_socket();
}
#[derive(Deserialize)]
struct Session {
    jwt: String,
    identity: util_lib::UserIdentity,
}

#[derive(Clone, PartialEq)]
pub struct SessionFailure {
    pub message: &'static str,
    pub can_start_new: bool,
}
impl From<&'static str> for SessionFailure {
    fn from(message: &'static str) -> Self {
        Self {
            message,
            can_start_new: false,
        }
    }
}
impl SessionFailure {
    fn from_js(error: JsValue) -> Self {
        let kind = error
            .dyn_ref::<js_sys::Error>()
            .map(|e| String::from(e.message()))
            .unwrap_or_default();
        let message = match kind.as_str() {
            "legacy_unavailable" => {
                "旧版身份未能迁入当前服务，旧凭据已保留。可以重试原身份，或使用新身份进入。"
            }
            "session_expired" => {
                "原身份的会话已失效，无法续期，旧凭据已保留。可以重试原身份，或使用新身份进入。"
            }
            "network" => "无法连接身份服务，请检查网络后重试。本地身份已保留。",
            "blocked" => "身份请求被服务拒绝（403），请稍后重试。本地身份已保留。",
            "rate_limited" => "身份请求过于频繁，请稍后重试。本地身份已保留。",
            "server" => "身份服务暂时无法处理请求，请稍后重试。本地身份已保留。",
            "storage" => {
                "无法读取或保存本地身份，请允许此网站存储数据并检查可用空间。请勿清除原凭据。"
            }
            "locks" => {
                "当前浏览器不支持身份保护所需的功能，请使用新版 Chrome 或 Edge 通过 HTTPS 访问。"
            }
            "configuration" => "身份服务地址配置有误，请等待网站更新。",
            "response" => "身份服务返回了无效响应，请重试。本地身份已保留。",
            "unauthorized" => "身份请求未通过认证，请重试。本地凭据已保留。",
            _ => "身份初始化失败，请重试。本地凭据已保留。",
        };
        Self {
            message,
            can_start_new: matches!(kind.as_str(), "legacy_unavailable" | "session_expired"),
        }
    }
}

pub async fn initialize(base: &str) -> Result<(String, String, String), SessionFailure> {
    initialize_inner(base, false).await
}

pub async fn start_new_identity(base: &str) -> Result<(String, String, String), SessionFailure> {
    initialize_inner(base, true).await
}

async fn initialize_inner(
    base: &str,
    start_new: bool,
) -> Result<(String, String, String), SessionFailure> {
    if base.is_empty() {
        return Err("尚未配置身份服务地址，请配置 PATCHWORK_API_BASE 后构建".into());
    }
    let value = if start_new {
        start_new_session(base).await
    } else {
        initialize_session(base).await
    }
    .map_err(SessionFailure::from_js)?;
    let session: Session = serde_wasm_bindgen::from_value(value)
        .map_err(|_| SessionFailure::from("身份响应无效，本地凭据已保留"))?;
    let auth = v1::ClientEnvelope {
        protocol_version: protocol::VERSION,
        request_id: "authenticate".into(),
        payload: Some(client_envelope::Payload::Authenticate(v1::Authenticate {
            access_token: session.jwt.clone(),
        })),
    }
    .encode_to_vec();
    let ping = v1::ClientEnvelope {
        protocol_version: protocol::VERSION,
        request_id: "heartbeat".into(),
        payload: Some(client_envelope::Payload::Ping(v1::Ping { nonce: 1 })),
    }
    .encode_to_vec();
    let url = format!(
        "{}/ws",
        base.trim_end_matches('/').replacen("http", "ws", 1)
    );
    let first = open_socket(&url, &auth, &ping)
        .await
        .map_err(|_| SessionFailure::from("身份已保存，但游戏连接未成功，请重试"))?;
    let bytes = js_sys::Uint8Array::new(&first).to_vec();
    let valid=v1::ServerEnvelope::decode(bytes.as_slice()).ok().is_some_and(|m|m.protocol_version==protocol::VERSION && matches!(m.payload,Some(server_envelope::Payload::Authenticated(a)) if a.user_id==session.identity.user_id));
    if !valid {
        close_socket();
        return Err("游戏连接认证失败，本地身份已保留".into());
    }
    Ok((
        session.jwt,
        session.identity.user_id,
        session.identity.nickname,
    ))
}
