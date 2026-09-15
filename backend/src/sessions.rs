//! Single logical connection per user; all dispatch and disconnects use the same fence.
use crate::persistence::{Database, StoreError};
use crate::{
    game::{self, wire},
    persistence::friends::{Outcome, Permit},
};
use actix::prelude::*;
use std::collections::{HashMap, HashSet};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use tokio::sync::{mpsc, watch};
use util_lib::protocol::{
    VERSION,
    v1::{self, client_envelope, server_envelope},
};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub user: Uuid,
    pub session: Uuid,
    pub generation: u64,
}
struct Active {
    stamp: Stamp,
    expires: i64,
    kick: watch::Sender<bool>,
    current: Arc<AtomicBool>,
    push: Option<mpsc::Sender<v1::ServerEnvelope>>,
    database: Database,
    synchronized: Option<Uuid>,
    target: Option<v1::ResumeState>,
    presence_revision: Arc<AtomicU64>,
}
#[derive(Default)]
pub struct SessionRegistry {
    active: HashMap<Uuid, Active>,
    lobby: Option<Addr<game::LobbyManager>>,
    registering: HashSet<Uuid>,
    revision: Arc<AtomicU64>,
}
impl Actor for SessionRegistry {
    type Context = Context<Self>;
    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.set_mailbox_capacity(128);
        ctx.run_interval(std::time::Duration::from_secs(1), |actor, _| {
            let expired = actor
                .active
                .values()
                .filter(|a| !actor.valid(a.stamp))
                .map(|a| a.stamp)
                .collect::<Vec<_>>();
            for stamp in expired {
                actor.disconnect(stamp);
            }
        });
    }
}
#[derive(Message)]
#[rtype(result = "Result<Stamp,StoreError>")]
pub struct Register {
    pub database: Database,
    pub user: Uuid,
    pub session: Uuid,
    pub auth_version: i64,
    pub expires: i64,
    pub kick: watch::Sender<bool>,
}
impl Handler<Register> for SessionRegistry {
    type Result = ResponseActFuture<Self, Result<Stamp, StoreError>>;
    fn handle(&mut self, m: Register, _: &mut Context<Self>) -> Self::Result {
        if self.registering.contains(&m.user)
            || self.registering.len() >= 128
            || (self.active.len() >= 1024 && !self.active.contains_key(&m.user))
        {
            return Box::pin(async { Err(StoreError::PlayerBusy) }.into_actor(self));
        }
        self.registering.insert(m.user);
        self.revision.fetch_add(1, Ordering::SeqCst);
        if let Some(active) = self.active.get(&m.user) {
            active.presence_revision.fetch_add(1, Ordering::SeqCst);
        }
        Box::pin(
            async move {
                let generation = m
                    .database
                    .claim_connection(m.user, m.session, m.auth_version)
                    .await;
                (m, generation)
            }
            .into_actor(self)
            .map(|(m, generation), actor, _| {
                actor.registering.remove(&m.user);
                actor.revision.fetch_add(1, Ordering::SeqCst);
                if generation == Err(StoreError::CommitUnknown)
                    && let Some(stamp) = actor.active.get(&m.user).map(|a| a.stamp)
                {
                    actor.disconnect(stamp);
                }
                let generation = generation?;
                // The durable claim fences the old socket even if the new handshake was cancelled.
                if let Some(stamp) = actor.active.get(&m.user).map(|a| a.stamp) {
                    actor.disconnect(stamp);
                }
                if m.kick.is_closed() || m.expires <= chrono::Utc::now().timestamp() {
                    return Err(StoreError::Permission);
                }
                let stamp = Stamp {
                    user: m.user,
                    session: m.session,
                    generation,
                };
                if let Some(old) = actor.active.insert(
                    m.user,
                    Active {
                        stamp,
                        expires: m.expires,
                        kick: m.kick,
                        current: Arc::new(AtomicBool::new(true)),
                        push: None,
                        database: m.database,
                        synchronized: None,
                        target: None,
                        presence_revision: Arc::new(AtomicU64::new(0)),
                    },
                ) {
                    old.current.store(false, Ordering::SeqCst);
                    let _ = old.kick.send(true);
                }
                Ok(stamp)
            }),
        )
    }
}
impl SessionRegistry {
    pub fn with_lobby(lobby: Addr<game::LobbyManager>) -> Self {
        Self {
            lobby: Some(lobby),
            ..Self::default()
        }
    }
    fn disconnect(&mut self, stamp: Stamp) -> bool {
        if !self
            .active
            .get(&stamp.user)
            .is_some_and(|a| a.stamp == stamp)
        {
            return false;
        }
        let a = self.active.remove(&stamp.user).expect("matching stamp");
        a.presence_revision.fetch_add(1, Ordering::SeqCst);
        a.current.store(false, Ordering::SeqCst);
        let _ = a.kick.send(true);
        self.revision.fetch_add(1, Ordering::SeqCst);
        true
    }
    fn permits(&self) -> HashMap<Uuid, Permit> {
        self.active
            .iter()
            .filter(|(_, a)| self.valid(a.stamp))
            .map(|(u, a)| {
                (
                    *u,
                    Permit {
                        user: *u,
                        session: a.stamp.session,
                        generation: a.stamp.generation,
                        expires: a.expires,
                        current: a.current.clone(),
                    },
                )
            })
            .collect()
    }
    fn user_revisions(&self) -> HashMap<Uuid, (Arc<AtomicU64>, u64)> {
        self.active
            .iter()
            .map(|(user, active)| {
                (
                    *user,
                    (
                        active.presence_revision.clone(),
                        active.presence_revision.load(Ordering::SeqCst),
                    ),
                )
            })
            .collect()
    }
    fn valid(&self, stamp: Stamp) -> bool {
        self.active.get(&stamp.user).is_some_and(|a| {
            a.stamp == stamp
                && a.current.load(Ordering::SeqCst)
                && !a.kick.is_closed()
                && a.expires > chrono::Utc::now().timestamp()
        })
    }
}
#[derive(Message)]
#[rtype(result = "bool")]
pub struct Disconnect(pub Stamp);
impl Handler<Disconnect> for SessionRegistry {
    type Result = bool;
    fn handle(&mut self, m: Disconnect, _: &mut Context<Self>) -> bool {
        self.disconnect(m.0)
    }
}
#[derive(Message)]
#[rtype(result = "()")]
pub struct Revoke {
    pub user: Uuid,
    pub session: Uuid,
}
impl Handler<Revoke> for SessionRegistry {
    type Result = ();
    fn handle(&mut self, m: Revoke, _: &mut Context<Self>) {
        if let Some(stamp) = self
            .active
            .get(&m.user)
            .filter(|a| a.stamp.session == m.session)
            .map(|a| a.stamp)
        {
            self.disconnect(stamp);
        }
    }
}
#[derive(Message)]
#[rtype(result = "Result<v1::ServerEnvelope,StoreError>")]
pub struct Dispatch {
    pub stamp: Stamp,
    pub message: v1::ClientEnvelope,
}
impl Handler<Dispatch> for SessionRegistry {
    type Result = ResponseFuture<Result<v1::ServerEnvelope, StoreError>>;
    fn handle(&mut self, m: Dispatch, ctx: &mut Context<Self>) -> Self::Result {
        if !self.valid(m.stamp) {
            return Box::pin(async { Err(StoreError::Permission) });
        }
        if matches!(
            m.message.payload,
            Some(client_envelope::Payload::Resume(_) | client_envelope::Payload::SyncAck(_))
        ) {
            let registry = ctx.address();
            return Box::pin(async move {
                let id = m.message.request_id;
                let result = registry
                    .send(Synchronize {
                        stamp: m.stamp,
                        payload: m.message.payload.expect("matched"),
                    })
                    .await
                    .unwrap_or(Err(StoreError::Unavailable));
                Ok(wire::envelope(Some(id), result.unwrap_or_else(wire::error)))
            });
        }
        if let Some(client_envelope::Payload::Lobby(request)) = m.message.payload.clone() {
            let lobby = self.lobby.clone();
            let online = self.permits();
            let permit = online
                .get(&m.stamp.user)
                .cloned()
                .expect("validated permit");
            return Box::pin(async move {
                let id = m.message.request_id;
                let result = match lobby {
                    Some(lobby) => lobby
                        .send(game::Request {
                            permit,
                            online,
                            request_id: id.clone(),
                            request,
                        })
                        .await
                        .unwrap_or(Err(StoreError::Unavailable)),
                    None => Err(StoreError::Unavailable),
                };
                Ok(wire::envelope(Some(id), result.unwrap_or_else(wire::error)))
            });
        }
        if let Some(client_envelope::Payload::Game(request)) = m.message.payload.clone() {
            if self
                .active
                .get(&m.stamp.user)
                .and_then(|a| a.synchronized)
                .is_none_or(|id| id.to_string() != request.game_id)
            {
                return Box::pin(async move {
                    Ok(wire::envelope(
                        Some(m.message.request_id),
                        wire::error(StoreError::SyncRequired),
                    ))
                });
            }
            let online = self.permits();
            let permit = online
                .get(&m.stamp.user)
                .cloned()
                .expect("validated permit");
            let presence = crate::persistence::recovery::Presence {
                online,
                synchronized: self
                    .active
                    .iter()
                    .filter(|(_, a)| self.valid(a.stamp))
                    .filter_map(|(u, a)| a.synchronized.map(|g| (*u, g)))
                    .collect(),
                registering: self.registering.clone(),
                revision: self.revision.clone(),
                observed_revision: self.revision.load(Ordering::SeqCst),
                user_revisions: self.user_revisions(),
            };
            let lobby = self.lobby.clone();
            return Box::pin(async move {
                let id = m.message.request_id;
                let result = match lobby {
                    Some(lobby) => lobby
                        .send(game::GameRequest {
                            permit,
                            request_id: id.clone(),
                            request,
                            presence,
                        })
                        .await
                        .unwrap_or(Err(StoreError::Unavailable)),
                    None => Err(StoreError::Unavailable),
                };
                Ok(wire::envelope(Some(id), result.unwrap_or_else(wire::error)))
            });
        }
        let payload = match m.message.payload {
            Some(client_envelope::Payload::Ping(p)) => {
                server_envelope::Payload::Pong(v1::Pong { nonce: p.nonce })
            }
            _ => server_envelope::Payload::Error(v1::ErrorResponse {
                code: v1::ErrorCode::NotImplemented as i32,
                retryable: false,
            }),
        };
        Box::pin(async move {
            Ok(v1::ServerEnvelope {
                protocol_version: VERSION,
                request_id: Some(m.message.request_id),
                event_seq: 0,
                payload: Some(payload),
            })
        })
    }
}

#[derive(Message)]
#[rtype(result = "bool")]
pub struct Attach {
    pub stamp: Stamp,
    pub push: mpsc::Sender<v1::ServerEnvelope>,
}
impl Handler<Attach> for SessionRegistry {
    type Result = bool;
    fn handle(&mut self, m: Attach, _: &mut Context<Self>) -> bool {
        if !self.valid(m.stamp) {
            return false;
        }
        self.active.get_mut(&m.stamp.user).expect("valid").push = Some(m.push);
        true
    }
}
#[derive(Message)]
#[rtype(result = "()")]
pub struct Publish(pub Outcome);
impl Handler<Publish> for SessionRegistry {
    type Result = ();
    fn handle(&mut self, m: Publish, _: &mut Context<Self>) {
        let online = self.permits();
        let room = wire::envelope(
            None,
            server_envelope::Payload::Room(wire::room(&m.0.room, &online)),
        );
        let game =
            m.0.game
                .as_ref()
                .map(|g| wire::envelope(None, server_envelope::Payload::Game(wire::game(g))));
        let mut failed = Vec::new();
        for user in m.0.recipients {
            let Some(active) = self.active.get(&user) else {
                continue;
            };
            if !self.valid(active.stamp) {
                continue;
            }
            let Some(push) = &active.push else { continue };
            if push.try_send(room.clone()).is_err()
                || game
                    .as_ref()
                    .is_some_and(|g| push.try_send(g.clone()).is_err())
            {
                failed.push(active.stamp);
            }
        }
        for stamp in failed {
            self.disconnect(stamp);
        }
    }
}
#[derive(Message)]
#[rtype(result = "crate::persistence::recovery::Presence")]
pub struct Snapshot;
impl Handler<Snapshot> for SessionRegistry {
    type Result = MessageResult<Snapshot>;
    fn handle(&mut self, _: Snapshot, _: &mut Context<Self>) -> Self::Result {
        MessageResult(crate::persistence::recovery::Presence {
            online: self.permits(),
            synchronized: self
                .active
                .iter()
                .filter(|(_, a)| self.valid(a.stamp))
                .filter_map(|(u, a)| a.synchronized.map(|g| (*u, g)))
                .collect(),
            registering: self.registering.clone(),
            revision: self.revision.clone(),
            observed_revision: self.revision.load(Ordering::SeqCst),
            user_revisions: self.user_revisions(),
        })
    }
}
#[derive(Message)]
#[rtype(result = "Result<server_envelope::Payload,StoreError>")]
struct Synchronize {
    stamp: Stamp,
    payload: client_envelope::Payload,
}
impl Handler<Synchronize> for SessionRegistry {
    type Result = ResponseActFuture<Self, Result<server_envelope::Payload, StoreError>>;
    fn handle(&mut self, m: Synchronize, _: &mut Context<Self>) -> Self::Result {
        let Some(p) = self
            .permits()
            .get(&m.stamp.user)
            .filter(|p| p.generation == m.stamp.generation)
            .cloned()
        else {
            return Box::pin(async { Err(StoreError::Permission) }.into_actor(self));
        };
        let active = self.active.get_mut(&m.stamp.user).expect("permit");
        let db = active.database.clone();
        let (request, ack) = match m.payload {
            client_envelope::Payload::Resume(r) => {
                active.synchronized = None;
                active.target = None;
                active.presence_revision.fetch_add(1, Ordering::SeqCst);
                self.revision.fetch_add(1, Ordering::SeqCst);
                (r, None)
            }
            client_envelope::Payload::SyncAck(a) => {
                if !active.target.as_ref().is_some_and(|t| {
                    t.sync_token == a.sync_token
                        && t.game_id == a.game_id
                        && t.version == a.version
                        && t.event_seq == a.event_seq
                }) {
                    return Box::pin(async { Err(StoreError::VersionConflict) }.into_actor(self));
                }
                (
                    v1::ResumeRequest {
                        game_id: a.game_id.clone(),
                        last_seq: a.event_seq,
                        has_snapshot: true,
                    },
                    Some(a),
                )
            }
            _ => return Box::pin(async { Err(StoreError::InvalidInput) }.into_actor(self)),
        };
        Box::pin(
            async move { db.resume_game(p, request).await }
                .into_actor(self)
                .map(move |result, actor, _| {
                    let result = result?;
                    if !actor.valid(m.stamp) {
                        return Err(StoreError::Permission);
                    }
                    let active = actor.active.get_mut(&m.stamp.user).expect("valid");
                    if let Some(ack) = ack {
                        if result.version != ack.version
                            || result.event_seq != ack.event_seq
                            || !active
                                .target
                                .as_ref()
                                .is_some_and(|t| t.sync_token == ack.sync_token)
                        {
                            return Err(StoreError::VersionConflict);
                        }
                        active.synchronized = Uuid::parse_str(&ack.game_id).ok();
                        active.presence_revision.fetch_add(1, Ordering::SeqCst);
                        actor.revision.fetch_add(1, Ordering::SeqCst);
                        Ok(server_envelope::Payload::Acknowledged(v1::Acknowledged {
                            room_version: 0,
                            game_version: ack.version,
                        }))
                    } else {
                        active.target = Some(result.clone());
                        Ok(server_envelope::Payload::Resumed(result))
                    }
                }),
        )
    }
}
