use crate::{
    AppState,
    api::error::ApiError,
    identity,
    sessions::{Attach, Disconnect, Dispatch, Register, Stamp},
};
use actix_web::{HttpRequest, HttpResponse, web};
use actix_ws::{CloseCode, CloseReason, Message};
use prost::Message as _;
use std::time::Duration;
use tokio::time::Instant;
use util_lib::protocol::{
    self,
    v1::{self, client_envelope, server_envelope},
};

pub async fn connect(
    req: HttpRequest,
    body: web::Payload,
    state: web::Data<AppState>,
) -> Result<HttpResponse, actix_web::Error> {
    let origin = req.headers().get("Origin").and_then(|v| v.to_str().ok());
    if !req.query_string().is_empty()
        || !origin.is_some_and(|o| state.allowed_origins.iter().any(|a| a == o))
    {
        return Err(ApiError::forbidden().into());
    }
    identity::database(&state)?;
    let slot = state
        .websocket_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| actix_web::error::ErrorServiceUnavailable("connection capacity"))?;
    let (response, session, stream) = actix_ws::handle(&req, body)?;
    actix_web::rt::spawn(async move {
        let _slot = slot;
        connection_session(
            state,
            session,
            stream.max_frame_size(protocol::MAX_CLIENT_MESSAGE_BYTES),
        )
        .await;
    });
    Ok(response)
}
async fn send(session: &mut actix_ws::Session, message: v1::ServerEnvelope) -> bool {
    if message.encoded_len() > protocol::MAX_SERVER_MESSAGE_BYTES {
        return false;
    }
    matches!(
        tokio::time::timeout(
            Duration::from_secs(1),
            session.binary(message.encode_to_vec())
        )
        .await,
        Ok(Ok(()))
    )
}
// Every exit, including cancellation and partial authentication, uses the same fence.
struct Lease {
    registry: actix::Addr<crate::sessions::SessionRegistry>,
    stamp: Option<Stamp>,
}
impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(stamp) = self.stamp {
            self.registry.do_send(Disconnect(stamp));
        }
    }
}
pub(crate) struct RateLimit {
    tokens: f64,
    updated: Instant,
}
impl RateLimit {
    pub(crate) fn new() -> Self {
        Self {
            tokens: 40.0,
            updated: Instant::now(),
        }
    }
    pub(crate) fn allow(&mut self) -> bool {
        let now = Instant::now();
        self.tokens =
            (self.tokens + now.duration_since(self.updated).as_secs_f64() * 20.0).min(40.0);
        self.updated = now;
        if self.tokens < 1.0 {
            false
        } else {
            self.tokens -= 1.0;
            true
        }
    }
}
async fn connection_session(
    state: web::Data<AppState>,
    mut session: actix_ws::Session,
    mut stream: actix_ws::MessageStream,
) {
    let mut lease = Lease {
        registry: state.registry.clone(),
        stamp: None,
    };
    let (kick, mut kicked) = tokio::sync::watch::channel(false);
    let (push, mut outgoing) = tokio::sync::mpsc::channel(32);
    let mut rate = RateLimit::new();
    let authenticate = async {
        let frame = loop {
            let msg = stream.recv().await;
            if !rate.allow() {
                return Err(());
            }
            match msg {
                Some(Ok(Message::Binary(bytes))) => break bytes,
                Some(Ok(Message::Ping(bytes))) => session.pong(&bytes).await.map_err(|_| ())?,
                _ => return Err(()),
            }
        };
        let envelope = protocol::decode_client(&frame).map_err(|_| ())?;
        let Some(client_envelope::Payload::Authenticate(auth)) = envelope.payload else {
            return Err(());
        };
        let principal = identity::authenticate(&state, &auth.access_token)
            .await
            .map_err(|_| ())?;
        let stamp = state
            .registry
            .send(Register {
                database: identity::database(&state).map_err(|_| ())?.clone(),
                user: principal.profile.id,
                session: principal.session.ok_or(())?,
                auth_version: principal.profile.auth_version,
                expires: principal.expires,
                kick,
            })
            .await
            .map_err(|_| ())?
            .map_err(|_| ())?;
        lease.stamp = Some(stamp);
        if !state
            .registry
            .send(Attach { stamp, push })
            .await
            .map_err(|_| ())?
        {
            return Err(());
        }
        Ok((stamp, principal.expires, envelope.request_id))
    };
    let authenticated = tokio::time::timeout(
        Duration::from_secs(state.auth.websocket_auth_timeout_secs),
        authenticate,
    )
    .await;
    let mut close_code = CloseCode::Away;
    let mut close_cause = "transport_or_timeout";
    if let Ok(Ok((stamp, expires, id))) = authenticated {
        let response = crate::game::wire::envelope(
            Some(id),
            server_envelope::Payload::Authenticated(v1::Authenticated {
                user_id: stamp.user.to_string(),
                generation: stamp.generation,
            }),
        );
        if send(&mut session, response).await {
            let expiration = tokio::time::sleep(Duration::from_secs(
                expires.saturating_sub(chrono::Utc::now().timestamp()) as u64,
            ));
            tokio::pin!(expiration);
            let mut last_input = Instant::now();
            let mut heartbeat = tokio::time::interval(Duration::from_secs(1));
            type Pending =
                futures_util::future::LocalBoxFuture<'static, Option<v1::ServerEnvelope>>;
            let mut pending: Option<Pending> = None;
            loop {
                tokio::select! {
                    biased;
                    _=kicked.changed()=>{
                        close_cause="registry";
                        // Registry expiry can win the race with this task's timer.
                        // Expiry must remain reconnectable so the browser rotates its token.
                        close_code=if expires<=chrono::Utc::now().timestamp() {
                            CloseCode::Away
                        } else {
                            CloseCode::Policy
                        };
                        break;
                    },
                    _=&mut expiration=>break,
                    _=heartbeat.tick()=>{
                        if last_input.elapsed()>=Duration::from_secs(45) {break;}
                    },
                    reply=async {match pending.as_mut() {Some(f)=>f.await,None=>std::future::pending().await}}=>{
                        pending=None;
                        let Some(reply)=reply else {break};
                        if !send(&mut session,reply).await {break;}
                    },
                    msg=stream.recv()=>{
                        if !rate.allow() {
                            close_cause="rate_limit";
                            let _=send(&mut session,protocol::ProtocolError(v1::ErrorCode::RateLimited).response()).await;
                            close_code=CloseCode::Policy;break;
                        }
                        match msg {
                            Some(Ok(Message::Binary(bytes)))=>{
                                let message=match protocol::decode_client(&bytes) {
                                    Ok(m)=>m,
                                    Err(e)=>{close_cause="invalid_protocol";let _=send(&mut session,e.response()).await;close_code=CloseCode::Policy;break;},
                                };
                                last_input=Instant::now();
                                match message.payload.as_ref() {
                                    Some(client_envelope::Payload::Authenticate(_))=>{close_cause="repeated_auth";close_code=CloseCode::Policy;break;},
                                    Some(client_envelope::Payload::Ping(p))=>{
                                        if !send(&mut session,crate::game::wire::envelope(Some(message.request_id),server_envelope::Payload::Pong(v1::Pong {nonce:p.nonce}))).await {break;}
                                    },
                                    _ if pending.is_some()=>{
                                        if !send(&mut session,crate::game::wire::envelope(Some(message.request_id),crate::game::wire::error(crate::persistence::StoreError::PlayerBusy))).await {break;}
                                    },
                                    _=>{
                                        let registry=state.registry.clone();
                                        pending=Some(Box::pin(async move {
                                            tokio::time::timeout(Duration::from_secs(10),registry.send(Dispatch {stamp,message})).await.ok()?.ok()?.ok()
                                        }));
                                    },
                                }
                            },
                            Some(Ok(Message::Ping(bytes)))=>{
                                if !matches!(tokio::time::timeout(Duration::from_secs(1),session.pong(&bytes)).await,Ok(Ok(()))) {break;}
                            },
                            Some(Ok(Message::Pong(_)))=>{},
                            _=>break,
                        }
                    },
                    push=outgoing.recv()=>{
                        let Some(message)=push else {break};
                        if !send(&mut session,message).await {break;}
                    },
                }
            }
        }
    } else {
        close_cause = "authentication";
        close_code = CloseCode::Policy;
        let _ = send(
            &mut session,
            protocol::ProtocolError(v1::ErrorCode::Unauthenticated).response(),
        )
        .await;
    }
    tracing::info!(target: "patchwork_audit", event="websocket_closed", cause=close_cause, code=u16::from(close_code));
    drop(lease);
    let _ = tokio::time::timeout(
        Duration::from_secs(1),
        session.close(Some(CloseReason {
            code: close_code,
            description: Some("session ended".into()),
        })),
    )
    .await;
}
