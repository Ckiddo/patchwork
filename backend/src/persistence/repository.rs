//! Trusted server-side storage operations. Game rules and authorization stay in handlers.
use super::{
    Database, StoreError as E,
    transaction::{Committed, TxError},
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

#[derive(Clone, Serialize)]
pub enum Operation {
    CreateRoom {
        room: Uuid,
        code: String,
        mode: String,
        rules: String,
    },
    JoinRoom {
        room: Uuid,
        expected_version: i64,
    },
    Queue {
        ticket: Uuid,
        mode: String,
        rules: String,
        generation: i64,
        expires: DateTime<Utc>,
    },
    Pair {
        users: [Uuid; 2],
        tickets: [Uuid; 2],
        room: Uuid,
        code: String,
    },
}
#[derive(Clone)]
pub struct OperationRequest {
    pub operation_id: Uuid,
    pub user: Uuid,
    pub request_id: String,
    pub operation: Operation,
}
fn hash<T: Serialize>(value: &T) -> Result<Vec<u8>, E> {
    Ok(Sha256::digest(serde_json::to_vec(value).map_err(|_| E::InvalidInput)?).to_vec())
}
fn request_id(id: &str) -> Result<(), E> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(E::InvalidInput);
    }
    Ok(())
}
async fn lock_users(c: &mut PgConnection, users: &[Uuid]) -> Result<(), TxError> {
    let mut users = users.to_vec();
    users.sort();
    users.dedup();
    for user in users {
        sqlx::query(
            "INSERT INTO patchwork.player_occupancy(user_id) VALUES($1) ON CONFLICT DO NOTHING",
        )
        .bind(user)
        .execute(&mut *c)
        .await?;
        sqlx::query("SELECT user_id FROM patchwork.player_occupancy WHERE user_id=$1 FOR UPDATE")
            .bind(user)
            .fetch_one(&mut *c)
            .await?;
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
        return Err(E::Conflict.into());
    }
    Ok(())
}
async fn receipt(
    c: &mut PgConnection,
    user: Uuid,
    id: &str,
    fingerprint: &[u8],
) -> Result<Option<Value>, TxError> {
    let row = sqlx::query("SELECT payload_hash,response FROM patchwork.operation_receipts WHERE user_id=$1 AND request_id=$2")
        .bind(user).bind(id).fetch_optional(c).await?;
    row.map(|r| {
        if r.get::<Vec<u8>, _>("payload_hash") != fingerprint {
            return Err(E::RequestIdConflict.into());
        }
        Ok(r.get("response"))
    })
    .transpose()
}
async fn make_room(
    c: &mut PgConnection,
    room: Uuid,
    code: &str,
    mode: &str,
    rules: &str,
    users: &[Uuid],
) -> Result<(), TxError> {
    if users.is_empty() || users.len() > 2 || !super::friends::valid_rules(rules) {
        return Err(E::InvalidInput.into());
    }
    sqlx::query("INSERT INTO patchwork.rooms(room_id,code,mode,rules_version,owner_id) VALUES($1,$2,$3,$4,$5)")
        .bind(room).bind(code).bind(mode).bind(rules).bind(users[0]).execute(&mut *c).await?;
    for (seat, user) in users.iter().enumerate() {
        sqlx::query("INSERT INTO patchwork.room_members(room_id,seat,user_id) VALUES($1,$2,$3)")
            .bind(room)
            .bind(seat as i16)
            .bind(user)
            .execute(&mut *c)
            .await?;
        sqlx::query("UPDATE patchwork.player_occupancy SET state='room',room_id=$2,ticket_id=NULL,operation_id=NULL WHERE user_id=$1")
            .bind(user).bind(room).execute(&mut *c).await?;
    }
    Ok(())
}

impl Database {
    /// Existing profiles win during later controlled legacy identity import.
    pub async fn ensure_user(&self, id: Uuid, nickname: String) -> Result<Committed<()>, E> {
        self.transaction(move |c| { let nickname=nickname.clone(); Box::pin(async move {
            sqlx::query("INSERT INTO patchwork.users(user_id,nickname) VALUES($1,$2) ON CONFLICT DO NOTHING")
                .bind(id).bind(nickname).execute(&mut *c).await?;
            sqlx::query("INSERT INTO patchwork.player_occupancy(user_id) VALUES($1) ON CONFLICT DO NOTHING")
                .bind(id).execute(c).await?;
            Ok(())
        }) }).await
    }

    pub async fn operate(&self, req: OperationRequest) -> Result<Committed<Value>, E> {
        request_id(&req.request_id)?;
        let fingerprint = hash(&req.operation)?;
        self.transaction(move |c| { let req=req.clone(); let fingerprint=fingerprint.clone(); Box::pin(async move {
            let mut users=vec![req.user];
            if let Operation::Pair { users: pair,.. }=&req.operation { users.extend(pair); }
            lock_users(c,&users).await?;
            if let Some(value)=receipt(c,req.user,&req.request_id,&fingerprint).await? { return Ok(value); }
            let result=match &req.operation {
                Operation::CreateRoom {room,code,mode,rules} => {
                    idle(c,req.user).await?;
                    make_room(c,*room,code,mode,rules,&[req.user]).await?;
                    json!({"room_id":room,"version":0})
                }
                Operation::JoinRoom {room,expected_version} => {
                    idle(c,req.user).await?;
                    let r=sqlx::query("SELECT phase,version FROM patchwork.rooms WHERE room_id=$1 FOR UPDATE")
                        .bind(room).fetch_optional(&mut *c).await?.ok_or(E::NotFound)?;
                    if r.get::<i64,_>("version")!=*expected_version { return Err(E::VersionConflict.into()); }
                    if r.get::<String,_>("phase")!="waiting" { return Err(E::Conflict.into()); }
                    let seats: Vec<i16>=sqlx::query_scalar("SELECT seat FROM patchwork.room_members WHERE room_id=$1 ORDER BY seat")
                        .bind(room).fetch_all(&mut *c).await?;
                    let seat=(0..2i16).find(|s| !seats.contains(s)).ok_or(E::Conflict)?;
                    sqlx::query("UPDATE patchwork.room_members SET ready=false WHERE room_id=$1").bind(room).execute(&mut *c).await?;
                    sqlx::query("INSERT INTO patchwork.room_members(room_id,seat,user_id) VALUES($1,$2,$3)")
                        .bind(room).bind(seat).bind(req.user).execute(&mut *c).await?;
                    let version: i64=sqlx::query_scalar("UPDATE patchwork.rooms SET version=version+1,updated_at=now() WHERE room_id=$1 AND version=$2 RETURNING version")
                        .bind(room).bind(expected_version).fetch_one(&mut *c).await?;
                    sqlx::query("UPDATE patchwork.player_occupancy SET state='room',room_id=$2 WHERE user_id=$1")
                        .bind(req.user).bind(room).execute(&mut *c).await?;
                    json!({"room_id":room,"version":version,"seat":seat})
                }
                Operation::Queue {ticket,mode,rules,generation,expires} => {
                    idle(c,req.user).await?;
                    sqlx::query("INSERT INTO patchwork.matchmaking_tickets(ticket_id,user_id,mode,rules_version,connection_generation,status,expires_at) VALUES($1,$2,$3,$4,$5,'queued',$6)")
                        .bind(ticket).bind(req.user).bind(mode).bind(rules).bind(generation).bind(expires).execute(&mut *c).await?;
                    sqlx::query("UPDATE patchwork.player_occupancy SET state='queue',ticket_id=$2 WHERE user_id=$1")
                        .bind(req.user).bind(ticket).execute(&mut *c).await?;
                    json!({"ticket_id":ticket})
                }
                Operation::Pair {users,tickets,room,code} => {
                    if users[0]==users[1] || tickets[0]==tickets[1] || !users.contains(&req.user) { return Err(E::InvalidInput.into()); }
                    let mut sorted=*tickets; sorted.sort();
                    for t in sorted { sqlx::query("SELECT ticket_id FROM patchwork.matchmaking_tickets WHERE ticket_id=$1 FOR UPDATE")
                        .bind(t).fetch_optional(&mut *c).await?.ok_or(E::NotFound)?; }
                    let rows=sqlx::query("SELECT t.*,o.state,o.ticket_id AS occupied_ticket FROM patchwork.matchmaking_tickets t JOIN patchwork.player_occupancy o USING(user_id) WHERE t.ticket_id=ANY($1) ORDER BY t.join_seq")
                        .bind(tickets.to_vec()).fetch_all(&mut *c).await?;
                    if rows.len()!=2 { return Err(E::Conflict.into()); }
                    for (user,ticket) in users.iter().zip(tickets) {
                        if !rows.iter().any(|r| r.get::<Uuid,_>("user_id")==*user && r.get::<Uuid,_>("ticket_id")==*ticket
                            && r.get::<String,_>("status")=="queued" && r.get::<String,_>("state")=="queue"
                            && r.get::<Option<Uuid>,_>("occupied_ticket")==Some(*ticket)
                            && r.get::<DateTime<Utc>,_>("expires_at")>Utc::now()) { return Err(E::Conflict.into()); }
                    }
                    let mode: String=rows[0].get("mode"); let rules: String=rows[0].get("rules_version");
                    if rows[1].get::<String,_>("mode")!=mode || rows[1].get::<String,_>("rules_version")!=rules { return Err(E::Conflict.into()); }
                    let earliest: Vec<Uuid>=sqlx::query_scalar("SELECT ticket_id FROM patchwork.matchmaking_tickets WHERE mode=$1 AND rules_version=$2 AND status='queued' AND expires_at>now() ORDER BY join_seq LIMIT 2")
                        .bind(&mode).bind(&rules).fetch_all(&mut *c).await?;
                    if earliest.len()!=2 || !earliest.iter().all(|t| tickets.contains(t)) { return Err(E::Conflict.into()); }
                    let seated=[rows[0].get("user_id"),rows[1].get("user_id")];
                    make_room(c,*room,code,&mode,&rules,&seated).await?;
                    sqlx::query("UPDATE patchwork.matchmaking_tickets SET status='matched',room_id=$2 WHERE ticket_id=ANY($1)")
                        .bind(tickets.to_vec()).bind(room).execute(&mut *c).await?;
                    json!({"room_id":room,"version":0})
                }
            };
            sqlx::query("INSERT INTO patchwork.operation_receipts(operation_id,user_id,request_id,payload_hash,response) VALUES($1,$2,$3,$4,$5)")
                .bind(req.operation_id).bind(req.user).bind(req.request_id).bind(fingerprint).bind(&result).execute(c).await?;
            Ok(result)
        }) }).await
    }
    /// Locks the same user as the original operation, fencing any in-flight COMMIT.
    /// None means confirmed absent; a failed lookup must keep caller reservations frozen.
    pub async fn recover_operation(
        &self,
        req: OperationRequest,
    ) -> Result<Committed<Option<Value>>, E> {
        request_id(&req.request_id)?;
        let fingerprint = hash(&req.operation)?;
        self.transaction(move |c| {
            let req = req.clone();
            let fingerprint = fingerprint.clone();
            Box::pin(async move {
                lock_users(c, &[req.user]).await?;
                receipt(c, req.user, &req.request_id, &fingerprint).await
            })
        })
        .await
    }
}

#[derive(Clone, Serialize)]
pub struct GameWrite {
    pub game: Uuid,
    pub user: Uuid,
    pub request_id: String,
    pub expected_version: i64,
    /// Canonical validated intent, including every semantic command argument.
    pub intent: Value,
    pub snapshot: Value,
    pub event: Value,
    pub result: Option<FinalResult>,
}
#[derive(Clone, Serialize)]
pub struct FinalResult {
    pub score0: i32,
    pub score1: i32,
    pub winner_seat: Option<i16>,
    pub reason: String,
}
impl Database {
    /// Stores a server-validated initial snapshot; does not implement Patchwork rules.
    pub async fn begin_game(
        &self,
        room: Uuid,
        game: Uuid,
        version: i64,
        snapshot: Value,
    ) -> Result<Committed<()>, E> {
        self.transaction(move |c| { let snapshot=snapshot.clone(); Box::pin(async move {
            let r=sqlx::query("SELECT phase,version,rules_version FROM patchwork.rooms WHERE room_id=$1 FOR UPDATE")
                .bind(room).fetch_optional(&mut *c).await?.ok_or(E::NotFound)?;
            if r.get::<i64,_>("version")!=version { return Err(E::VersionConflict.into()); }
            if r.get::<String,_>("phase")!="waiting" { return Err(E::Conflict.into()); }
            super::friends::require_startable_rules(&r.get::<String,_>("rules_version"))?;
            // Formal games must use the fenced friend-room start and generated initial state.
            if r.get::<String,_>("rules_version") == game_core::rules::CUSTOM_RULES_VERSION { return Err(E::InvalidInput.into()); }
            let members=sqlx::query("SELECT user_id,ready FROM patchwork.room_members WHERE room_id=$1 ORDER BY seat")
                .bind(room).fetch_all(&mut *c).await?;
            if members.len()!=2 || !members.iter().all(|r| r.get::<bool,_>("ready")) { return Err(E::Conflict.into()); }
            sqlx::query("INSERT INTO patchwork.games(game_id,room_id,player0,player1,phase,rules_version,snapshot) VALUES($1,$2,$3,$4,'playing',$5,$6)")
                .bind(game).bind(room).bind(members[0].get::<Uuid,_>("user_id")).bind(members[1].get::<Uuid,_>("user_id"))
                .bind(r.get::<String,_>("rules_version")).bind(snapshot).execute(&mut *c).await?;
            sqlx::query("UPDATE patchwork.rooms SET phase='playing',version=version+1,updated_at=now() WHERE room_id=$1")
                .bind(room).execute(c).await?;
            Ok(())
        }) }).await
    }
    pub async fn write_game(&self, write: GameWrite) -> Result<Committed<Value>, E> {
        request_id(&write.request_id)?;
        if write.expected_version < 0 || write.expected_version == i64::MAX {
            return Err(E::InvalidInput);
        }
        // Full input is hashed: retries cannot substitute a different server-derived result.
        let fingerprint = hash(&write)?;
        self.transaction(move |c| { let w=write.clone(); let fingerprint=fingerprint.clone(); Box::pin(async move {
            let room:Uuid=sqlx::query_scalar("SELECT room_id FROM patchwork.games WHERE game_id=$1")
                .bind(w.game).fetch_optional(&mut *c).await?.ok_or(E::NotFound)?;
            sqlx::query("SELECT room_id FROM patchwork.rooms WHERE room_id=$1 FOR UPDATE")
                .bind(room).fetch_one(&mut *c).await?;
            let game=sqlx::query("SELECT player0,player1,phase,state_version,event_seq,rules_version FROM patchwork.games WHERE game_id=$1 FOR UPDATE")
                .bind(w.game).fetch_optional(&mut *c).await?.ok_or(E::NotFound)?;
            // Legacy fixture writer never accepts authoritative custom-game snapshots.
            if game.get::<String,_>("rules_version") == game_core::rules::CUSTOM_RULES_VERSION { return Err(E::InvalidInput.into()); }
            if w.user!=game.get::<Uuid,_>("player0") && w.user!=game.get::<Uuid,_>("player1") { return Err(E::Permission.into()); }
            if let Some(row)=sqlx::query("SELECT payload_hash,response FROM patchwork.command_receipts WHERE game_id=$1 AND user_id=$2 AND request_id=$3")
                .bind(w.game).bind(w.user).bind(&w.request_id).fetch_optional(&mut *c).await? {
                if row.get::<Vec<u8>,_>("payload_hash")!=fingerprint { return Err(E::RequestIdConflict.into()); }
                return Ok(row.get("response"));
            }
            if game.get::<i64,_>("state_version")!=w.expected_version { return Err(E::VersionConflict.into()); }
            if game.get::<String,_>("phase")!="playing" { return Err(E::Conflict.into()); }
            let version=w.expected_version+1;
            let seq=game.get::<i64,_>("event_seq").checked_add(1).ok_or(E::InvalidInput)?;
            let phase=match &w.result { Some(r) if r.reason=="abandoned" => "abandoned", Some(_) => "finished", None => "playing" };
            let changed=sqlx::query("UPDATE patchwork.games SET snapshot=$2,state_version=$3,event_seq=$4,phase=$5,updated_at=now() WHERE game_id=$1 AND state_version=$6")
                .bind(w.game).bind(w.snapshot).bind(version).bind(seq).bind(phase).bind(w.expected_version).execute(&mut *c).await?;
            if changed.rows_affected()!=1 { return Err(E::VersionConflict.into()); }
            sqlx::query("INSERT INTO patchwork.game_events(game_id,seq,state_version,user_id,payload) VALUES($1,$2,$3,$4,$5)")
                .bind(w.game).bind(seq).bind(version).bind(w.user).bind(w.event).execute(&mut *c).await?;
            if let Some(r)=w.result {
                sqlx::query("INSERT INTO patchwork.game_results(game_id,score0,score1,winner_seat,reason) VALUES($1,$2,$3,$4,$5)")
                    .bind(w.game).bind(r.score0).bind(r.score1).bind(r.winner_seat).bind(r.reason).execute(&mut *c).await?;
                let changed=sqlx::query("UPDATE patchwork.rooms SET phase='finished',version=version+1,updated_at=now() WHERE room_id=$1 AND phase='playing' AND version<9223372036854775807")
                    .bind(room).execute(&mut *c).await?;
                if changed.rows_affected()!=1 {return Err(E::VersionConflict.into());}
            }
            let response=json!({"game_id":w.game,"version":version,"event_seq":seq});
            sqlx::query("INSERT INTO patchwork.command_receipts(game_id,user_id,request_id,payload_hash,state_version,response) VALUES($1,$2,$3,$4,$5,$6)")
                .bind(w.game).bind(w.user).bind(w.request_id).bind(fingerprint).bind(version).bind(&response).execute(c).await?;
            Ok(response)
        }) }).await
    }
    pub async fn recover_game_write(
        &self,
        write: GameWrite,
    ) -> Result<Committed<Option<Value>>, E> {
        request_id(&write.request_id)?;
        let fingerprint = hash(&write)?;
        self.transaction(move |c| { let w=write.clone(); let fingerprint=fingerprint.clone(); Box::pin(async move {
            sqlx::query("SELECT game_id FROM patchwork.games WHERE game_id=$1 FOR UPDATE").bind(w.game)
                .fetch_optional(&mut *c).await?.ok_or(E::NotFound)?;
            let row=sqlx::query("SELECT payload_hash,response FROM patchwork.command_receipts WHERE game_id=$1 AND user_id=$2 AND request_id=$3")
                .bind(w.game).bind(w.user).bind(w.request_id).fetch_optional(c).await?;
            row.map(|r| { if r.get::<Vec<u8>,_>("payload_hash")!=fingerprint { Err(E::RequestIdConflict.into()) } else { Ok(r.get("response")) } }).transpose()
        }) }).await
    }
    pub async fn game_snapshot(&self, game: Uuid) -> Result<Value, E> {
        sqlx::query_scalar("SELECT snapshot FROM patchwork.games WHERE game_id=$1")
            .bind(game)
            .fetch_optional(self.pool())
            .await
            .map_err(|_| E::Unavailable)?
            .ok_or(E::NotFound)
    }
    pub async fn events_after(&self, game: Uuid, seq: i64, limit: i64) -> Result<Vec<Value>, E> {
        if seq < 0 || !(1..=100).contains(&limit) {
            return Err(E::InvalidInput);
        }
        sqlx::query_scalar("SELECT jsonb_build_object('seq',seq,'version',state_version,'payload',payload) FROM patchwork.game_events WHERE game_id=$1 AND seq>$2 ORDER BY seq LIMIT $3")
            .bind(game).bind(seq).bind(limit).fetch_all(self.pool()).await.map_err(|_| E::Unavailable)
    }
    pub async fn rooms_page(
        &self,
        after: Option<(DateTime<Utc>, Uuid)>,
        limit: i64,
    ) -> Result<Vec<Value>, E> {
        if !(1..=100).contains(&limit) {
            return Err(E::InvalidInput);
        }
        let (time, id) = after.map_or((None, None), |(t, id)| (Some(t), Some(id)));
        sqlx::query_scalar("SELECT jsonb_build_object('room_id',room_id,'code',code,'created_at',created_at,'version',version,'mode',mode,'rules_version',rules_version,'has_password',password_hash IS NOT NULL) FROM patchwork.rooms WHERE phase='waiting' AND ($1::timestamptz IS NULL OR (created_at,room_id)>($1,$2)) ORDER BY created_at,room_id LIMIT $3")
            .bind(time).bind(id).bind(limit).fetch_all(self.pool()).await.map_err(|_| E::Unavailable)
    }
}
