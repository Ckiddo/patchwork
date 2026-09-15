//! Durable recovery clocks and consistent replay reads. No network side effects.
use super::{
    Database, StoreError as E,
    friends::*,
    transaction::{Committed, TxError},
};
use crate::recovery::RecoveryConfig;
use prost::Message;
use serde_json::json;
use sqlx::{PgConnection, Row};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};
use util_lib::protocol::{MAX_SERVER_MESSAGE_BYTES, v1};
use uuid::Uuid;

#[derive(Clone)]
pub struct Presence {
    pub online: HashMap<Uuid, Permit>,
    pub synchronized: HashMap<Uuid, Uuid>,
    pub registering: HashSet<Uuid>,
    pub revision: Arc<AtomicU64>,
    pub observed_revision: u64,
    pub user_revisions: HashMap<Uuid, (Arc<AtomicU64>, u64)>,
}
impl Presence {
    pub(crate) fn stable(&self) -> bool {
        self.revision.load(Ordering::SeqCst) == self.observed_revision
    }
    /// A game commit depends only on its participants. Unrelated room disconnects
    /// must not revoke an otherwise valid action while its transaction is running.
    pub(crate) fn stable_for(&self, users: &[Uuid]) -> bool {
        users.iter().all(|user| {
            !self.registering.contains(user)
                && self.online.get(user).is_some_and(Permit::live)
                && self
                    .user_revisions
                    .get(user)
                    .is_some_and(|(revision, observed)| {
                        revision.load(Ordering::SeqCst) == *observed
                    })
        })
    }
}

#[cfg(test)]
mod presence_tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn another_rooms_churn_does_not_revoke_the_participants() {
        let users = [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
        let mut presence = Presence {
            online: users
                .iter()
                .map(|user| {
                    (
                        *user,
                        Permit {
                            user: *user,
                            session: Uuid::new_v4(),
                            generation: 1,
                            expires: i64::MAX,
                            current: Arc::new(AtomicBool::new(true)),
                        },
                    )
                })
                .collect(),
            synchronized: HashMap::new(),
            registering: HashSet::new(),
            revision: Arc::new(AtomicU64::new(0)),
            observed_revision: 0,
            user_revisions: users
                .iter()
                .map(|user| (*user, (Arc::new(AtomicU64::new(0)), 0)))
                .collect(),
        };
        let participants = &users[..2];
        assert!(presence.stable_for(participants));
        presence.revision.fetch_add(1, Ordering::SeqCst);
        presence.online[&users[2]]
            .current
            .store(false, Ordering::SeqCst);
        presence.user_revisions[&users[2]]
            .0
            .fetch_add(1, Ordering::SeqCst);
        assert!(!presence.stable());
        assert!(presence.stable_for(participants));

        // An in-flight takeover must still fence this room, even before the
        // durable generation claim returns and replaces the old socket.
        presence.registering.insert(users[0]);
        assert!(!presence.stable_for(participants));
        presence.registering.clear();
        presence.user_revisions[&users[0]]
            .0
            .fetch_add(1, Ordering::SeqCst);
        assert!(!presence.stable_for(participants));
    }
}
#[derive(Clone)]
pub struct Observation {
    pub room: Uuid,
    pub epoch: Uuid,
    pub tick: Uuid,
    pub elapsed_ms: i64,
    pub started: Instant,
    pub presence: Presence,
    pub policy: RecoveryConfig,
}
async fn lock_room(c: &mut PgConnection, id: Uuid) -> Result<(), TxError> {
    let users: Vec<Uuid> = sqlx::query_scalar(
        "SELECT user_id FROM patchwork.room_members WHERE room_id=$1 ORDER BY user_id",
    )
    .bind(id)
    .fetch_all(&mut *c)
    .await?;
    for user in &users {
        sqlx::query("SELECT user_id FROM patchwork.users WHERE user_id=$1 FOR UPDATE")
            .bind(user)
            .fetch_one(&mut *c)
            .await?;
    }
    for user in &users {
        sqlx::query("SELECT user_id FROM patchwork.player_occupancy WHERE user_id=$1 FOR UPDATE")
            .bind(user)
            .fetch_one(&mut *c)
            .await?;
    }
    sqlx::query("SELECT room_id FROM patchwork.rooms WHERE room_id=$1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *c)
        .await?
        .ok_or(E::NotFound)?;
    let after: Vec<Uuid> = sqlx::query_scalar(
        "SELECT user_id FROM patchwork.room_members WHERE room_id=$1 ORDER BY user_id",
    )
    .bind(id)
    .fetch_all(&mut *c)
    .await?;
    if users != after {
        return Err(E::VersionConflict.into());
    }
    Ok(())
}
async fn outcome(
    c: &mut PgConnection,
    id: Uuid,
    mut recipients: Vec<Uuid>,
) -> Result<Outcome, TxError> {
    let room = read_room(c, id).await?;
    let game = read_game(c, room.game).await?;
    recipients.extend(room.members.iter().map(|m| m.user));
    recipients.sort();
    recipients.dedup();
    Ok(Outcome {
        room,
        game,
        recipients,
    })
}
async fn transition(
    c: &mut PgConnection,
    game: &GameView,
    phase: &str,
    reason: &str,
    winner: Option<i16>,
) -> Result<(), TxError> {
    if game.rules == game_core::rules::CUSTOM_RULES_VERSION {
        use game_core::{Seat, state::ResultReason};
        let state = super::gameplay::decode(game)?;
        let (next, events, result_reason) = match phase {
            "playing" | "paused" => (
                state
                    .with_connection_pause(phase == "paused")
                    .map_err(super::gameplay::core_error)?,
                vec![],
                None,
            ),
            "finished" | "abandoned" => {
                let why = if phase == "abandoned" {
                    ResultReason::Abandoned
                } else {
                    let winner = Seat::try_from(
                        u32::try_from(winner.ok_or(E::Unavailable)?).map_err(|_| E::Unavailable)?,
                    )
                    .map_err(|_| E::Unavailable)?;
                    ResultReason::Forfeit {
                        loser: winner.other(),
                    }
                };
                let end = state.terminate(why).map_err(super::gameplay::core_error)?;
                (
                    end.state,
                    end.events,
                    Some(if phase == "abandoned" {
                        "abandoned"
                    } else {
                        "timeout"
                    }),
                )
            }
            _ => return Err(E::Unavailable.into()),
        };
        return super::gameplay::persist(c, game, &next, &events, None, reason, result_reason)
            .await;
    }
    let seq = game.seq.checked_add(1).ok_or(E::InvalidInput)?;
    let version = game.version.checked_add(1).ok_or(E::InvalidInput)?;
    sqlx::query("UPDATE patchwork.games SET phase=$2,state_version=state_version+1,event_seq=event_seq+1,updated_at=now() WHERE game_id=$1")
        .bind(game.id).bind(phase).execute(&mut *c).await?;
    sqlx::query("INSERT INTO patchwork.game_events(game_id,seq,state_version,user_id,payload) VALUES($1,$2,$3,NULL,$4)")
        .bind(game.id).bind(seq).bind(version)
        .bind(json!({"kind":"connection_state_v1","phase":phase,"reason":reason,"state":game.state}))
        .execute(&mut *c).await?;
    Ok(())
}
impl Database {
    /// Idempotent for one startup epoch, including a lost COMMIT acknowledgement.
    pub async fn recovery_boot(&self, epoch: Uuid, policy: RecoveryConfig) -> Result<(), E> {
        let rooms: Vec<Uuid> = sqlx::query_scalar("SELECT room_id FROM patchwork.rooms WHERE phase IN ('waiting','starting','playing','finished') ORDER BY room_id")
            .fetch_all(self.pool()).await.map_err(|_|E::Unavailable)?;
        for id in rooms {
            let policy = policy.clone();
            self.transaction(move |c| {let policy=policy.clone(); Box::pin(async move {
                lock_room(c,id).await?;
                let old: Option<Uuid> = sqlx::query_scalar("SELECT server_epoch FROM patchwork.room_recovery WHERE room_id=$1")
                    .bind(id).fetch_optional(&mut *c).await?;
                if old==Some(epoch) {return Ok(());}
                sqlx::query("INSERT INTO patchwork.room_recovery(room_id,server_epoch,restart_remaining_ms,recovering0,recovering1) VALUES($1,$2,$3,true,true) ON CONFLICT(room_id) DO UPDATE SET server_epoch=$2,last_tick=NULL,both_offline_ms=0,restart_remaining_ms=$3,recovering0=true,recovering1=true")
                    .bind(id).bind(epoch).bind(policy.restart_grace_secs as i64*1000).execute(&mut *c).await?;
                sqlx::query("UPDATE patchwork.room_members SET disconnected_at=now(),offline_ms=0,ready=false WHERE room_id=$1")
                    .bind(id).execute(&mut *c).await?;
                let room=read_room(c,id).await?;
                if let Some(game)=read_game(c,room.game).await?
                    && matches!(game.phase.as_str(),"playing"|"paused") {
                        transition(c,&game,"paused","server_restarted",None).await?;
                }
                sqlx::query("UPDATE patchwork.rooms SET version=version+1 WHERE room_id=$1")
                    .bind(id).execute(&mut *c).await?;
                Ok(())
            })}).await?;
        }
        Ok(())
    }
    pub async fn observe_room(&self, o: Observation) -> Result<Committed<Outcome>, E> {
        self.transaction(move |c| {let o=o.clone(); Box::pin(async move {
            lock_room(c,o.room).await?;
            sqlx::query("INSERT INTO patchwork.room_recovery(room_id,server_epoch) VALUES($1,$2) ON CONFLICT DO NOTHING")
                .bind(o.room).bind(o.epoch).execute(&mut *c).await?;
            let clock=sqlx::query("SELECT * FROM patchwork.room_recovery WHERE room_id=$1")
                .bind(o.room).fetch_one(&mut *c).await?;
            if clock.get::<Uuid,_>("server_epoch")!=o.epoch {return Err(E::Permission.into());}
            // A retried tick never charges twice, even after connection takeover.
            if clock.get::<Option<Uuid>,_>("last_tick")==Some(o.tick) {
                return outcome(c,o.room,vec![]).await;
            }
            let room=read_room(c,o.room).await?;
            let recipients=room.members.iter().map(|m|m.user).collect::<Vec<_>>();
            if !o.presence.stable() || recipients.iter().any(|u|o.presence.registering.contains(u))
                || o.started.elapsed().as_millis()>2000 {return Err(E::Unavailable.into());}
            let delta=o.elapsed_ms.clamp(0,3000);
            let mut online=[false;2];
            let mut was_offline=[false;2];
            let mut changed=false;
            for member in &room.members {
                let previous=sqlx::query("SELECT disconnected_at IS NOT NULL AS offline,offline_ms,connection_generation FROM patchwork.room_members WHERE room_id=$1 AND user_id=$2")
                    .bind(o.room).bind(member.user).fetch_one(&mut *c).await?;
                let seat=member.seat as usize;
                was_offline[seat]=previous.get("offline");
                let generation: i64=sqlx::query_scalar("SELECT connection_generation FROM patchwork.users WHERE user_id=$1")
                    .bind(member.user).fetch_one(&mut *c).await?;
                let permit=o.presence.online.get(&member.user);
                if let Some(p)=permit {
                    // A DB claim can precede its Registry callback: this sample is inconclusive.
                    if p.generation as i64 != generation || !p.live() {return Err(E::Unavailable.into());}
                    fence(c,p).await?;
                }
                online[seat]=permit.is_some();
                let offline_ms=if online[seat] {0} else {
                    previous.get::<i64,_>("offline_ms") + if was_offline[seat] {delta} else {0}
                };
                changed |= was_offline[seat]==online[seat]
                    || permit.is_some_and(|p|p.generation as i64 != previous.get::<i64,_>("connection_generation"));
                sqlx::query("UPDATE patchwork.room_members SET disconnected_at=CASE WHEN $3 THEN NULL ELSE COALESCE(disconnected_at,now()) END,offline_ms=$4,connection_generation=$5,ready=CASE WHEN $3 THEN ready ELSE false END WHERE room_id=$1 AND user_id=$2")
                    .bind(o.room).bind(member.user).bind(online[seat]).bind(offline_ms)
                    .bind(permit.map(|p|p.generation as i64).unwrap_or(previous.get("connection_generation")))
                    .execute(&mut *c).await?;
                if matches!(room.phase.as_str(),"waiting"|"finished") && !online[seat]
                    && offline_ms>=o.policy.waiting_grace_secs as i64*1000 {
                    sqlx::query("UPDATE patchwork.player_occupancy SET state='idle',room_id=NULL WHERE user_id=$1")
                        .bind(member.user).execute(&mut *c).await?;
                    sqlx::query("DELETE FROM patchwork.room_members WHERE room_id=$1 AND user_id=$2")
                        .bind(o.room).bind(member.user).execute(&mut *c).await?;
                    changed=true;
                }
            }
            if matches!(room.phase.as_str(),"waiting"|"finished") && changed {
                let remaining: Vec<Uuid>=sqlx::query_scalar("SELECT user_id FROM patchwork.room_members WHERE room_id=$1 ORDER BY seat")
                    .bind(o.room).fetch_all(&mut *c).await?;
                let owner=room.owner.filter(|u|remaining.contains(u)).or_else(||remaining.first().copied());
                sqlx::query("UPDATE patchwork.rooms SET owner_id=$2,phase=CASE WHEN $2::uuid IS NULL THEN 'closed' ELSE phase END WHERE room_id=$1")
                    .bind(o.room).bind(owner).execute(&mut *c).await?;
                // Any presence transition invalidates a waiting room's ready consensus.
                sqlx::query("UPDATE patchwork.room_members SET ready=false WHERE room_id=$1")
                    .bind(o.room).execute(&mut *c).await?;
            }
            let mut budgets=[clock.get::<i64,_>("budget0_ms"),clock.get("budget1_ms")];
            let mut both=clock.get::<i64,_>("both_offline_ms");
            let mut restart=clock.get::<i64,_>("restart_remaining_ms");
            let mut recovering=[clock.get::<bool,_>("recovering0"),clock.get("recovering1")];
            if let Some(game)=read_game(c,room.game).await?
                && matches!(game.phase.as_str(),"playing"|"paused") {
                    // An open transport is insufficient: a player must confirm restored state.
                    for member in &room.members {
                        online[member.seat as usize]&=o.presence.synchronized.get(&member.user)==Some(&game.id);
                    }
                    was_offline=recovering;
                    recovering=[!online[0],!online[1]];
                    let synced=room.members.iter().all(|m|o.presence.synchronized.get(&m.user)==Some(&game.id));
                    let all_online=online==[true,true];
                    if all_online && synced {restart=0;}
                    else {restart=(restart-delta).max(0);}
                    let mut winner=None;
                    let mut abandoned=false;
                    if online==[false,false] {
                        both+=if was_offline==[true,true] {delta} else {0};
                        abandoned=both>=o.policy.both_offline_retention_secs as i64*1000;
                    } else {
                        both=0;
                        if restart==0 && clock.get::<i64,_>("restart_remaining_ms")==0 {
                            for seat in 0..2 {
                                if !online[seat] && online[1-seat] {
                                    budgets[seat]+=if was_offline[seat] && !was_offline[1-seat] {delta} else {0};
                                    if budgets[seat]>=o.policy.game_budget_secs as i64*1000 {
                                        winner=Some((1-seat) as i16);
                                    }
                                }
                            }
                        }
                    }
                    let phase=if abandoned {"abandoned"} else if winner.is_some() {"finished"}
                        else if all_online && synced {"playing"} else {"paused"};
                    if phase!=game.phase {
                        transition(c,&game,phase,if abandoned {"both_offline"} else if winner.is_some() {"disconnect_budget"} else {"connection_changed"},winner).await?;
                        changed=true;
                    }
                    if abandoned || winner.is_some() {
                        if game.rules != game_core::rules::CUSTOM_RULES_VERSION {
                            sqlx::query("INSERT INTO patchwork.game_results(game_id,score0,score1,winner_seat,reason) VALUES($1,0,0,$2,$3)")
                            .bind(game.id).bind(winner).bind(if abandoned {"abandoned"}else{"timeout"}).execute(&mut *c).await?;
                        }
                        sqlx::query("UPDATE patchwork.rooms SET phase='finished' WHERE room_id=$1")
                            .bind(o.room).execute(&mut *c).await?;
                    }
            }
            sqlx::query("UPDATE patchwork.room_recovery SET last_tick=$2,budget0_ms=$3,budget1_ms=$4,both_offline_ms=$5,restart_remaining_ms=$6,recovering0=$7,recovering1=$8 WHERE room_id=$1")
                .bind(o.room).bind(o.tick).bind(budgets[0]).bind(budgets[1]).bind(both).bind(restart).bind(recovering[0]).bind(recovering[1]).execute(&mut *c).await?;
            if changed {
                sqlx::query("UPDATE patchwork.rooms SET version=version+1 WHERE room_id=$1")
                    .bind(o.room).execute(&mut *c).await?;
            }
            if !o.presence.stable() || o.started.elapsed().as_millis()>2000 {
                return Err(E::Unavailable.into());
            }
            outcome(c,o.room,recipients).await
        })}).await
    }
    /// Room SHARE lock covers the game snapshot, phase and contiguous event tail.
    pub async fn resume_game(
        &self,
        permit: Permit,
        request: v1::ResumeRequest,
    ) -> Result<v1::ResumeState, E> {
        let id = Uuid::parse_str(&request.game_id).map_err(|_| E::InvalidInput)?;
        let last = i64::try_from(request.last_seq).map_err(|_| E::InvalidInput)?;
        self.transaction(move |c| {let p=permit.clone(); let request=request.clone(); Box::pin(async move {
            let room: Uuid=sqlx::query_scalar("SELECT room_id FROM patchwork.games WHERE game_id=$1")
                .bind(id).fetch_optional(&mut *c).await?.ok_or(E::NotFound)?;
            sqlx::query("SELECT room_id FROM patchwork.rooms WHERE room_id=$1 FOR SHARE")
                .bind(room).fetch_one(&mut *c).await?;
            fence(c,&p).await?;
            let member: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM patchwork.games WHERE game_id=$1 AND (player0=$2 OR player1=$2))")
                .bind(id).bind(p.user).fetch_one(&mut *c).await?;
            if !member {return Err(E::Permission.into());}
            let game=read_game(c,Some(id)).await?.ok_or(E::NotFound)?;
            if last>game.seq {return Err(E::InvalidInput.into());}
            let mut state=v1::ResumeState {game_id:id.to_string(),version:game.version as u64,event_seq:game.seq as u64,
                sync_token:Uuid::new_v4().to_string(),snapshot:None,events:vec![],phase:game.phase.clone()};
            if request.has_snapshot && game.seq-last<=64 {
                let rows=sqlx::query("SELECT seq,state_version,payload FROM patchwork.game_events WHERE game_id=$1 AND seq>$2 ORDER BY seq LIMIT 64")
                    .bind(id).bind(last).fetch_all(&mut *c).await?;
                for r in rows {
                    state.events.push(v1::GameEvent {seq:r.get::<i64,_>("seq") as u64,version:r.get::<i64,_>("state_version") as u64,
                        payload_json:r.get::<serde_json::Value,_>("payload").to_string().into_bytes()});
                }
            }
            let contiguous=state.events.len() as i64==game.seq-last
                && state.events.iter().enumerate().all(|(i,e)|e.seq==last as u64+i as u64+1);
            if !request.has_snapshot || !contiguous || state.encoded_len()>MAX_SERVER_MESSAGE_BYTES-256 {
                state.events.clear();
                state.snapshot=Some(v1::GameSnapshot {game_id:game.id.to_string(),version:game.version as u64,
                    event_seq:game.seq as u64,rules_version:game.rules.clone(),state_json:game.state.to_string().into_bytes(),phase:game.phase.clone()});
            }
            if state.encoded_len()>MAX_SERVER_MESSAGE_BYTES-256 {return Err(E::Unavailable.into());}
            if !p.live() {return Err(E::Permission.into());}
            Ok(state)
        })}).await.map(Committed::into_inner)
    }
}
