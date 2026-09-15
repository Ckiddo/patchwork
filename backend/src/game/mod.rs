//! One lobby creates each Room actor and reserves a user until a mutation is resolved.
mod room;
pub mod wire;
use crate::{
    persistence::{Database, StoreError as E, friends::*},
    sessions::{Publish, SessionRegistry},
};
use actix::prelude::*;
use argon2::{
    Argon2, PasswordHasher, PasswordVerifier,
    password_hash::{PasswordHash, SaltString},
};
use hmac::{Hmac, Mac};
use prost::Message as _;
use sha2::Sha256;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{Semaphore, oneshot};
use util_lib::protocol::v1::{self, lobby_request::Command, server_envelope::Payload};
use uuid::Uuid;

#[derive(Default)]
pub struct LobbyManager {
    config: Option<Configure>,
    rooms: HashMap<Uuid, Addr<room::Room>>,
    occupancy: HashMap<Uuid, Uuid>,
    pending: HashMap<Uuid, Uuid>,
    ready: bool,
    versions: HashMap<Uuid, i64>,
}
impl Actor for LobbyManager {
    type Context = Context<Self>;
    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.set_mailbox_capacity(128);
        ctx.run_interval(std::time::Duration::from_secs(30), |actor, _| {
            actor.rooms.retain(|_, room| room.connected());
            actor.versions.retain(|id, _| actor.rooms.contains_key(id));
        });
    }
}
#[derive(Clone, Message)]
#[rtype(result = "()")]
pub struct Configure {
    pub database: Database,
    pub registry: Addr<SessionRegistry>,
    pub fingerprint_key: Vec<u8>,
    pub password_workers: Arc<Semaphore>,
    pub recovery: crate::recovery::RecoveryConfig,
    pub epoch: Uuid,
}
impl Handler<Configure> for LobbyManager {
    type Result = AtomicResponse<Self, ()>;
    fn handle(&mut self, m: Configure, _: &mut Context<Self>) -> Self::Result {
        self.ready = false;
        AtomicResponse::new(Box::pin(
            async move {
                let index = match m.database.recovery_boot(m.epoch, m.recovery.clone()).await {
                    Ok(()) => m.database.friend_index().await,
                    Err(e) => Err(e),
                };
                (m, index)
            }
            .into_actor(self)
            .map(|(m, index), actor, ctx| match index {
                Ok(index) => {
                    actor.occupancy = index.into_iter().collect();
                    for id in actor.occupancy.values() {
                        actor.rooms.entry(*id).or_insert_with(|| {
                            room::Room::new(*id, m.clone(), ctx.address()).start()
                        });
                    }
                    actor.ready = true;
                    actor.config = Some(m);
                }
                Err(_) => {
                    ctx.notify_later(m, std::time::Duration::from_secs(2));
                }
            }),
        ))
    }
}
#[derive(Message)]
#[rtype(result = "()")]
pub struct Probe;
impl Handler<Probe> for LobbyManager {
    type Result = ();
    fn handle(&mut self, _: Probe, _: &mut Context<Self>) {}
}
#[derive(Message)]
#[rtype(result = "bool")]
pub struct RoomsReady;
/// Read-only diagnostic used by concurrency tests and future internal metrics.
#[derive(Message)]
#[rtype(result = "usize")]
pub struct PendingCount;
impl Handler<PendingCount> for LobbyManager {
    type Result = usize;
    fn handle(&mut self, _: PendingCount, _: &mut Context<Self>) -> usize {
        self.pending.len()
    }
}
impl Handler<RoomsReady> for LobbyManager {
    type Result = bool;
    fn handle(&mut self, _: RoomsReady, _: &mut Context<Self>) -> bool {
        self.ready
    }
}
#[derive(Message)]
#[rtype(result = "()")]
pub struct IndexChanged(pub RoomView);
impl Handler<IndexChanged> for LobbyManager {
    type Result = ();
    fn handle(&mut self, m: IndexChanged, _: &mut Context<Self>) {
        let room = m.0;
        if self
            .versions
            .get(&room.id)
            .is_none_or(|v| *v <= room.version)
        {
            self.versions.insert(room.id, room.version);
            self.occupancy.retain(|_, id| *id != room.id);
            for member in &room.members {
                self.occupancy.insert(member.user, room.id);
            }
        }
    }
}
#[derive(Message)]
#[rtype(result = "Result<Payload,E>")]
pub struct Request {
    pub permit: Permit,
    pub online: HashMap<Uuid, Permit>,
    pub request_id: String,
    pub request: v1::LobbyRequest,
}
impl Handler<Request> for LobbyManager {
    type Result = ResponseFuture<Result<Payload, E>>;
    fn handle(&mut self, m: Request, ctx: &mut Context<Self>) -> Self::Result {
        if !self.ready {
            return Box::pin(async { Err(E::Unavailable) });
        }
        if !m.permit.live() {
            return Box::pin(async { Err(E::Permission) });
        }
        let config = self
            .config
            .as_ref()
            .expect("ready has configuration")
            .clone();
        let command = m.request.command.clone();
        if let Some(Command::List(list)) = command {
            return Box::pin(async move {
                let cursor = if list.cursor.is_empty() {
                    None
                } else {
                    Some(Uuid::parse_str(&list.cursor).map_err(|_| E::InvalidInput)?)
                };
                let (rooms, next) = config.database.friend_page(cursor, list.limit).await?;
                if !m.permit.live() {
                    return Err(E::Permission);
                }
                Ok(Payload::Rooms(v1::RoomList {
                    rooms: rooms.iter().map(|r| wire::room(r, &m.online)).collect(),
                    next_cursor: next.map(|id| id.to_string()).unwrap_or_default(),
                }))
            });
        }
        if matches!(command, Some(Command::Get(_))) {
            let own = self.occupancy.get(&m.permit.user).copied();
            return Box::pin(async move {
                let target = if m.request.room_id.is_empty() {
                    own.ok_or(E::NotFound)?
                } else {
                    Uuid::parse_str(&m.request.room_id).map_err(|_| E::InvalidInput)?
                };
                let outcome = config.database.friend_get(target, m.permit.user).await?;
                if !m.permit.live() {
                    return Err(E::Permission);
                }
                config
                    .registry
                    .send(Publish(outcome.clone()))
                    .await
                    .map_err(|_| E::Unavailable)?;
                Ok(Payload::Room(wire::room(&outcome.room, &m.online)))
            });
        }
        self.submit(Input::Lobby(m), config, ctx)
    }
}

enum Input {
    Lobby(Request),
    Game(GameRequest),
}
#[derive(Message)]
#[rtype(result = "Result<Payload,E>")]
pub struct GameRequest {
    pub permit: Permit,
    pub request_id: String,
    pub request: v1::GameRequest,
    pub presence: crate::persistence::recovery::Presence,
}
impl Handler<GameRequest> for LobbyManager {
    type Result = ResponseFuture<Result<Payload, E>>;
    fn handle(&mut self, m: GameRequest, ctx: &mut Context<Self>) -> Self::Result {
        if !self.ready {
            return Box::pin(async { Err(E::Unavailable) });
        }
        if !m.permit.live() {
            return Box::pin(async { Err(E::Permission) });
        }
        let config = self.config.as_ref().expect("ready configuration").clone();
        self.submit(Input::Game(m), config, ctx)
    }
}
impl LobbyManager {
    fn submit(
        &mut self,
        input: Input,
        config: Configure,
        ctx: &mut Context<Self>,
    ) -> ResponseFuture<Result<Payload, E>> {
        let (user, online) = match &input {
            Input::Lobby(m) => (m.permit.user, m.online.clone()),
            Input::Game(m) => (m.permit.user, m.presence.online.clone()),
        };
        if self.pending.contains_key(&user) || self.pending.len() >= 128 {
            return Box::pin(async { Err(E::PlayerBusy) });
        }
        let operation = Uuid::new_v4();
        self.pending.insert(user, operation);
        let (reply, receive) = oneshot::channel();
        ctx.spawn(
            async move {
                let prepared = match input {
                    Input::Lobby(m) => prepare(&config, m, operation).await,
                    Input::Game(m) => prepare_game(&config, m, operation).await,
                };
                (config, prepared)
            }
            .into_actor(self)
            .map(move |(config, prepared), actor, ctx| {
                let mutation = match prepared {
                    Ok(m) => m,
                    Err(e) => {
                        actor.pending.remove(&user);
                        let _ = reply.send(Err(e));
                        return;
                    }
                };
                let is_game = matches!(mutation.action, Mutation::Game { .. });
                let address = actor
                    .rooms
                    .entry(mutation.room)
                    .and_modify(|address| {
                        if !address.connected() {
                            *address =
                                room::Room::new(mutation.room, config.clone(), ctx.address())
                                    .start();
                        }
                    })
                    .or_insert_with(|| {
                        room::Room::new(mutation.room, config.clone(), ctx.address()).start()
                    })
                    .clone();
                // No timeout releases this reservation. The Room resolves uncertain commits.
                let recovery = config.database.clone();
                let recovery_registry = config.registry.clone();
                ctx.spawn(
                    async move {
                        match address.send(room::Execute(mutation.clone())).await {
                            Ok(result) => result,
                            Err(_) => loop {
                                // A closed mailbox is not proof of rollback.
                                match recovery.friend_recover(mutation.clone()).await {
                                    Ok(value) => match value.into_inner() {
                                        None => break Err(E::Unavailable),
                                        Some(original) => {
                                            if let Ok(mut current) =
                                                recovery.friend_current(mutation.room).await
                                            {
                                                current
                                                    .recipients
                                                    .extend(original.recipients.iter().copied());
                                                current.recipients.sort();
                                                current.recipients.dedup();
                                                let _ = recovery_registry
                                                    .send(Publish(current.clone()))
                                                    .await;
                                                break Ok(room::Resolved {
                                                    original,
                                                    current: current.room,
                                                });
                                            }
                                        }
                                    },
                                    Err(E::RequestIdConflict) => break Err(E::RequestIdConflict),
                                    Err(_) => {}
                                }
                                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                            },
                        }
                    }
                    .into_actor(actor)
                    .map(move |result, actor, _| {
                        if actor.pending.get(&user) == Some(&operation) {
                            actor.pending.remove(&user);
                        }
                        if let Ok(outcome) = &result {
                            let room = &outcome.current;
                            if actor
                                .versions
                                .get(&room.id)
                                .is_none_or(|v| *v <= room.version)
                            {
                                actor.versions.insert(room.id, room.version);
                                actor.occupancy.retain(|_, r| *r != room.id);
                                for member in &room.members {
                                    actor.occupancy.insert(member.user, room.id);
                                }
                            }
                        }
                        let _ = reply.send(result.map(|o| {
                            if is_game {
                                Payload::Acknowledged(v1::Acknowledged {
                                    room_version: o.original.room.version as u64,
                                    game_version: o
                                        .original
                                        .game
                                        .as_ref()
                                        .map(|g| g.version as u64)
                                        .unwrap_or(0),
                                })
                            } else {
                                Payload::Room(wire::room(&o.original.room, &online))
                            }
                        }));
                    }),
                );
            }),
        );
        Box::pin(async move { receive.await.unwrap_or(Err(E::Unavailable)) })
    }
}
async fn prepare_game(
    config: &Configure,
    m: GameRequest,
    operation: Uuid,
) -> Result<RoomMutation, E> {
    let expected_version =
        i64::try_from(m.request.expected_version).map_err(|_| E::InvalidInput)?;
    if expected_version == i64::MAX {
        return Err(E::InvalidInput);
    }
    let id = Uuid::parse_str(&m.request.game_id).map_err(|_| E::InvalidInput)?;
    let room = config.database.game_room(id).await?;
    Ok(RoomMutation {
        operation,
        request_id: m.request_id,
        room,
        expected_version,
        fingerprint: crate::persistence::gameplay::fingerprint(m.permit.user, &m.request),
        permit: m.permit,
        online: m.presence.online.clone(),
        action: Mutation::Game {
            request: m.request,
            presence: m.presence,
        },
    })
}
async fn prepare(config: &Configure, m: Request, operation: Uuid) -> Result<RoomMutation, E> {
    if !m.permit.live() {
        return Err(E::Permission);
    }
    let mut expected = i64::try_from(m.request.expected_version).map_err(|_| E::InvalidInput)?;
    if expected == i64::MAX {
        return Err(E::InvalidInput);
    }
    let mut mac =
        Hmac::<Sha256>::new_from_slice(&config.fingerprint_key).map_err(|_| E::Unavailable)?;
    mac.update(b"friend-command-v1");
    mac.update(m.permit.user.as_bytes());
    mac.update(&m.request.encode_to_vec());
    let fingerprint = mac.finalize().into_bytes().to_vec();
    let command = m.request.command.ok_or(E::InvalidInput)?;
    let (room, action) = match command {
        Command::Create(c) => {
            if !m.request.room_id.is_empty()
                || expected != 0
                || c.mode != "casual"
                || !valid_rules(&c.rules_version)
            {
                return Err(E::InvalidInput);
            }
            let mut key = Hmac::<Sha256>::new_from_slice(&config.fingerprint_key)
                .map_err(|_| E::Unavailable)?;
            key.update(b"friend-room-id-v1");
            key.update(m.permit.user.as_bytes());
            key.update(m.request_id.as_bytes());
            let bytes = key.finalize().into_bytes();
            let room = Uuid::from_slice(&bytes[..16]).map_err(|_| E::Unavailable)?;
            const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
            let code: String = bytes[16..26]
                .iter()
                .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
                .collect();
            let password_hash = match c.password {
                Some(p) if !p.is_empty() => Some(password_hash(config, p).await?),
                _ => None,
            };
            (
                room,
                Mutation::Create {
                    code,
                    mode: c.mode,
                    rules: c.rules_version,
                    password_hash,
                },
            )
        }
        Command::Join(j) => {
            let by_code = if j.code.is_empty() {
                None
            } else {
                let code = j.code.to_ascii_uppercase();
                if !(6..=10).contains(&code.len())
                    || !code.bytes().all(|b| b.is_ascii_alphanumeric())
                {
                    return Err(E::InvalidInput);
                }
                Some(config.database.friend_target(&code).await?)
            };
            let by_id = if m.request.room_id.is_empty() {
                None
            } else {
                Some(Uuid::parse_str(&m.request.room_id).map_err(|_| E::InvalidInput)?)
            };
            if by_code.is_some() && by_id.is_some() && by_code != by_id {
                return Err(E::InvalidInput);
            }
            let room = by_code.or(by_id).ok_or(E::InvalidInput)?;
            if by_id.is_none() {
                if expected != 0 {
                    return Err(E::InvalidInput);
                }
                expected = config.database.friend_room(room).await?.version;
            }
            let verified_hash = config.database.friend_password(room).await?;
            if let Some(hash) = &verified_hash {
                password_verify(config, j.password.unwrap_or_default(), hash.clone()).await?;
            }
            (room, Mutation::Join { verified_hash })
        }
        command => {
            let room = Uuid::parse_str(&m.request.room_id).map_err(|_| E::InvalidInput)?;
            let action = match command {
                Command::Leave(_) => Mutation::Leave,
                Command::SetReady(r) => Mutation::Ready(r.ready),
                Command::SetRules(r) if valid_rules(&r.rules_version) => {
                    Mutation::Rules(r.rules_version)
                }
                Command::Start(_) => Mutation::Start {
                    game: Uuid::new_v4(),
                    first_player: rand::random_range(0..2),
                },
                _ => return Err(E::InvalidInput),
            };
            (room, action)
        }
    };
    Ok(RoomMutation {
        operation,
        request_id: m.request_id,
        room,
        expected_version: expected,
        fingerprint,
        permit: m.permit,
        online: m.online,
        action,
    })
}
fn password_length(password: &str) -> Result<(), E> {
    if password.is_empty() || password.len() > 128 {
        Err(E::InvalidInput)
    } else {
        Ok(())
    }
}
async fn password_hash(config: &Configure, password: String) -> Result<String, E> {
    password_length(&password)?;
    let worker = config
        .password_workers
        .clone()
        .try_acquire_owned()
        .map_err(|_| E::Unavailable)?;
    tokio::task::spawn_blocking(move || {
        let _worker = worker;
        let salt =
            SaltString::encode_b64(&rand::random::<[u8; 16]>()).map_err(|_| E::Unavailable)?;
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|_| E::Unavailable)
    })
    .await
    .map_err(|_| E::Unavailable)?
}
async fn password_verify(config: &Configure, password: String, hash: String) -> Result<(), E> {
    password_length(&password).map_err(|_| E::BadPassword)?;
    let worker = config
        .password_workers
        .clone()
        .try_acquire_owned()
        .map_err(|_| E::Unavailable)?;
    tokio::task::spawn_blocking(move || {
        let _worker = worker;
        let hash = PasswordHash::new(&hash).map_err(|_| E::Unavailable)?;
        Argon2::default()
            .verify_password(password.as_bytes(), &hash)
            .map_err(|_| E::BadPassword)
    })
    .await
    .map_err(|_| E::Unavailable)?
}
