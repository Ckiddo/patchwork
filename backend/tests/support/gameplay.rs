use super::*;
use backend::{
    game::Configure,
    persistence::{
        StoreError as E,
        friends::{Mutation, RoomMutation},
        gameplay::fingerprint,
        recovery::Observation,
    },
    recovery::RecoveryConfig,
    sessions::Snapshot,
};
use game_core::{
    BoardPosition, Seat,
    geometry::Orientation,
    rules::{CUSTOM_RULES_VERSION, PatchId, patch},
    state::{ActionPhase, GameSnapshot, Lifecycle},
};
use sqlx::Row;
use std::{
    sync::{Arc, atomic::Ordering},
    time::Instant,
};

async fn rig(database: Database) -> (Harness, Uuid) {
    let lobby = LobbyManager::default().start();
    let registry = SessionRegistry::with_lobby(lobby.clone()).start();
    let epoch = Uuid::new_v4();
    lobby
        .send(Configure {
            database: database.clone(),
            registry: registry.clone(),
            fingerprint_key: b"isolated-gameplay-tests".to_vec(),
            password_workers: Arc::new(tokio::sync::Semaphore::new(2)),
            recovery: RecoveryConfig::default(),
            epoch,
        })
        .await
        .unwrap();
    assert!(lobby.send(RoomsReady).await.unwrap());
    (
        Harness {
            db: database,
            registry,
            lobby,
        },
        epoch,
    )
}
async fn start(h: &Harness) -> (Player, Player, v1::RoomSnapshot, v1::LobbyRequest, String) {
    let a = player(h).await;
    let b = player(h).await;
    let r = room(
        call(
            h,
            &a,
            command(
                "",
                0,
                Command::Create(v1::CreateRoom {
                    mode: "casual".into(),
                    rules_version: CUSTOM_RULES_VERSION.into(),
                    password: None,
                }),
            ),
        )
        .await,
    );
    let r = join(h, &b, &r).await;
    let r = ready(h, &a, &r).await;
    let r = ready(h, &b, &r).await;
    let req = command(&r.room_id, r.version, Command::Start(v1::Empty {}));
    let id = Uuid::new_v4().to_string();
    let r = room(send(h, &a, req.clone(), &id).await);
    (a, b, r, req, id)
}
async fn read(h: &Harness, room_id: Uuid) -> backend::persistence::friends::GameView {
    h.db.friend_current(room_id).await.unwrap().game.unwrap()
}
fn core(g: &backend::persistence::friends::GameView) -> GameSnapshot {
    serde_json::from_value(g.state.clone()).unwrap()
}
async fn sync(h: &Harness, p: &Player, id: Uuid) {
    for _ in 0..4 {
        let resumed = recovery::resume(h, p, id, 0, false).await;
        let response = recovery::dispatch(h, p, recovery::ack(&resumed)).await;
        if matches!(response.payload, Some(Payload::Acknowledged(_))) {
            return;
        }
        assert_eq!(code(response), v1::ErrorCode::VersionConflict as i32);
    }
    panic!("sync did not stabilize")
}
async fn tick(h: &Harness, room: Uuid, epoch: Uuid, ms: i64, policy: RecoveryConfig) {
    h.db.observe_room(Observation {
        room,
        epoch,
        tick: Uuid::new_v4(),
        elapsed_ms: ms,
        started: Instant::now(),
        presence: h.registry.send(Snapshot).await.unwrap(),
        policy,
    })
    .await
    .unwrap();
}
async fn playable(h: &Harness, a: &Player, b: &Player, r: &v1::RoomSnapshot, epoch: Uuid) {
    let id = Uuid::parse_str(&r.game_id).unwrap();
    sync(h, a, id).await;
    sync(h, b, id).await;
    tick(
        h,
        Uuid::parse_str(&r.room_id).unwrap(),
        epoch,
        0,
        RecoveryConfig::default(),
    )
    .await;
}
fn req(
    g: &backend::persistence::friends::GameView,
    action: v1::game_request::Action,
) -> v1::GameRequest {
    v1::GameRequest {
        game_id: g.id.to_string(),
        expected_version: g.version as u64,
        action: Some(action),
    }
}
fn buy(id: u32, x: i32, y: i32, turns: u32) -> v1::game_request::Action {
    v1::game_request::Action::BuyAndPlace(v1::PlacePatch {
        patch_id: id.to_string(),
        position: Some(v1::BoardPosition { x, y }),
        quarter_turns: turns,
        flipped: false,
    })
}
fn advance() -> v1::game_request::Action {
    v1::game_request::Action::Advance(v1::Empty {})
}
async fn game_send(
    h: &Harness,
    p: &Player,
    request: v1::GameRequest,
    id: &str,
) -> v1::ServerEnvelope {
    h.registry
        .send(Dispatch {
            stamp: p.stamp,
            message: v1::ClientEnvelope {
                protocol_version: VERSION,
                request_id: id.into(),
                payload: Some(v1::client_envelope::Payload::Game(request)),
            },
        })
        .await
        .unwrap()
        .unwrap()
}
fn accepted(response: v1::ServerEnvelope) -> v1::Acknowledged {
    match response.payload {
        Some(Payload::Acknowledged(a)) => a,
        Some(Payload::Error(e)) => panic!("game error {}", e.code),
        _ => panic!("expected ack"),
    }
}
async fn mutation(
    h: &Harness,
    p: &Player,
    room: Uuid,
    request: v1::GameRequest,
    id: &str,
) -> RoomMutation {
    let presence = h.registry.send(Snapshot).await.unwrap();
    RoomMutation {
        operation: Uuid::new_v4(),
        request_id: id.into(),
        room,
        expected_version: request.expected_version as i64,
        fingerprint: fingerprint(p.stamp.user, &request),
        permit: presence.online[&p.stamp.user].clone(),
        online: presence.online.clone(),
        action: Mutation::Game { request, presence },
    }
}
fn drain(p: &mut Player) {
    while p.push.try_recv().is_ok() {}
}

async fn commit_without_reply(h: &Harness, m: RoomMutation) {
    let path = std::env::var("PATCHWORK_TEST_CONFIG").unwrap();
    let config = backend::config::Config::load(std::path::Path::new(&path))
        .unwrap()
        .database
        .unwrap();
    let (port, armed, proxy) = pg_proxy::start(config.port).await;
    let relay = db_with(false, |c| {
        c.port = port;
        c.max_connections = 1;
    })
    .await;
    armed.store(true, Ordering::SeqCst);
    assert_eq!(
        relay.friend_mutate(m.clone()).await.unwrap_err(),
        E::CommitUnknown
    );
    assert!(h.db.friend_recover(m).await.unwrap().into_inner().is_some());
    relay.close().await;
    proxy.abort();
}

#[actix_web::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn real_start_is_persisted_once_and_wire_actions_are_fenced() {
    let (h, epoch) = rig(db().await).await;
    let (mut a, mut b, r, start_request, start_id) = start(&h).await;
    let room_id = Uuid::parse_str(&r.room_id).unwrap();
    let g = read(&h, room_id).await;
    let s = core(&g);
    assert_eq!(s.player(Seat::First).user_id(), a.stamp.user.to_string());
    assert_eq!(s.player(Seat::Second).user_id(), b.stamp.user.to_string());
    assert_eq!(s.supply().remaining_count(), 33);
    assert_eq!(s.supply().candidates()[0], PatchId(10));
    assert_eq!(
        s.first_player().index() as u32,
        r.first_player_seat.unwrap()
    );
    assert_eq!(
        room(send(&h, &a, start_request, &start_id).await).encode_to_vec(),
        r.encode_to_vec()
    );
    assert_eq!(core(&read(&h, room_id).await).supply(), s.supply());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM patchwork.games WHERE room_id=$1")
            .bind(room_id)
            .fetch_one(h.db.pool())
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        code(game_send(&h, &a, req(&g, advance()), "needs-sync").await),
        v1::ErrorCode::SyncRequired as i32
    );
    playable(&h, &a, &b, &r, epoch).await;
    let g = read(&h, room_id).await;
    let (actor, other) = if core(&g).input_actor() == Some(Seat::First) {
        (&a, &b)
    } else {
        (&b, &a)
    };
    assert_eq!(
        code(game_send(&h, other, req(&g, advance()), "wrong-turn").await),
        v1::ErrorCode::NotYourTurn as i32
    );
    for (action, expected) in [
        (buy(10, 9, 0, 0), v1::ErrorCode::InvalidPlacement),
        (buy(10, 0, 0, u32::MAX), v1::ErrorCode::InvalidRequest),
        (buy(10, 0, 0, 4), v1::ErrorCode::InvalidRequest),
        (buy(0, 0, 0, 0), v1::ErrorCode::InvalidRequest),
    ] {
        assert_eq!(
            code(game_send(&h, actor, req(&g, action), &Uuid::new_v4().to_string()).await),
            expected as i32
        );
        assert_eq!(read(&h, room_id).await.state, g.state);
    }
    let request = req(&g, buy(10, 0, 0, 0));
    let reply = accepted(game_send(&h, actor, request.clone(), "purchase").await);
    assert_eq!(reply.game_version, g.version as u64 + 1);
    assert_eq!(
        accepted(game_send(&h, actor, request.clone(), "purchase").await).encode_to_vec(),
        reply.encode_to_vec()
    );
    assert_eq!(
        code(game_send(&h, actor, req(&g, advance()), "purchase").await),
        v1::ErrorCode::RequestIdConflict as i32
    );
    assert_eq!(
        code(game_send(&h, actor, req(&g, advance()), "stale").await),
        v1::ErrorCode::VersionConflict as i32
    );
    let after = read(&h, room_id).await;
    let expected = s
        .apply_action(
            actor.stamp.user.to_string().as_str(),
            game_core::actions::GameAction::BuyAndPlace {
                patch_id: PatchId(10),
                position: BoardPosition::new(0, 0).unwrap(),
                orientation: Orientation::default(),
            },
        )
        .unwrap()
        .state;
    assert_eq!(core(&after), expected);
    let old = actor.stamp;
    let replacement = connect(&h, actor.stamp.user, actor.session).await;
    assert_eq!(
        h.registry
            .send(Dispatch {
                stamp: old,
                message: v1::ClientEnvelope {
                    protocol_version: VERSION,
                    request_id: "old-generation".into(),
                    payload: Some(v1::client_envelope::Payload::Game(request))
                }
            })
            .await
            .unwrap()
            .err()
            .unwrap(),
        E::Permission
    );
    sync(&h, &replacement, after.id).await;
    assert_eq!(read(&h, room_id).await.state, after.state);
    drain(&mut a);
    drain(&mut b);
}

#[actix_web::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn real_game_events_replay_and_natural_result_commits_once() {
    let (h, epoch) = rig(db().await).await;
    let (mut a, mut b, r, _, _) = start(&h).await;
    let room_id = Uuid::parse_str(&r.room_id).unwrap();
    playable(&h, &a, &b, &r, epoch).await;
    let mut purchases = 0;
    let mut commands = 0;
    let mut last = None;
    for step in 0..112 {
        let g = read(&h, room_id).await;
        let state = core(&g);
        if state.result().is_some() {
            break;
        }
        let seat = state.input_actor().unwrap();
        let actor = if seat == Seat::First { &a } else { &b };
        let action = match state.action_phase() {
            ActionPhase::Special { .. } => {
                let position = (0..9)
                    .flat_map(|y| (0..9).map(move |x| BoardPosition::new(x, y).unwrap()))
                    .find(|&p| state.player(seat).board().at(p).is_none())
                    .unwrap();
                let (x, y) = position.coordinates();
                v1::game_request::Action::PlaceSpecialPatch(v1::BoardPosition {
                    x: i32::from(x),
                    y: i32::from(y),
                })
            }
            _ => {
                let mut chosen = None;
                'candidate: for id in state.supply().candidates() {
                    if u32::from(patch(id).unwrap().button_cost) > state.player(seat).buttons() {
                        continue;
                    }
                    for turns in 0..4 {
                        for y in 0..9 {
                            for x in 0..9 {
                                let action = buy(id.0, x, y, turns);
                                let intent = backend::persistence::gameplay::action(&req(
                                    &g,
                                    action.clone(),
                                ))
                                .unwrap()
                                .unwrap();
                                if state
                                    .apply_action(&actor.stamp.user.to_string(), intent)
                                    .is_ok()
                                {
                                    chosen = Some(action);
                                    break 'candidate;
                                }
                            }
                        }
                    }
                }
                chosen.unwrap_or_else(advance)
            }
        };
        purchases += usize::from(matches!(action, v1::game_request::Action::BuyAndPlace(_)));
        let request = req(&g, action);
        let expected = state
            .apply_action(
                &actor.stamp.user.to_string(),
                backend::persistence::gameplay::action(&request)
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
        let id = format!("full-game-{step}");
        if expected.state.result().is_some() {
            // The final action commits, but the transport loses COMMIT's response.
            // The ordinary actor retry below must recover the original result.
            commit_without_reply(&h, mutation(&h, actor, room_id, request.clone(), &id).await)
                .await;
        }
        let ack = accepted(game_send(&h, actor, request.clone(), &id).await);
        commands += 1;
        let stored = read(&h, room_id).await;
        assert_eq!(core(&stored), expected.state);
        assert_eq!(stored.version as u64, ack.game_version);
        let presence = h.registry.send(Snapshot).await.unwrap();
        let tail =
            h.db.resume_game(
                presence.online[&actor.stamp.user].clone(),
                v1::ResumeRequest {
                    game_id: g.id.to_string(),
                    last_seq: g.seq as u64,
                    has_snapshot: true,
                },
            )
            .await
            .unwrap();
        assert_eq!(tail.events.len(), 1);
        assert!(tail.snapshot.is_none());
        let event: serde_json::Value =
            serde_json::from_slice(&tail.events[0].payload_json).unwrap();
        assert_eq!(event["kind"], "game_transition_v1");
        assert_eq!(event["state"], stored.state);
        assert_eq!(
            event["events"],
            serde_json::to_value(expected.events).unwrap()
        );
        last = Some((actor.stamp.user, request, id, ack));
        drain(&mut a);
        drain(&mut b);
    }
    assert!(purchases > 0);
    let stored = read(&h, room_id).await;
    let state = core(&stored);
    let result = state.result().unwrap();
    assert_eq!(h.db.friend_room(room_id).await.unwrap().phase, "finished");
    let result_row =
        sqlx::query("SELECT score0,score1,reason FROM patchwork.game_results WHERE game_id=$1")
            .bind(stored.id)
            .fetch_one(h.db.pool())
            .await
            .unwrap();
    assert_eq!(
        result_row.get::<i32, _>("score0") as u64,
        result.scores[0].total
    );
    assert_eq!(
        result_row.get::<i32, _>("score1") as u64,
        result.scores[1].total
    );
    assert_eq!(result_row.get::<String, _>("reason"), "completed");
    let (user, request, id, ack) = last.unwrap();
    let actor = if user == a.stamp.user { &a } else { &b };
    assert_eq!(
        accepted(game_send(&h, actor, request, &id).await).encode_to_vec(),
        ack.encode_to_vec()
    );
    assert_eq!(read(&h, room_id).await, stored);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM patchwork.command_receipts WHERE game_id=$1"
        )
        .bind(stored.id)
        .fetch_one(h.db.pool())
        .await
        .unwrap(),
        commands
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM patchwork.game_results WHERE game_id=$1"
        )
        .bind(stored.id)
        .fetch_one(h.db.pool())
        .await
        .unwrap(),
        1
    );
}

#[actix_web::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn restart_preserves_special_queue_and_resignation_uses_actual_scores() {
    let (h, epoch) = rig(db().await).await;
    let (mut a, mut b, r, _, _) = start(&h).await;
    let room_id = Uuid::parse_str(&r.room_id).unwrap();
    playable(&h, &a, &b, &r, epoch).await;
    let saved = loop {
        let g = read(&h, room_id).await;
        if !core(&g).action().pending_specials().is_empty() {
            break g;
        }
        let actor = if core(&g).input_actor() == Some(Seat::First) {
            &a
        } else {
            &b
        };
        accepted(game_send(&h, actor, req(&g, advance()), &Uuid::new_v4().to_string()).await);
        drain(&mut a);
        drain(&mut b);
    };
    let (restarted, new_epoch) = rig(h.db.clone()).await;
    let paused = read(&restarted, room_id).await;
    let expected = core(&saved).with_connection_pause(true).unwrap();
    assert_eq!(core(&paused), expected);
    let a2 = connect(&restarted, a.stamp.user, a.session).await;
    let b2 = connect(&restarted, b.stamp.user, b.session).await;
    playable(&restarted, &a2, &b2, &r, new_epoch).await;
    let restored = read(&restarted, room_id).await;
    assert_eq!(core(&restored), core(&saved));
    let actor = if core(&restored).input_actor() == Some(Seat::First) {
        &a2
    } else {
        &b2
    };
    accepted(
        game_send(
            &restarted,
            actor,
            req(
                &restored,
                v1::game_request::Action::PlaceSpecialPatch(v1::BoardPosition { x: 0, y: 0 }),
            ),
            "restored-special",
        )
        .await,
    );
    let g = read(&restarted, room_id).await;
    let request = req(&g, v1::game_request::Action::Resign(v1::Empty {}));
    let ack = accepted(game_send(&restarted, &b2, request.clone(), "resign").await);
    assert_eq!(
        accepted(game_send(&restarted, &b2, request, "resign").await).encode_to_vec(),
        ack.encode_to_vec()
    );
    let final_game = core(&read(&restarted, room_id).await);
    assert_eq!(final_game.lifecycle(), Lifecycle::Finished);
    assert_eq!(
        final_game.result().unwrap().outcome,
        game_core::state::Outcome::Won {
            winner: Seat::First
        }
    );
    assert_eq!(
        final_game.result().unwrap().scores[0].total,
        u64::from(final_game.player(Seat::First).buttons())
    );
    let row =
        sqlx::query("SELECT score0,score1,reason FROM patchwork.game_results WHERE game_id=$1")
            .bind(g.id)
            .fetch_one(h.db.pool())
            .await
            .unwrap();
    assert_eq!(row.get::<String, _>("reason"), "resigned");
    assert_eq!(
        row.get::<i32, _>("score0") as u64,
        final_game.result().unwrap().scores[0].total
    );
}

#[actix_web::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn unknown_commit_and_database_outage_resolve_without_reapplying_actions() {
    let (h, epoch) = rig(db().await).await;
    let (a, b, r, _, _) = start(&h).await;
    let room_id = Uuid::parse_str(&r.room_id).unwrap();
    playable(&h, &a, &b, &r, epoch).await;
    let g = read(&h, room_id).await;
    let actor = if core(&g).input_actor() == Some(Seat::First) {
        &a
    } else {
        &b
    };
    let request = req(&g, buy(10, 0, 0, 0));
    let m = mutation(&h, actor, room_id, request.clone(), "lost-purchase-ack").await;
    let path = std::env::var("PATCHWORK_TEST_CONFIG").unwrap();
    let config = backend::config::Config::load(std::path::Path::new(&path))
        .unwrap()
        .database
        .unwrap();
    let (port, armed, proxy) = pg_proxy::start(config.port).await;
    let relay = db_with(false, |c| {
        c.port = port;
        c.max_connections = 1;
    })
    .await;
    armed.store(true, Ordering::SeqCst);
    assert_eq!(
        relay.friend_mutate(m.clone()).await.unwrap_err(),
        E::CommitUnknown
    );
    let original =
        h.db.friend_recover(m.clone())
            .await
            .unwrap()
            .into_inner()
            .unwrap();
    assert_eq!(
        h.db.friend_mutate(m.clone()).await.unwrap().into_inner(),
        original
    );
    assert_eq!(
        core(&original.game.unwrap())
            .player(core(&g).input_actor().unwrap())
            .buttons(),
        3
    );
    assert_eq!(read(&h, room_id).await.version, g.version + 1);
    let mut conflict = m;
    let altered = req(&g, advance());
    conflict.fingerprint = fingerprint(actor.stamp.user, &altered);
    if let Mutation::Game { request, .. } = &mut conflict.action {
        *request = altered;
    }
    assert_eq!(
        h.db.friend_mutate(conflict).await.unwrap_err(),
        E::RequestIdConflict
    );
    relay.close().await;
    proxy.abort();

    let g = read(&h, room_id).await;
    let actor = if core(&g).input_actor() == Some(Seat::First) {
        &a
    } else {
        &b
    };
    let m = mutation(&h, actor, room_id, req(&g, advance()), "after-outage").await;
    let unavailable = db_with(false, |c| {
        c.statement_timeout_ms = 100;
        c.max_connections = 1;
    })
    .await;
    let mut lock = h.db.pool().begin().await.unwrap();
    sqlx::query("SELECT room_id FROM patchwork.rooms WHERE room_id=$1 FOR UPDATE")
        .bind(room_id)
        .execute(&mut *lock)
        .await
        .unwrap();
    assert_eq!(
        unavailable.friend_mutate(m.clone()).await.unwrap_err(),
        E::Unavailable
    );
    lock.rollback().await.unwrap();
    assert!(
        h.db.friend_recover(m.clone())
            .await
            .unwrap()
            .into_inner()
            .is_none()
    );
    assert_eq!(read(&h, room_id).await, g);
    assert_eq!(
        h.db.friend_mutate(m)
            .await
            .unwrap()
            .into_inner()
            .game
            .unwrap()
            .version,
        g.version + 1
    );
}

#[actix_web::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn concurrent_game_requests_accept_exactly_one_version() {
    let (h, epoch) = rig(db().await).await;
    let (a, b, r, _, _) = start(&h).await;
    let room_id = Uuid::parse_str(&r.room_id).unwrap();
    playable(&h, &a, &b, &r, epoch).await;
    let g = read(&h, room_id).await;
    let actor = if core(&g).input_actor() == Some(Seat::First) {
        &a
    } else {
        &b
    };
    let m1 = mutation(&h, actor, room_id, req(&g, advance()), "concurrent-1").await;
    let m2 = mutation(&h, actor, room_id, req(&g, advance()), "concurrent-2").await;
    let (r1, r2) = tokio::join!(h.db.friend_mutate(m1), h.db.friend_mutate(m2));
    let results = [r1, r2];
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results.into_iter().find_map(Result::err),
        Some(E::VersionConflict)
    );
    let after = read(&h, room_id).await;
    assert_eq!(after.version, g.version + 1);
    assert_eq!(after.seq, g.seq + 1);
    assert_eq!(
        core(&after),
        core(&g)
            .apply_action(
                &actor.stamp.user.to_string(),
                game_core::actions::GameAction::Advance
            )
            .unwrap()
            .state
    );
}

#[actix_web::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn real_timeouts_and_abandonment_preserve_scores_and_unique_results() {
    for both_offline in [false, true] {
        let (h, epoch) = rig(db().await).await;
        let (a, b, r, _, _) = start(&h).await;
        let room_id = Uuid::parse_str(&r.room_id).unwrap();
        playable(&h, &a, &b, &r, epoch).await;
        let g = read(&h, room_id).await;
        let actor = if core(&g).input_actor() == Some(Seat::First) {
            &a
        } else {
            &b
        };
        accepted(game_send(&h, actor, req(&g, advance()), "before-timeout").await);
        let saved = core(&read(&h, room_id).await);
        h.registry.send(Disconnect(a.stamp)).await.unwrap();
        if both_offline {
            h.registry.send(Disconnect(b.stamp)).await.unwrap();
        }
        let policy = RecoveryConfig {
            waiting_grace_secs: 2,
            game_budget_secs: 3,
            both_offline_retention_secs: 4,
            restart_grace_secs: 2,
        };
        tick(&h, room_id, epoch, 0, policy.clone()).await;
        tick(&h, room_id, epoch, 2000, policy.clone()).await;
        tick(&h, room_id, epoch, 2000, policy.clone()).await;
        let g = read(&h, room_id).await;
        let reason = if both_offline {
            game_core::state::ResultReason::Abandoned
        } else {
            game_core::state::ResultReason::Forfeit { loser: Seat::First }
        };
        assert_eq!(core(&g), saved.terminate(reason).unwrap().state);
        let result = core(&g).result().unwrap().clone();
        let row = sqlx::query(
            "SELECT score0,score1,reason,winner_seat FROM patchwork.game_results WHERE game_id=$1",
        )
        .bind(g.id)
        .fetch_one(h.db.pool())
        .await
        .unwrap();
        assert_eq!(row.get::<i32, _>("score0") as u64, result.scores[0].total);
        assert_eq!(row.get::<i32, _>("score1") as u64, result.scores[1].total);
        assert_eq!(
            row.get::<String, _>("reason"),
            if both_offline { "abandoned" } else { "timeout" }
        );
        assert_eq!(
            row.get::<Option<i16>, _>("winner_seat"),
            if both_offline { None } else { Some(1) }
        );
        tick(&h, room_id, epoch, 2000, policy).await;
        assert_eq!(read(&h, room_id).await, g);
    }
}
