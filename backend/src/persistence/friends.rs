//! Friend-room transactions. No Actor addresses or network side effects.
use super::{
    Database, StoreError as E,
    transaction::{Committed, TxError},
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use uuid::Uuid;

/// A revocable in-process lease plus the durable database generation.
#[derive(Clone)]
pub struct Permit {
    pub user: Uuid,
    pub session: Uuid,
    pub generation: u64,
    pub expires: i64,
    pub current: Arc<AtomicBool>,
}
impl Permit {
    pub fn live(&self) -> bool {
        self.current.load(Ordering::SeqCst) && self.expires > Utc::now().timestamp()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Member {
    pub user: Uuid,
    pub seat: u32,
    pub ready: bool,
    pub nickname: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RoomView {
    pub id: Uuid,
    pub version: i64,
    pub phase: String,
    pub owner: Option<Uuid>,
    pub code: String,
    pub mode: String,
    pub rules: String,
    pub protected: bool,
    pub members: Vec<Member>,
    pub game: Option<Uuid>,
    pub first_player: Option<u32>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GameView {
    pub id: Uuid,
    pub rules: String,
    pub version: i64,
    pub seq: i64,
    pub state: Value,
    #[serde(default)]
    pub phase: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Outcome {
    pub room: RoomView,
    pub game: Option<GameView>,
    pub recipients: Vec<Uuid>,
}
// Password material intentionally has no Debug or Serialize implementation.
#[derive(Clone)]
pub enum Mutation {
    Game {
        request: util_lib::protocol::v1::GameRequest,
        presence: super::recovery::Presence,
    },
    Create {
        code: String,
        mode: String,
        rules: String,
        password_hash: Option<String>,
    },
    Join {
        verified_hash: Option<String>,
    },
    Leave,
    Ready(bool),
    Rules(String),
    Start {
        game: Uuid,
        first_player: u32,
    },
}
#[derive(Clone)]
pub struct RoomMutation {
    pub operation: Uuid,
    pub request_id: String,
    pub room: Uuid,
    pub expected_version: i64,
    pub fingerprint: Vec<u8>,
    pub permit: Permit,
    pub online: HashMap<Uuid, Permit>,
    pub action: Mutation,
}
fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
pub fn valid_rules(value: &str) -> bool {
    game_core::rules::registry::resolve(value).is_ok()
}
pub(super) fn require_startable_rules(value: &str) -> Result<(), E> {
    use game_core::rules::registry::{RegistryError, require_startable};
    require_startable(value)
        .map(|_| ())
        .map_err(|error| match error {
            RegistryError::EngineNotReady => E::RulesNotImplemented,
            _ => E::InvalidInput,
        })
}
pub(super) async fn read_room(c: &mut PgConnection, id: Uuid) -> Result<RoomView, TxError> {
    let r = sqlx::query("SELECT r.*,g.game_id,g.snapshot->'first_player_seat' AS first_player FROM patchwork.rooms r LEFT JOIN patchwork.games g USING(room_id) WHERE r.room_id=$1 ORDER BY g.created_at DESC LIMIT 1")
        .bind(id).fetch_optional(&mut *c).await?.ok_or(E::NotFound)?;
    let members = sqlx::query("SELECT m.user_id,m.seat,m.ready,u.nickname FROM patchwork.room_members m JOIN patchwork.users u USING(user_id) WHERE room_id=$1 ORDER BY seat")
        .bind(id).fetch_all(c).await?.into_iter().map(|r| Member {
            user:r.get("user_id"), seat:r.get::<i16,_>("seat") as u32,
            ready:r.get("ready"), nickname:r.get("nickname"),
        }).collect();
    Ok(RoomView {
        id,
        version: r.get("version"),
        phase: r.get("phase"),
        owner: r.get("owner_id"),
        code: r.get("code"),
        mode: r.get("mode"),
        rules: r.get("rules_version"),
        protected: r.get::<Option<String>, _>("password_hash").is_some(),
        members,
        game: r.get("game_id"),
        first_player: r
            .get::<Option<Value>, _>("first_player")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
    })
}
pub(super) async fn read_game(
    c: &mut PgConnection,
    id: Option<Uuid>,
) -> Result<Option<GameView>, TxError> {
    let Some(id) = id else { return Ok(None) };
    let r = sqlx::query("SELECT rules_version,state_version,event_seq,snapshot,phase,player0,player1 FROM patchwork.games WHERE game_id=$1")
        .bind(id).fetch_one(c).await?;
    let game = GameView {
        id,
        rules: r.get("rules_version"),
        version: r.get("state_version"),
        seq: r.get("event_seq"),
        state: r.get("snapshot"),
        phase: r.get("phase"),
    };
    if game.rules == game_core::rules::CUSTOM_RULES_VERSION {
        let state = super::gameplay::decode(&game)?;
        for (seat, column) in [
            (game_core::Seat::First, "player0"),
            (game_core::Seat::Second, "player1"),
        ] {
            if state.player(seat).user_id() != r.get::<Uuid, _>(column).to_string() {
                return Err(E::Unavailable.into());
            }
        }
    }
    Ok(Some(game))
}
async fn receipt(c: &mut PgConnection, m: &RoomMutation) -> Result<Option<Outcome>, TxError> {
    let r = sqlx::query("SELECT payload_hash,response FROM patchwork.operation_receipts WHERE user_id=$1 AND request_id=$2")
        .bind(m.permit.user).bind(&m.request_id).fetch_optional(c).await?;
    r.map(|r| {
        if r.get::<Vec<u8>, _>("payload_hash") != m.fingerprint {
            return Err(E::RequestIdConflict.into());
        }
        serde_json::from_value(r.get("response")).map_err(|_| E::Unavailable.into())
    })
    .transpose()
}
pub(super) async fn fence(c: &mut PgConnection, p: &Permit) -> Result<(), TxError> {
    if !p.live() {
        return Err(E::Permission.into());
    }
    let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM patchwork.users u JOIN patchwork.sessions s USING(user_id) WHERE u.user_id=$1 AND s.session_id=$2 AND u.connection_generation=$3 AND s.revoked_at IS NULL AND s.expires_at>now())")
        .bind(p.user).bind(p.session).bind(i64::try_from(p.generation).map_err(|_|E::Permission)?)
        .fetch_one(c).await?;
    if !valid || !p.live() {
        return Err(E::Permission.into());
    }
    Ok(())
}
async fn locks(c: &mut PgConnection, m: &RoomMutation) -> Result<(), TxError> {
    // Snapshot participants first; Room serializes membership. Recheck after locking.
    let mut users: Vec<Uuid> =
        sqlx::query_scalar("SELECT user_id FROM patchwork.room_members WHERE room_id=$1")
            .bind(m.room)
            .fetch_all(&mut *c)
            .await?;
    users.push(m.permit.user);
    users.sort();
    users.dedup();
    for user in &users {
        sqlx::query("SELECT user_id FROM patchwork.users WHERE user_id=$1 FOR UPDATE")
            .bind(user)
            .fetch_optional(&mut *c)
            .await?
            .ok_or(E::NotFound)?;
    }
    for user in &users {
        sqlx::query("SELECT user_id FROM patchwork.player_occupancy WHERE user_id=$1 FOR UPDATE")
            .bind(user)
            .fetch_optional(&mut *c)
            .await?
            .ok_or(E::NotFound)?;
    }
    sqlx::query("SELECT room_id FROM patchwork.rooms WHERE room_id=$1 FOR UPDATE")
        .bind(m.room)
        .fetch_optional(&mut *c)
        .await?;
    let current: Vec<Uuid> =
        sqlx::query_scalar("SELECT user_id FROM patchwork.room_members WHERE room_id=$1")
            .bind(m.room)
            .fetch_all(c)
            .await?;
    if current.iter().any(|u| !users.contains(u)) {
        return Err(E::VersionConflict.into());
    }
    Ok(())
}
async fn idle(c: &mut PgConnection, user: Uuid) -> Result<(), TxError> {
    let state: String =
        sqlx::query_scalar("SELECT state FROM patchwork.player_occupancy WHERE user_id=$1")
            .bind(user)
            .fetch_one(c)
            .await?;
    if state != "idle" {
        return Err(E::PlayerBusy.into());
    }
    Ok(())
}
impl Database {
    pub async fn friend_current(&self, id: Uuid) -> Result<Outcome, E> {
        self.transaction(move |c| {
            Box::pin(async move {
                sqlx::query("SELECT room_id FROM patchwork.rooms WHERE room_id=$1 FOR SHARE")
                    .bind(id)
                    .fetch_optional(&mut *c)
                    .await?
                    .ok_or(E::NotFound)?;
                let room = read_room(c, id).await?;
                let game = read_game(c, room.game).await?;
                let recipients = room.members.iter().map(|m| m.user).collect();
                Ok(Outcome {
                    room,
                    game,
                    recipients,
                })
            })
        })
        .await
        .map(Committed::into_inner)
    }
    pub async fn friend_index(&self) -> Result<Vec<(Uuid, Uuid)>, E> {
        sqlx::query_as("SELECT user_id,room_id FROM patchwork.player_occupancy WHERE state='room'")
            .fetch_all(self.pool())
            .await
            .map_err(|_| E::Unavailable)
    }
    pub async fn friend_target(&self, code: &str) -> Result<Uuid, E> {
        sqlx::query_scalar("SELECT room_id FROM patchwork.rooms WHERE code=$1")
            .bind(code)
            .fetch_optional(self.pool())
            .await
            .map_err(|_| E::Unavailable)?
            .ok_or(E::NotFound)
    }
    pub async fn friend_password(&self, room: Uuid) -> Result<Option<String>, E> {
        sqlx::query_scalar("SELECT password_hash FROM patchwork.rooms WHERE room_id=$1")
            .bind(room)
            .fetch_optional(self.pool())
            .await
            .map_err(|_| E::Unavailable)?
            .ok_or(E::NotFound)
    }
    pub async fn friend_room(&self, room: Uuid) -> Result<RoomView, E> {
        self.transaction(move |c| {
            Box::pin(async move {
                // A room row lock makes members and room version a coherent snapshot.
                sqlx::query("SELECT room_id FROM patchwork.rooms WHERE room_id=$1 FOR SHARE")
                    .bind(room)
                    .fetch_optional(&mut *c)
                    .await?
                    .ok_or(E::NotFound)?;
                read_room(c, room).await
            })
        })
        .await
        .map(Committed::into_inner)
    }
    pub async fn friend_get(&self, room: Uuid, user: Uuid) -> Result<Outcome, E> {
        self.transaction(move |c| {
            Box::pin(async move {
                sqlx::query("SELECT room_id FROM patchwork.rooms WHERE room_id=$1 FOR SHARE")
                    .bind(room)
                    .fetch_optional(&mut *c)
                    .await?
                    .ok_or(E::NotFound)?;
                let room = read_room(c, room).await?;
                if !room.members.iter().any(|m| m.user == user) {
                    return Err(E::Permission.into());
                }
                let game = read_game(c, room.game).await?;
                Ok(Outcome {
                    room,
                    game,
                    recipients: vec![user],
                })
            })
        })
        .await
        .map(Committed::into_inner)
    }
    pub async fn friend_page(
        &self,
        cursor: Option<Uuid>,
        limit: u32,
    ) -> Result<(Vec<RoomView>, Option<Uuid>), E> {
        if !(1..=50).contains(&limit) {
            return Err(E::InvalidInput);
        }
        self.transaction(move |c| Box::pin(async move {
            let ids: Vec<Uuid> = sqlx::query_scalar("SELECT room_id FROM patchwork.rooms WHERE phase='waiting' AND ($1::uuid IS NULL OR room_id>$1) ORDER BY room_id LIMIT $2 FOR SHARE")
                .bind(cursor).bind(i64::from(limit)+1).fetch_all(&mut *c).await?;
            let next = if ids.len()>limit as usize {Some(ids[limit as usize-1])} else {None};
            let mut rooms=Vec::new();
            for id in ids.into_iter().take(limit as usize) {rooms.push(read_room(c,id).await?);}
            Ok((rooms,next))
        })).await.map(Committed::into_inner)
    }
    pub async fn friend_recover(&self, m: RoomMutation) -> Result<Committed<Option<Outcome>>, E> {
        self.transaction(move |c| {
            let m = m.clone();
            Box::pin(async move {
                locks(c, &m).await?;
                if let Mutation::Game { request, .. } = &m.action {
                    return super::gameplay::receipt(c, &m, request).await;
                }
                receipt(c, &m).await
            })
        })
        .await
    }
    pub async fn friend_mutate(&self, m: RoomMutation) -> Result<Committed<Outcome>, E> {
        if !valid_id(&m.request_id)
            || m.fingerprint.len() != 32
            || m.expected_version < 0
            || m.expected_version == i64::MAX
        {
            return Err(E::InvalidInput);
        }
        self.transaction(move |c| {let m=m.clone(); Box::pin(async move {
            locks(c,&m).await?;
            fence(c,&m.permit).await?;
            if let Mutation::Game {request,presence} = &m.action { return super::gameplay::execute(c,&m,request,presence).await; }
            if let Some(outcome)=receipt(c,&m).await? {return Ok(outcome);}
            let user=m.permit.user;
            let mut recipients=vec![user];
            if let Mutation::Create {code,mode,rules,password_hash}=&m.action {
                if mode!="casual" || !valid_rules(rules) || m.expected_version!=0 {return Err(E::InvalidInput.into());}
                idle(c,user).await?;
                sqlx::query("INSERT INTO patchwork.rooms(room_id,code,mode,rules_version,owner_id,password_hash) VALUES($1,$2,$3,$4,$5,$6)")
                    .bind(m.room).bind(code).bind(mode).bind(rules).bind(user).bind(password_hash).execute(&mut *c).await?;
                sqlx::query("INSERT INTO patchwork.room_members(room_id,seat,user_id,connection_generation) VALUES($1,0,$2,$3)")
                    .bind(m.room).bind(user).bind(m.permit.generation as i64).execute(&mut *c).await?;
                sqlx::query("UPDATE patchwork.player_occupancy SET state='room',room_id=$2 WHERE user_id=$1")
                    .bind(user).bind(m.room).execute(&mut *c).await?;
            } else {
                let room=read_room(c,m.room).await?;
                recipients.extend(room.members.iter().map(|m|m.user));
                if room.version!=m.expected_version {return Err(E::VersionConflict.into());}
                let member=room.members.iter().find(|m|m.user==user);
                if !matches!(m.action,Mutation::Join{..}) && member.is_none() {return Err(E::Permission.into());}
                match &m.action {
                    Mutation::Join {verified_hash} => {
                        if room.phase!="waiting" {return Err(E::RoomNotJoinable.into());}
                        idle(c,user).await?;
                        if room.members.len()>=2 {return Err(E::RoomFull.into());}
                        let current: Option<String>=sqlx::query_scalar("SELECT password_hash FROM patchwork.rooms WHERE room_id=$1")
                            .bind(m.room).fetch_one(&mut *c).await?;
                        if current!=*verified_hash {return Err(E::BadPassword.into());}
                        let seat=(0..2u32).find(|s|!room.members.iter().any(|m|m.seat==*s)).ok_or(E::RoomFull)?;
                        sqlx::query("INSERT INTO patchwork.room_members(room_id,seat,user_id,connection_generation) VALUES($1,$2,$3,$4)")
                            .bind(m.room).bind(seat as i16).bind(user).bind(m.permit.generation as i64).execute(&mut *c).await?;
                        sqlx::query("UPDATE patchwork.player_occupancy SET state='room',room_id=$2 WHERE user_id=$1")
                            .bind(user).bind(m.room).execute(&mut *c).await?;
                        reset_ready(c,m.room).await?;
                    }
                    Mutation::Leave => {
                        if !matches!(room.phase.as_str(),"waiting"|"finished") {return Err(E::RoomNotJoinable.into());}
                        sqlx::query("UPDATE patchwork.player_occupancy SET state='idle',room_id=NULL WHERE user_id=$1")
                            .bind(user).execute(&mut *c).await?;
                        sqlx::query("DELETE FROM patchwork.room_members WHERE room_id=$1 AND user_id=$2")
                            .bind(m.room).bind(user).execute(&mut *c).await?;
                        // One remaining member at most; stable seat never changes on transfer.
                        let next=room.members.iter().find(|m|m.user!=user).map(|m|m.user);
                        let owner=if room.owner==Some(user) {next} else {room.owner};
                        sqlx::query("UPDATE patchwork.rooms SET owner_id=$2,phase=CASE WHEN $3 THEN 'closed' ELSE phase END WHERE room_id=$1")
                            .bind(m.room).bind(owner).bind(next.is_none()).execute(&mut *c).await?;
                        reset_ready(c,m.room).await?;
                    }
                    Mutation::Ready(ready) => {
                        if room.phase!="waiting" {return Err(E::RoomNotJoinable.into());}
                        sqlx::query("UPDATE patchwork.room_members SET ready=$3,connection_generation=$4 WHERE room_id=$1 AND user_id=$2")
                            .bind(m.room).bind(user).bind(ready).bind(m.permit.generation as i64).execute(&mut *c).await?;
                    }
                    Mutation::Rules(rules) => {
                        if room.owner!=Some(user) {return Err(E::Permission.into());}
                        if room.phase!="waiting" {return Err(E::RoomNotJoinable.into());}
                        if !valid_rules(rules) {return Err(E::InvalidInput.into());}
                        if *rules!=room.rules {
                            sqlx::query("UPDATE patchwork.rooms SET rules_version=$2 WHERE room_id=$1")
                                .bind(m.room).bind(rules).execute(&mut *c).await?;
                            reset_ready(c,m.room).await?;
                        }
                    }
                    Mutation::Start {game,first_player} => {
                        if room.owner!=Some(user) {return Err(E::Permission.into());}
                        if room.phase!="waiting" {return Err(E::RoomNotJoinable.into());}
                        require_startable_rules(&room.rules)?;
                        let [a,b]: [&Member;2]=room.members.iter().collect::<Vec<_>>().try_into().map_err(|_|E::NotEnoughPlayers)?;
                        if a.user==b.user || a.seat!=0 || b.seat!=1 {return Err(E::NotEnoughPlayers.into());}
                        if !a.ready || !b.ready {return Err(E::NotReady.into());}
                        if *first_player>1 {return Err(E::InvalidInput.into());}
                        for member in [a,b] {
                            let permit=m.online.get(&member.user).ok_or(E::NotReady)?;
                            fence(c,permit).await.map_err(|_|TxError::from(E::NotReady))?;
                        }
                        // Starting and Playing are one committed transition: no half-created game.
                        sqlx::query("UPDATE patchwork.rooms SET phase='starting' WHERE room_id=$1")
                            .bind(m.room).execute(&mut *c).await?;
                        let snapshot=if room.rules == game_core::rules::CUSTOM_RULES_VERSION {
                            use rand::seq::SliceRandom;
                            let mut order: Vec<_> = game_core::rules::PATCHES.iter().map(|p|p.id).collect();
                            order.shuffle(&mut rand::rng());
                            let initial = game_core::state::GameSnapshot::new(game.to_string(),[a.user.to_string(),b.user.to_string()],game_core::Seat::try_from(*first_player).map_err(|_|E::InvalidInput)?,order).map_err(|_|E::Unavailable)?;
                            serde_json::to_value(initial).map_err(|_|E::Unavailable)?
                        } else {json!({"kind":game_core::rules::registry::LEGACY_SNAPSHOT_KIND,"game_id":game,"rules_version":room.rules,
                            "players":[{"user_id":a.user,"seat":a.seat},{"user_id":b.user,"seat":b.seat}],
                            "first_player_seat":first_player,"rules_implemented":false})};
                        sqlx::query("INSERT INTO patchwork.games(game_id,room_id,player0,player1,phase,rules_version,snapshot) VALUES($1,$2,$3,$4,'playing',$5,$6)")
                            .bind(game).bind(m.room).bind(a.user).bind(b.user).bind(&room.rules).bind(snapshot).execute(&mut *c).await?;
                        sqlx::query("UPDATE patchwork.rooms SET phase='playing' WHERE room_id=$1").bind(m.room).execute(&mut *c).await?;
                    }
                    Mutation::Create{..} | Mutation::Game{..} => unreachable!(),
                }
                sqlx::query("UPDATE patchwork.rooms SET version=version+1,updated_at=now() WHERE room_id=$1 AND version=$2")
                    .bind(m.room).bind(m.expected_version).execute(&mut *c).await?;
            }
            // Last in-process fence before commit; row locks serialize durable takeovers.
            if !m.permit.live() {return Err(E::Permission.into());}
            let room=read_room(c,m.room).await?;
            recipients.extend(room.members.iter().map(|m|m.user)); recipients.sort(); recipients.dedup();
            let game=if matches!(m.action,Mutation::Start{..}) {read_game(c,room.game).await?} else {None};
            let outcome=Outcome {room,game,recipients};
            sqlx::query("INSERT INTO patchwork.operation_receipts(operation_id,user_id,request_id,payload_hash,response) VALUES($1,$2,$3,$4,$5)")
                .bind(m.operation).bind(user).bind(&m.request_id).bind(&m.fingerprint)
                .bind(serde_json::to_value(&outcome).map_err(|_|E::InvalidInput)?).execute(c).await?;
            Ok(outcome)
        })}).await
    }
}
async fn reset_ready(c: &mut PgConnection, room: Uuid) -> Result<(), TxError> {
    sqlx::query("UPDATE patchwork.room_members SET ready=false WHERE room_id=$1")
        .bind(room)
        .execute(c)
        .await?;
    Ok(())
}
