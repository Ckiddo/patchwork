use super::*;
use backend::{
    persistence::{StoreError as E, recovery::Observation},
    recovery::RecoveryConfig,
    sessions::{Publish, Snapshot},
};
use sqlx::Row;
use std::time::{Duration, Instant};

fn policy() -> RecoveryConfig {
    RecoveryConfig {
        waiting_grace_secs: 2,
        game_budget_secs: 3,
        both_offline_retention_secs: 4,
        restart_grace_secs: 2,
    }
}
async fn observation(h: &Harness, room: Uuid, epoch: Uuid, elapsed_ms: i64) -> Observation {
    Observation {
        room,
        epoch,
        tick: Uuid::new_v4(),
        elapsed_ms,
        started: Instant::now(),
        presence: h.registry.send(Snapshot).await.unwrap(),
        policy: policy(),
    }
}
async fn tick(
    h: &Harness,
    room: Uuid,
    epoch: Uuid,
    ms: i64,
) -> backend::persistence::friends::Outcome {
    h.db.observe_room(observation(h, room, epoch, ms).await)
        .await
        .unwrap()
        .into_inner()
}
async fn started(h: &Harness) -> (Player, Player, Uuid, Uuid) {
    let a = player(h).await;
    let b = player(h).await;
    let r = create(h, &a, None).await;
    let r = join(h, &b, &r).await;
    let r = ready(h, &a, &r).await;
    let r = ready(h, &b, &r).await;
    let r = room(
        call(
            h,
            &a,
            command(&r.room_id, r.version, Command::Start(v1::Empty {})),
        )
        .await,
    );
    (
        a,
        b,
        Uuid::parse_str(&r.room_id).unwrap(),
        Uuid::parse_str(&r.game_id).unwrap(),
    )
}
pub(super) async fn dispatch(
    h: &Harness,
    p: &Player,
    payload: v1::client_envelope::Payload,
) -> v1::ServerEnvelope {
    h.registry
        .send(Dispatch {
            stamp: p.stamp,
            message: v1::ClientEnvelope {
                protocol_version: VERSION,
                request_id: Uuid::new_v4().to_string(),
                payload: Some(payload),
            },
        })
        .await
        .unwrap()
        .unwrap()
}
pub(super) async fn resume(
    h: &Harness,
    p: &Player,
    g: Uuid,
    last: u64,
    has: bool,
) -> v1::ResumeState {
    let r = dispatch(
        h,
        p,
        v1::client_envelope::Payload::Resume(v1::ResumeRequest {
            game_id: g.to_string(),
            last_seq: last,
            has_snapshot: has,
        }),
    )
    .await;
    match r.payload {
        Some(Payload::Resumed(r)) => r,
        _ => panic!("resume failed"),
    }
}
pub(super) fn ack(r: &v1::ResumeState) -> v1::client_envelope::Payload {
    v1::client_envelope::Payload::SyncAck(v1::SyncAck {
        game_id: r.game_id.clone(),
        version: r.version,
        event_seq: r.event_seq,
        sync_token: r.sync_token.clone(),
    })
}
async fn sync(h: &Harness, p: &Player, g: Uuid) {
    let r = resume(h, p, g, 0, false).await;
    assert!(matches!(
        dispatch(h, p, ack(&r)).await.payload,
        Some(Payload::Acknowledged(_))
    ));
}
async fn budgets(h: &Harness, room: Uuid) -> (i64, i64, i64) {
    sqlx::query_as("SELECT budget0_ms,budget1_ms,both_offline_ms FROM patchwork.room_recovery WHERE room_id=$1")
        .bind(room).fetch_one(h.db.pool()).await.unwrap()
}

#[actix_web::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn waiting_grace_takeover_and_expiry_preserve_seats() {
    let h = harness(db().await).await;
    let a = player(&h).await;
    let b = player(&h).await;
    let r = create(&h, &a, None).await;
    let r = join(&h, &b, &r).await;
    let id = Uuid::parse_str(&r.room_id).unwrap();
    let epoch = Uuid::new_v4();
    h.db.recovery_boot(epoch, policy()).await.unwrap();
    tick(&h, id, epoch, 0).await;
    assert!(h.registry.send(Disconnect(a.stamp)).await.unwrap());
    assert!(!h.registry.send(Disconnect(a.stamp)).await.unwrap());
    let offline = tick(&h, id, epoch, 0).await;
    assert_eq!(offline.room.members.len(), 2);
    tick(&h, id, epoch, 1000).await;
    let new = connect(&h, a.stamp.user, a.session).await;
    assert!(!h.registry.send(Disconnect(a.stamp)).await.unwrap());
    let restored = tick(&h, id, epoch, 1000).await;
    assert_eq!(
        restored
            .room
            .members
            .iter()
            .find(|m| m.user == new.stamp.user)
            .unwrap()
            .seat,
        0
    );
    assert!(restored.room.members.iter().all(|m| !m.ready));
    let generation: i64 = sqlx::query_scalar(
        "SELECT connection_generation FROM patchwork.room_members WHERE user_id=$1",
    )
    .bind(new.stamp.user)
    .fetch_one(h.db.pool())
    .await
    .unwrap();
    assert_eq!(generation, new.stamp.generation as i64);
    h.registry.send(Disconnect(new.stamp)).await.unwrap();
    tick(&h, id, epoch, 0).await;
    let expired = tick(&h, id, epoch, 2000).await;
    assert_eq!(expired.room.owner, Some(b.stamp.user));
    assert_eq!(expired.room.members[0].seat, 1);
    let occupancy: String =
        sqlx::query_scalar("SELECT state FROM patchwork.player_occupancy WHERE user_id=$1")
            .bind(new.stamp.user)
            .fetch_one(h.db.pool())
            .await
            .unwrap();
    assert_eq!(occupancy, "idle");
    h.registry.send(Disconnect(b.stamp)).await.unwrap();
    tick(&h, id, epoch, 0).await;
    assert_eq!(tick(&h, id, epoch, 2000).await.room.phase, "closed");
}

#[actix_web::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn cumulative_budget_and_both_offline_do_not_reset_or_double_charge() {
    let h = harness(db().await).await;
    let (a, b, id, g) = started(&h).await;
    let epoch = Uuid::new_v4();
    h.db.recovery_boot(epoch, policy()).await.unwrap();
    sync(&h, &a, g).await;
    sync(&h, &b, g).await;
    tick(&h, id, epoch, 0).await;
    h.registry.send(Disconnect(a.stamp)).await.unwrap();
    tick(&h, id, epoch, 0).await;
    tick(&h, id, epoch, 1000).await;
    assert_eq!(budgets(&h, id).await.0, 1000);
    let new = connect(&h, a.stamp.user, a.session).await;
    sync(&h, &new, g).await;
    tick(&h, id, epoch, 1000).await;
    assert_eq!(budgets(&h, id).await.0, 1000);
    h.registry.send(Disconnect(new.stamp)).await.unwrap();
    h.registry.send(Disconnect(b.stamp)).await.unwrap();
    tick(&h, id, epoch, 0).await;
    tick(&h, id, epoch, 2000).await;
    assert_eq!(budgets(&h, id).await, (1000, 0, 2000));
    let b = connect(&h, b.stamp.user, b.session).await;
    sync(&h, &b, g).await;
    tick(&h, id, epoch, 1000).await;
    assert_eq!(
        budgets(&h, id).await.0,
        1000,
        "the previous both-offline interval must not be charged to one player"
    );
    let observation = observation(&h, id, epoch, 2000).await;
    let done =
        h.db.observe_room(observation.clone())
            .await
            .unwrap()
            .into_inner();
    assert_eq!(done.game.unwrap().phase, "finished");
    h.db.observe_room(observation).await.unwrap();
    assert_eq!(budgets(&h, id).await.0, 3000);
    let result =
        sqlx::query("SELECT winner_seat,reason FROM patchwork.game_results WHERE game_id=$1")
            .bind(g)
            .fetch_one(h.db.pool())
            .await
            .unwrap();
    assert_eq!(result.get::<i16, _>("winner_seat"), 1);
    assert_eq!(result.get::<String, _>("reason"), "timeout");
}

#[actix_web::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn both_offline_abandons_without_winner() {
    let h = harness(db().await).await;
    let (a, b, id, g) = started(&h).await;
    let epoch = Uuid::new_v4();
    h.db.recovery_boot(epoch, policy()).await.unwrap();
    h.registry.send(Disconnect(a.stamp)).await.unwrap();
    h.registry.send(Disconnect(b.stamp)).await.unwrap();
    tick(&h, id, epoch, 0).await;
    tick(&h, id, epoch, 2000).await;
    let done = tick(&h, id, epoch, 2000).await;
    assert_eq!(done.game.unwrap().phase, "abandoned");
    let winner: Option<i16> =
        sqlx::query_scalar("SELECT winner_seat FROM patchwork.game_results WHERE game_id=$1")
            .bind(g)
            .fetch_one(h.db.pool())
            .await
            .unwrap();
    assert!(winner.is_none());
    assert_eq!(budgets(&h, id).await, (0, 0, 4000));
}

#[actix_web::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn restart_and_stale_observations_do_not_charge_server_downtime() {
    let h = harness(db().await).await;
    let (a, b, id, g) = started(&h).await;
    let epoch = Uuid::new_v4();
    h.db.recovery_boot(epoch, policy()).await.unwrap();
    sync(&h, &a, g).await;
    sync(&h, &b, g).await;
    tick(&h, id, epoch, 0).await;
    h.registry.send(Disconnect(a.stamp)).await.unwrap();
    tick(&h, id, epoch, 0).await;
    tick(&h, id, epoch, 1000).await;
    let mut old = observation(&h, id, epoch, 3000).await;
    old.started = Instant::now() - Duration::from_secs(20);
    assert_eq!(h.db.observe_room(old).await.unwrap_err(), E::Unavailable);
    assert_eq!(budgets(&h, id).await.0, 1000);
    let reboot = Uuid::new_v4();
    h.db.recovery_boot(reboot, policy()).await.unwrap();
    let before = h.db.friend_current(id).await.unwrap();
    h.db.recovery_boot(reboot, policy()).await.unwrap();
    assert_eq!(before, h.db.friend_current(id).await.unwrap());
    tick(&h, id, reboot, 2000).await;
    assert_eq!(budgets(&h, id).await.0, 1000);
    assert_eq!(before.game.unwrap().phase, "paused");
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM patchwork.game_results WHERE game_id=$1")
            .bind(g)
            .fetch_one(h.db.pool())
            .await
            .unwrap();
    assert_eq!(count, 0);
}

#[actix_web::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn resume_tail_compaction_ack_and_generation_fences() {
    let h = harness(db().await).await;
    let (a, b, id, g) = started(&h).await;
    let outsider = player(&h).await;
    let epoch = Uuid::new_v4();
    h.db.recovery_boot(epoch, policy()).await.unwrap();
    let action = v1::client_envelope::Payload::Game(v1::GameRequest {
        game_id: g.to_string(),
        expected_version: 0,
        action: Some(v1::game_request::Action::Advance(v1::Empty {})),
    });
    assert_eq!(
        code(dispatch(&h, &a, action.clone()).await),
        v1::ErrorCode::SyncRequired as i32
    );
    let request = v1::client_envelope::Payload::Resume(v1::ResumeRequest {
        game_id: g.to_string(),
        last_seq: 0,
        has_snapshot: false,
    });
    assert_eq!(
        code(dispatch(&h, &outsider, request).await),
        v1::ErrorCode::Forbidden as i32
    );
    let full = resume(&h, &a, g, 0, false).await;
    assert!(full.snapshot.is_some());
    let delta = resume(&h, &a, g, 0, true).await;
    assert!(delta.snapshot.is_none());
    assert_eq!(delta.events.len(), 1);
    assert!(matches!(
        dispatch(&h, &a, ack(&delta)).await.payload,
        Some(Payload::Acknowledged(_))
    ));
    assert_eq!(
        code(dispatch(&h, &a, action.clone()).await),
        v1::ErrorCode::NotImplemented as i32
    );
    let stale = resume(&h, &a, g, 0, false).await;
    sync(&h, &a, g).await;
    sync(&h, &b, g).await;
    tick(&h, id, epoch, 0).await;
    assert_eq!(
        code(dispatch(&h, &a, ack(&stale)).await),
        v1::ErrorCode::VersionConflict as i32
    );
    let before = resume(&h, &a, g, 0, false).await;
    let new = connect(&h, a.stamp.user, a.session).await;
    assert_eq!(
        code(dispatch(&h, &new, ack(&before)).await),
        v1::ErrorCode::VersionConflict as i32
    );
    assert_eq!(
        code(dispatch(&h, &new, action).await),
        v1::ErrorCode::SyncRequired as i32
    );
    sqlx::query("DELETE FROM patchwork.game_events WHERE game_id=$1 AND seq=1")
        .bind(g)
        .execute(h.db.pool())
        .await
        .unwrap();
    assert!(resume(&h, &new, g, 0, true).await.snapshot.is_some());
}

#[actix_web::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn slow_consumer_only_loses_its_own_lease() {
    let h = harness(db().await).await;
    let a = player(&h).await;
    let b = player(&h).await;
    let r = create(&h, &a, None).await;
    let r = join(&h, &b, &r).await;
    let (tiny, _unread) = mpsc::channel(1);
    assert!(
        h.registry
            .send(Attach {
                stamp: a.stamp,
                push: tiny
            })
            .await
            .unwrap()
    );
    let outcome =
        h.db.friend_current(Uuid::parse_str(&r.room_id).unwrap())
            .await
            .unwrap();
    h.registry.send(Publish(outcome.clone())).await.unwrap();
    h.registry.send(Publish(outcome)).await.unwrap();
    let online = h.registry.send(Snapshot).await.unwrap().online;
    assert!(!online.contains_key(&a.stamp.user));
    assert!(online.contains_key(&b.stamp.user));
    assert!(matches!(
        dispatch(
            &h,
            &b,
            v1::client_envelope::Payload::Ping(v1::Ping { nonce: 9 })
        )
        .await
        .payload,
        Some(Payload::Pong(_))
    ));
}

#[actix_web::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn lost_tick_commit_ack_is_reconciled_once() {
    let h = harness(db().await).await;
    let (a, b, id, g) = started(&h).await;
    let epoch = Uuid::new_v4();
    h.db.recovery_boot(epoch, policy()).await.unwrap();
    sync(&h, &a, g).await;
    sync(&h, &b, g).await;
    tick(&h, id, epoch, 0).await;
    h.registry.send(Disconnect(a.stamp)).await.unwrap();
    tick(&h, id, epoch, 0).await;
    let config = backend::config::Config::load(std::path::Path::new(
        &std::env::var("PATCHWORK_TEST_CONFIG").unwrap(),
    ))
    .unwrap()
    .database
    .unwrap();
    let (port, armed, proxy) = pg_proxy::start(config.port).await;
    let relay = db_with(false, |c| {
        c.port = port;
        c.max_connections = 1;
    })
    .await;
    let o = observation(&h, id, epoch, 1000).await;
    armed.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        relay.observe_room(o.clone()).await.unwrap_err(),
        E::CommitUnknown
    );
    let committed = h.db.observe_room(o.clone()).await.unwrap().into_inner();
    assert_eq!(budgets(&h, id).await.0, 1000);
    assert_eq!(h.db.observe_room(o).await.unwrap().into_inner(), committed);
    let (kick, _rx) = watch::channel(false);
    armed.store(true, std::sync::atomic::Ordering::SeqCst);
    let claimed = h
        .registry
        .send(Register {
            database: relay.clone(),
            user: b.stamp.user,
            session: b.session,
            auth_version: 0,
            expires: chrono::Utc::now().timestamp() + 300,
            kick,
        })
        .await
        .unwrap();
    assert_eq!(claimed.unwrap_err(), E::CommitUnknown);
    assert!(
        !h.registry
            .send(Snapshot)
            .await
            .unwrap()
            .online
            .contains_key(&b.stamp.user)
    );
    relay.close().await;
    proxy.abort();
}

#[actix_web::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn paused_postgres_response_freezes_clocks_without_blocking_registry() {
    let h = harness(db().await).await;
    let (a, b, id, g) = started(&h).await;
    let epoch = Uuid::new_v4();
    h.db.recovery_boot(epoch, policy()).await.unwrap();
    sync(&h, &a, g).await;
    sync(&h, &b, g).await;
    tick(&h, id, epoch, 0).await;
    h.registry.send(Disconnect(a.stamp)).await.unwrap();
    tick(&h, id, epoch, 0).await;
    let config = backend::config::Config::load(std::path::Path::new(
        &std::env::var("PATCHWORK_TEST_CONFIG").unwrap(),
    ))
    .unwrap()
    .database
    .unwrap();
    let (port, paused, proxy) = pg_proxy::pausable(config.port).await;
    let relay = db_with(false, |c| {
        c.port = port;
        c.max_connections = 1;
    })
    .await;
    paused.store(true, std::sync::atomic::Ordering::SeqCst);
    let o = observation(&h, id, epoch, 3000).await;
    let db = relay.clone();
    let task = actix_web::rt::spawn(async move { db.observe_room(o).await });
    let pong = tokio::time::timeout(
        Duration::from_millis(300),
        dispatch(
            &h,
            &b,
            v1::client_envelope::Payload::Ping(v1::Ping { nonce: 1 }),
        ),
    )
    .await
    .unwrap();
    assert!(matches!(pong.payload, Some(Payload::Pong(_))));
    tokio::time::sleep(Duration::from_millis(2100)).await;
    paused.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(task.await.unwrap().unwrap_err(), E::Unavailable);
    assert_eq!(budgets(&h, id).await.0, 0);
    let stranger = player(&h).await;
    let (kick, _rx) = watch::channel(false);
    let registry = h.registry.clone();
    let db = relay.clone();
    paused.store(true, std::sync::atomic::Ordering::SeqCst);
    let registration = actix_web::rt::spawn(async move {
        registry
            .send(Register {
                database: db,
                user: stranger.stamp.user,
                session: stranger.session,
                auth_version: 0,
                expires: chrono::Utc::now().timestamp() + 300,
                kick,
            })
            .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        h.registry
            .send(Snapshot)
            .await
            .unwrap()
            .registering
            .contains(&stranger.stamp.user)
    );
    tokio::time::timeout(
        Duration::from_millis(300),
        dispatch(
            &h,
            &b,
            v1::client_envelope::Payload::Ping(v1::Ping { nonce: 2 }),
        ),
    )
    .await
    .unwrap();
    paused.store(false, std::sync::atomic::Ordering::SeqCst);
    registration.await.unwrap().unwrap().unwrap();
    assert_eq!(budgets(&h, id).await.0, 0);
    relay.close().await;
    proxy.abort();
}

#[actix_web::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn room_timer_expires_seat_and_reconciles_lobby_directory() {
    let database = db().await;
    let lobby = LobbyManager::default().start();
    let state = AppState::new(b"isolated-recovery-clock-test", lobby.clone())
        .with_recovery(policy())
        .with_database(database.clone());
    assert!(lobby.send(RoomsReady).await.unwrap());
    let h = Harness {
        db: database,
        registry: state.registry.clone(),
        lobby,
    };
    let a = player(&h).await;
    let b = player(&h).await;
    let r = create(&h, &a, None).await;
    let r = join(&h, &b, &r).await;
    h.registry.send(Disconnect(a.stamp)).await.unwrap();
    let id = Uuid::parse_str(&r.room_id).unwrap();
    tokio::time::timeout(Duration::from_secs(7), async {
        loop {
            if h.db.friend_current(id).await.unwrap().room.members.len() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("room timer did not expire the seat");
    let new = connect(&h, a.stamp.user, a.session).await;
    let current = h.db.friend_current(id).await.unwrap();
    assert_eq!(current.room.owner, Some(b.stamp.user));
    assert_eq!(current.room.members[0].seat, 1);
    assert_eq!(
        code(call(&h, &new, command("", 0, Command::Get(v1::Empty {}))).await),
        v1::ErrorCode::NotFound as i32
    );
    assert_ne!(create(&h, &new, None).await.room_id, r.room_id);
}
