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
pub async fn initialize(base: &str) -> Result<(String, String, String), String> {
    if base.is_empty() {
        return Err("尚未配置身份服务地址，请配置 PATCHWORK_API_BASE 后构建".into());
    }
    let value = initialize_session(base).await.map_err(|_| {
        "身份服务暂不可用或会话已失效。本地凭据已保留，请重试；旧身份需要服务端启用迁入".to_string()
    })?;
    let session: Session = serde_wasm_bindgen::from_value(value)
        .map_err(|_| "身份响应无效，本地凭据已保留".to_string())?;
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
        .map_err(|_| "身份已保存，但游戏连接未成功，请重试".to_string())?;
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
