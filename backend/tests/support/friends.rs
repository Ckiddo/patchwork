use super::{db, db_with, pg_proxy};
use actix::{Actor, Addr};
use backend::{
    AppState,
    game::{LobbyManager, RoomsReady},
    identity,
    persistence::Database,
    sessions::{Attach, Disconnect, Dispatch, Register, SessionRegistry, Stamp},
};
use prost::Message;
use tokio::sync::{mpsc, watch};
use util_lib::protocol::{
    VERSION,
    v1::{self, lobby_request::Command, server_envelope::Payload},
};
use uuid::Uuid;
#[path = "gameplay.rs"]
mod gameplay;
#[path = "recovery.rs"]
mod recovery;
struct Harness {
    db: Database,
    registry: Addr<SessionRegistry>,
    lobby: Addr<LobbyManager>,
}
struct Player {
    stamp: Stamp,
    session: Uuid,
    _kick: watch::Receiver<bool>,
    push: mpsc::Receiver<v1::ServerEnvelope>,
}
async fn harness(database: Database) -> Harness {
    let lobby = LobbyManager::default().start();
    let state = AppState::new(b"isolated-friend-room-test-key", lobby.clone())
        .with_database(database.clone());
    assert!(lobby.send(RoomsReady).await.unwrap());
    Harness {
        db: database,
        registry: state.registry.clone(),
        lobby,
    }
}
async fn player(h: &Harness) -> Player {
    let session = Uuid::new_v4();
    let profile =
        h.db.create_identity_session(
            session,
            identity::token_hash(&identity::random_token()).unwrap(),
            None,
        )
        .await
        .unwrap();
    connect(h, profile.id, session).await
}
async fn connect(h: &Harness, user: Uuid, session: Uuid) -> Player {
    let (kick, rx) = watch::channel(false);
    let stamp = h
        .registry
        .send(Register {
            database: h.db.clone(),
            user,
            session,
            auth_version: 0,
            expires: chrono::Utc::now().timestamp() + 300,
            kick,
        })
        .await
        .unwrap()
        .unwrap();
    let (push, receiver) = mpsc::channel(128);
    assert!(h.registry.send(Attach { stamp, push }).await.unwrap());
    Player {
        stamp,
        session,
        _kick: rx,
        push: receiver,
    }
}
fn command(room: &str, version: u64, command: Command) -> v1::LobbyRequest {
    v1::LobbyRequest {
        room_id: room.into(),
        expected_version: version,
        command: Some(command),
    }
}
async fn send(h: &Harness, p: &Player, request: v1::LobbyRequest, id: &str) -> v1::ServerEnvelope {
    h.registry
        .send(Dispatch {
            stamp: p.stamp,
            message: v1::ClientEnvelope {
                protocol_version: VERSION,
                request_id: id.into(),
                payload: Some(v1::client_envelope::Payload::Lobby(request)),
            },
        })
        .await
        .unwrap()
        .unwrap()
}
async fn call(h: &Harness, p: &Player, request: v1::LobbyRequest) -> v1::ServerEnvelope {
    send(h, p, request, &Uuid::new_v4().to_string()).await
}
fn room(response: v1::ServerEnvelope) -> v1::RoomSnapshot {
    match response.payload {
        Some(Payload::Room(r)) => r,
        Some(Payload::Error(e)) => panic!("expected room, got error code {}", e.code),
        _ => panic!("expected room"),
    }
}
fn code(response: v1::ServerEnvelope) -> i32 {
    match response.payload {
        Some(Payload::Error(e)) => e.code,
        _ => panic!("expected error"),
    }
}
async fn create(h: &Harness, p: &Player, password: Option<&str>) -> v1::RoomSnapshot {
    room(
        call(
            h,
            p,
            command(
                "",
                0,
                Command::Create(v1::CreateRoom {
                    mode: "casual".into(),
                    rules_version: "v1".into(),
                    password: password.map(str::to_string),
                }),
            ),
        )
        .await,
    )
}
async fn join(h: &Harness, p: &Player, r: &v1::RoomSnapshot) -> v1::RoomSnapshot {
    room(
        call(
            h,
            p,
            command(
                &r.room_id,
                r.version,
                Command::Join(v1::JoinRoom {
                    code: r.code.clone(),
                    password: None,
                }),
            ),
        )
        .await,
    )
}
async fn ready(h: &Harness, p: &Player, r: &v1::RoomSnapshot) -> v1::RoomSnapshot {
    room(
        call(
            h,
            p,
            command(
                &r.room_id,
                r.version,
                Command::SetReady(v1::SetReady { ready: true }),
            ),
        )
        .await,
    )
}
async fn game_push(p: &mut Player) -> v1::GameSnapshot {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if let Some(Payload::Game(g)) = p.push.recv().await.expect("push channel").payload {
                return g;
            }
        }
    })
    .await
    .expect("missing game push")
}

#[actix_web::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn registered_rules_gate_creation_updates_and_both_start_paths() {
    use backend::persistence::StoreError;
    use game_core::rules::{CUSTOM_RULES_VERSION, registry::*};
    let h = harness(db().await).await;
    let mut a = player(&h).await;
    let b = player(&h).await;
    let request = command(
        "",
        0,
        Command::Create(v1::CreateRoom {
            mode: "casual".into(),
            rules_version: "unregistered_rules".into(),
            password: None,
        }),
    );
    assert_eq!(
        code(call(&h, &a, request).await),
        v1::ErrorCode::InvalidRequest as i32
    );
    let occupancy: String =
        sqlx::query_scalar("SELECT state FROM patchwork.player_occupancy WHERE user_id=$1")
            .bind(a.stamp.user)
            .fetch_one(h.db.pool())
            .await
            .unwrap();
    assert_eq!(occupancy, "idle");

    let r = create(&h, &a, None).await;
    let r = join(&h, &b, &r).await;
    let r = ready(&h, &a, &r).await;
    let r = ready(&h, &b, &r).await;
    let room_id = Uuid::parse_str(&r.room_id).unwrap();
    let before = h.db.friend_room(room_id).await.unwrap();
    assert_eq!(
        code(
            call(
                &h,
                &a,
                command(
                    &r.room_id,
                    r.version,
                    Command::SetRules(v1::SetRules {
                        rules_version: "v999".into()
                    })
                )
            )
            .await
        ),
        v1::ErrorCode::InvalidRequest as i32
    );
    assert_eq!(h.db.friend_room(room_id).await.unwrap(), before);

    let r = room(
        call(
            &h,
            &a,
            command(
                &r.room_id,
                r.version,
                Command::SetRules(v1::SetRules {
                    rules_version: CUSTOM_RULES_VERSION.into(),
                }),
            ),
        )
        .await,
    );
    assert!(r.members.iter().all(|m| !m.ready));
    let r = ready(&h, &a, &r).await;
    let r = ready(&h, &b, &r).await;
    let before = h.db.friend_room(room_id).await.unwrap();
    assert_eq!(
        h.db.begin_game(
            room_id,
            Uuid::new_v4(),
            r.version as i64,
            serde_json::json!({"kind":LEGACY_SNAPSHOT_KIND})
        )
        .await
        .unwrap_err(),
        StoreError::InvalidInput
    );
    assert_eq!(h.db.friend_room(room_id).await.unwrap(), before);

    // Pre-registration waiting rooms may carry arbitrary labels. Recheck the stored rule at start.
    sqlx::query(
        "UPDATE patchwork.rooms SET rules_version='old-unregistered-label' WHERE room_id=$1",
    )
    .bind(room_id)
    .execute(h.db.pool())
    .await
    .unwrap();
    assert_eq!(
        code(
            call(
                &h,
                &a,
                command(&r.room_id, r.version, Command::Start(v1::Empty {}))
            )
            .await
        ),
        v1::ErrorCode::InvalidRequest as i32
    );
    assert_eq!(
        h.db.begin_game(
            room_id,
            Uuid::new_v4(),
            r.version as i64,
            serde_json::json!({})
        )
        .await
        .unwrap_err(),
        StoreError::InvalidInput
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM patchwork.games WHERE room_id=$1")
        .bind(room_id)
        .fetch_one(h.db.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
    let current = h.db.friend_room(room_id).await.unwrap();
    assert_eq!(current.version, r.version as i64);
    assert_eq!(current.phase, "waiting");
    assert!(current.members.iter().all(|m| m.ready));

    // The existing empty-game path still works after deliberately selecting a preview rule.
    let r = room(
        call(
            &h,
            &a,
            command(
                &r.room_id,
                r.version,
                Command::SetRules(v1::SetRules {
                    rules_version: PREVIEW_VERSION.into(),
                }),
            ),
        )
        .await,
    );
    let r = ready(&h, &a, &r).await;
    let r = ready(&h, &b, &r).await;
    let r = room(
        call(
            &h,
            &a,
            command(&r.room_id, r.version, Command::Start(v1::Empty {})),
        )
        .await,
    );
    assert!(!r.game_id.is_empty());
    let game = game_push(&mut a).await;
    let state: serde_json::Value = serde_json::from_slice(&game.state_json).unwrap();
    assert_eq!(state["rules_implemented"], false);
    assert_eq!(
        snapshot_format(&game.rules_version, state["kind"].as_str().unwrap(), None),
        Ok(SnapshotFormat::LegacyPreview)
    );
}

#[actix_web::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn passwords_membership_reset_and_single_player_start_are_enforced() {
    let h = harness(db().await).await;
    let a = player(&h).await;
    let b = player(&h).await;
    let c = player(&h).await;
    let r = create(&h, &a, Some("friend-password-only-test")).await;
    assert!(r.requires_password);
    assert_eq!(r.members.len(), 1);
    assert_eq!(r.members[0].seat, 0);
    assert_eq!(
        code(
            call(
                &h,
                &a,
                command(&r.room_id, r.version, Command::Start(v1::Empty {}))
            )
            .await
        ),
        v1::ErrorCode::NotEnoughPlayers as i32
    );
    assert_eq!(
        code(
            call(
                &h,
                &b,
                command(
                    &r.room_id,
                    r.version,
                    Command::SetReady(v1::SetReady { ready: true })
                )
            )
            .await
        ),
        v1::ErrorCode::Forbidden as i32
    );
    assert_eq!(
        code(
            call(
                &h,
                &b,
                command(
                    &r.room_id,
                    r.version,
                    Command::Join(v1::JoinRoom {
                        code: r.code.clone(),
                        password: Some("wrong".into())
                    })
                )
            )
            .await
        ),
        v1::ErrorCode::BadPassword as i32
    );
    let r = ready(&h, &a, &r).await;
    let r = room(
        call(
            &h,
            &b,
            command(
                "",
                0,
                Command::Join(v1::JoinRoom {
                    code: r.code.to_lowercase(),
                    password: Some("friend-password-only-test".into()),
                }),
            ),
        )
        .await,
    );
    assert_eq!(r.members.len(), 2);
    assert!(r.members.iter().all(|m| !m.ready));
    assert_eq!(
        code(
            call(
                &h,
                &c,
                command(
                    &r.room_id,
                    r.version,
                    Command::Join(v1::JoinRoom {
                        code: r.code.clone(),
                        password: Some("friend-password-only-test".into())
                    })
                )
            )
            .await
        ),
        v1::ErrorCode::RoomFull as i32
    );
    let hash: String =
        sqlx::query_scalar("SELECT password_hash FROM patchwork.rooms WHERE room_id=$1")
            .bind(Uuid::parse_str(&r.room_id).unwrap())
            .fetch_one(h.db.pool())
            .await
            .unwrap();
    assert!(hash.starts_with("$argon2id$"));
    assert!(!hash.contains("friend-password-only-test"));
    let receipts: String = sqlx::query_scalar(
        "SELECT string_agg(response::text,'') FROM patchwork.operation_receipts WHERE user_id=$1",
    )
    .bind(a.stamp.user)
    .fetch_one(h.db.pool())
    .await
    .unwrap();
    assert!(!receipts.contains("password_hash") && !receipts.contains("friend-password-only-test"));
}

#[actix_web::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn ready_retries_rules_changes_and_owner_transfer_preserve_seats() {
    let h = harness(db().await).await;
    let a = player(&h).await;
    let b = player(&h).await;
    let c = player(&h).await;
    let r = create(&h, &a, None).await;
    let r = join(&h, &b, &r).await;
    let id = Uuid::new_v4().to_string();
    let req = command(
        &r.room_id,
        r.version,
        Command::SetReady(v1::SetReady { ready: true }),
    );
    let first = send(&h, &a, req.clone(), &id).await;
    let repeated = send(&h, &a, req.clone(), &id).await;
    assert_eq!(first.encode_to_vec(), repeated.encode_to_vec());
    assert_eq!(
        code(
            send(
                &h,
                &a,
                command(
                    &r.room_id,
                    r.version,
                    Command::SetReady(v1::SetReady { ready: false })
                ),
                &id
            )
            .await
        ),
        v1::ErrorCode::RequestIdConflict as i32
    );
    let r = ready(&h, &b, &room(first)).await;
    let r = room(
        call(
            &h,
            &a,
            command(
                &r.room_id,
                r.version,
                Command::SetRules(v1::SetRules {
                    rules_version: game_core::rules::registry::PREVIEW_VERSION.into(),
                }),
            ),
        )
        .await,
    );
    assert!(r.members.iter().all(|m| !m.ready));
    let r = ready(&h, &b, &r).await;
    let r = room(
        call(
            &h,
            &a,
            command(&r.room_id, r.version, Command::Leave(v1::Empty {})),
        )
        .await,
    );
    assert_eq!(r.owner_id, b.stamp.user.to_string());
    assert_eq!(r.members[0].seat, 1);
    assert!(!r.members[0].ready);
    let r = join(&h, &c, &r).await;
    assert_eq!(r.owner_id, b.stamp.user.to_string());
    assert_eq!(r.members[0].user_id, c.stamp.user.to_string());
    let r = room(
        call(
            &h,
            &c,
            command(&r.room_id, r.version, Command::Leave(v1::Empty {})),
        )
        .await,
    );
    let r = room(
        call(
            &h,
            &b,
            command(&r.room_id, r.version, Command::Leave(v1::Empty {})),
        )
        .await,
    );
    assert_eq!(r.phase, v1::RoomPhase::Closed as i32);
    assert!(r.owner_id.is_empty() && r.members.is_empty());
    let states: Vec<String> =
        sqlx::query_scalar("SELECT state FROM patchwork.player_occupancy WHERE user_id=ANY($1)")
            .bind(vec![a.stamp.user, b.stamp.user, c.stamp.user])
            .fetch_all(h.db.pool())
            .await
            .unwrap();
    assert!(states.iter().all(|s| s == "idle"));
}

#[actix_web::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn concurrent_last_seat_and_same_user_two_rooms_have_one_winner() {
    let h = harness(db().await).await;
    let a = player(&h).await;
    let b = player(&h).await;
    let c = player(&h).await;
    let r = create(&h, &a, None).await;
    let req = command(
        &r.room_id,
        r.version,
        Command::Join(v1::JoinRoom {
            code: r.code.clone(),
            password: None,
        }),
    );
    let (x, y) = tokio::join!(call(&h, &b, req.clone()), call(&h, &c, req));
    assert_eq!(
        [&x, &y]
            .iter()
            .filter(|r| matches!(r.payload, Some(Payload::Room(_))))
            .count(),
        1
    );
    let loser = if matches!(x.payload, Some(Payload::Room(_))) {
        &c
    } else {
        &b
    };
    let d = player(&h).await;
    let e = player(&h).await;
    let r1 = create(&h, &d, None).await;
    let r2 = create(&h, &e, None).await;
    let (x, y) = tokio::join!(
        call(
            &h,
            loser,
            command(
                &r1.room_id,
                0,
                Command::Join(v1::JoinRoom {
                    code: r1.code,
                    password: None
                })
            )
        ),
        call(
            &h,
            loser,
            command(
                &r2.room_id,
                0,
                Command::Join(v1::JoinRoom {
                    code: r2.code,
                    password: None
                })
            )
        )
    );
    assert_eq!(
        [&x, &y]
            .iter()
            .filter(|r| matches!(r.payload, Some(Payload::Room(_))))
            .count(),
        1
    );
    let memberships: i64 =
        sqlx::query_scalar("SELECT count(*) FROM patchwork.room_members WHERE user_id=$1")
            .bind(loser.stamp.user)
            .fetch_one(h.db.pool())
            .await
            .unwrap();
    assert_eq!(memberships, 1);
}

#[actix_web::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn all_members_receive_identical_game_and_duplicate_start_is_idempotent() {
    let h = harness(db().await).await;
    let mut a = player(&h).await;
    let mut b = player(&h).await;
    let c = player(&h).await;
    let r = create(&h, &a, None).await;
    let r = join(&h, &b, &r).await;
    assert_eq!(
        code(
            call(
                &h,
                &a,
                command(&r.room_id, r.version, Command::Start(v1::Empty {}))
            )
            .await
        ),
        v1::ErrorCode::NotReady as i32
    );
    let r = ready(&h, &a, &r).await;
    let r = ready(&h, &b, &r).await;
    let req = command(&r.room_id, r.version, Command::Start(v1::Empty {}));
    let id = Uuid::new_v4().to_string();
    let started = send(&h, &a, req.clone(), &id).await;
    let repeated = send(&h, &a, req, &id).await;
    assert_eq!(started.encode_to_vec(), repeated.encode_to_vec());
    let r = room(started);
    let g1 = game_push(&mut a).await;
    let g2 = game_push(&mut b).await;
    assert_eq!(g1.encode_to_vec(), g2.encode_to_vec());
    assert_eq!(g1.game_id, r.game_id);
    let snapshot: serde_json::Value = serde_json::from_slice(&g1.state_json).unwrap();
    assert_eq!(snapshot["players"][0]["user_id"], a.stamp.user.to_string());
    assert_eq!(snapshot["players"][1]["user_id"], b.stamp.user.to_string());
    assert!(snapshot["first_player_seat"].as_u64().unwrap() < 2);
    assert_eq!(snapshot["rules_implemented"], false);
    assert_eq!(
        code(
            call(
                &h,
                &a,
                command(&r.room_id, r.version, Command::Start(v1::Empty {}))
            )
            .await
        ),
        v1::ErrorCode::RoomNotJoinable as i32
    );
    assert_eq!(
        code(
            call(
                &h,
                &c,
                command(
                    &r.room_id,
                    r.version,
                    Command::Join(v1::JoinRoom {
                        code: r.code.clone(),
                        password: None
                    })
                )
            )
            .await
        ),
        v1::ErrorCode::RoomNotJoinable as i32
    );
    assert_eq!(
        code(
            call(
                &h,
                &a,
                command(&r.room_id, r.version, Command::Leave(v1::Empty {}))
            )
            .await
        ),
        v1::ErrorCode::RoomNotJoinable as i32
    );
    let games: i64 = sqlx::query_scalar("SELECT count(*) FROM patchwork.games WHERE room_id=$1")
        .bind(Uuid::parse_str(&r.room_id).unwrap())
        .fetch_one(h.db.pool())
        .await
        .unwrap();
    assert_eq!(games, 1);
    let mut final_snapshot = snapshot;
    final_snapshot["phase"] = serde_json::json!("finished");
    h.db.write_game(backend::persistence::repository::GameWrite {
        game: Uuid::parse_str(&r.game_id).unwrap(),
        user: a.stamp.user,
        request_id: Uuid::new_v4().to_string(),
        expected_version: 0,
        intent: serde_json::json!({"test_finish":true}),
        snapshot: final_snapshot,
        event: serde_json::json!({"finished":true}),
        result: Some(backend::persistence::repository::FinalResult {
            score0: 0,
            score1: 0,
            winner_seat: None,
            reason: "completed".into(),
        }),
    })
    .await
    .unwrap();
    let finished = room(call(&h, &a, command(&r.room_id, 0, Command::Get(v1::Empty {}))).await);
    assert_eq!(finished.phase, v1::RoomPhase::Finished as i32);
    assert_eq!(finished.version, r.version + 1);
    let left = room(
        call(
            &h,
            &a,
            command(&r.room_id, finished.version, Command::Leave(v1::Empty {})),
        )
        .await,
    );
    assert_eq!(left.owner_id, b.stamp.user.to_string());
}

#[actix_web::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn offline_peer_blocks_start_and_restart_recovers_membership() {
    let database = db().await;
    let h = harness(database.clone()).await;
    let a = player(&h).await;
    let b = player(&h).await;
    let r = create(&h, &a, None).await;
    let r = join(&h, &b, &r).await;
    let r = ready(&h, &a, &r).await;
    let r = ready(&h, &b, &r).await;
    assert!(h.registry.send(Disconnect(b.stamp)).await.unwrap());
    assert_eq!(
        code(
            call(
                &h,
                &a,
                command(&r.room_id, r.version, Command::Start(v1::Empty {}))
            )
            .await
        ),
        v1::ErrorCode::NotReady as i32
    );
    let restarted = harness(database).await;
    let new_a = connect(&restarted, a.stamp.user, a.session).await;
    let recovered = room(
        call(
            &restarted,
            &new_a,
            command("", 0, Command::Get(v1::Empty {})),
        )
        .await,
    );
    assert_eq!(recovered.room_id, r.room_id);
    assert_eq!(recovered.version, r.version + 1);
    assert!(recovered.members.iter().all(|m| !m.ready));
    assert_eq!(recovered.owner_id, r.owner_id);
    assert_eq!(
        recovered
            .members
            .iter()
            .map(|m| (&m.user_id, m.seat))
            .collect::<Vec<_>>(),
        r.members
            .iter()
            .map(|m| (&m.user_id, m.seat))
            .collect::<Vec<_>>()
    );
    assert!(restarted.lobby.send(RoomsReady).await.unwrap());
    // Old registry's in-memory lease cannot bypass the persisted generation fence.
    assert_eq!(
        code(
            call(
                &h,
                &a,
                command(
                    &r.room_id,
                    r.version,
                    Command::SetReady(v1::SetReady { ready: false })
                )
            )
            .await
        ),
        v1::ErrorCode::Forbidden as i32
    );
}

#[actix_web::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn room_actor_resolves_lost_commit_ack_before_releasing_user_reservation() {
    let config = backend::config::Config::load(std::path::Path::new(
        &std::env::var("PATCHWORK_TEST_CONFIG").unwrap(),
    ))
    .unwrap()
    .database
    .unwrap();
    let (port, armed, proxy) = pg_proxy::start(config.port).await;
    let relay = db_with(false, |c| {
        c.port = port;
        // Keep a spare connection for the room actor's recovery read after
        // the relay drops the original COMMIT acknowledgement. The
        // persistence-layer max-one-pool case is covered separately.
        c.max_connections = 2;
    })
    .await;
    let h = harness(relay.clone()).await;
    let a = player(&h).await;
    armed.store(true, std::sync::atomic::Ordering::SeqCst);
    let r = create(&h, &a, None).await;
    assert!(!armed.load(std::sync::atomic::Ordering::SeqCst));
    let r = ready(&h, &a, &r).await;
    assert!(r.members[0].ready);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM patchwork.room_members WHERE user_id=$1")
            .bind(a.stamp.user)
            .fetch_one(relay.pool())
            .await
            .unwrap();
    assert_eq!(count, 1);
    proxy.abort();
}

#[actix_web::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn keyset_pages_are_unique_and_old_receipts_do_not_roll_back_lobby_index() {
    let h = harness(db().await).await;
    let a = player(&h).await;
    let b = player(&h).await;
    let request = command(
        "",
        0,
        Command::Create(v1::CreateRoom {
            mode: "casual".into(),
            rules_version: "v1".into(),
            password: None,
        }),
    );
    let id = Uuid::new_v4().to_string();
    let original = send(&h, &a, request.clone(), &id).await;
    let r = room(original.clone());
    let r = join(&h, &b, &r).await;
    let repeated = send(&h, &a, request, &id).await;
    assert_eq!(original.encode_to_vec(), repeated.encode_to_vec());
    let current = room(call(&h, &b, command("", 0, Command::Get(v1::Empty {}))).await);
    assert_eq!(current.members.len(), 2);
    assert_eq!(current.version, r.version);
    let mut cursor = String::new();
    let mut ids = std::collections::HashSet::new();
    loop {
        let response = call(
            &h,
            &a,
            command("", 0, Command::List(v1::ListRooms { cursor, limit: 2 })),
        )
        .await;
        let Some(Payload::Rooms(page)) = response.payload else {
            panic!("expected page")
        };
        for r in page.rooms {
            assert_eq!(r.phase, v1::RoomPhase::Waiting as i32);
            assert!(ids.insert(r.room_id));
        }
        if page.next_cursor.is_empty() {
            break;
        }
        cursor = page.next_cursor;
        assert!(ids.len() < 200);
    }
    assert!(ids.contains(&r.room_id));
}

async fn wait_pending(h: &Harness, count: usize) {
    for _ in 0..100 {
        if h.lobby.send(backend::game::PendingCount).await.unwrap() == count {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("operation did not enter expected pending state");
}
fn dispatch(p: &Player, request: v1::LobbyRequest) -> Dispatch {
    Dispatch {
        stamp: p.stamp,
        message: v1::ClientEnvelope {
            protocol_version: VERSION,
            request_id: Uuid::new_v4().to_string(),
            payload: Some(v1::client_envelope::Payload::Lobby(request)),
        },
    }
}
#[actix_web::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn queued_old_connection_is_fenced_and_uncommitted_state_is_not_pushed() {
    let h = harness(db().await).await;
    let admin = db_with(true, |_| {}).await;
    let mut host = player(&h).await;
    let guest = player(&h).await;
    let r = create(&h, &host, None).await;
    while host.push.try_recv().is_ok() {}
    let mut held = admin.pool().begin().await.unwrap();
    sqlx::query("SELECT user_id FROM patchwork.users WHERE user_id=$1 FOR UPDATE")
        .bind(host.stamp.user)
        .execute(&mut *held)
        .await
        .unwrap();
    let registry = h.registry.clone();
    let ready = dispatch(
        &host,
        command(
            &r.room_id,
            r.version,
            Command::SetReady(v1::SetReady { ready: true }),
        ),
    );
    let first = actix_web::rt::spawn(async move { registry.send(ready).await.unwrap().unwrap() });
    wait_pending(&h, 1).await;
    let registry = h.registry.clone();
    let join_request = dispatch(
        &guest,
        command(
            &r.room_id,
            r.version,
            Command::Join(v1::JoinRoom {
                code: r.code.clone(),
                password: None,
            }),
        ),
    );
    let queued =
        actix_web::rt::spawn(async move { registry.send(join_request).await.unwrap().unwrap() });
    wait_pending(&h, 2).await;
    assert!(host.push.try_recv().is_err(), "broadcast before commit");
    let replacement = connect(&h, guest.stamp.user, guest.session).await;
    held.commit().await.unwrap();
    assert!(room(first.await.unwrap()).members[0].ready);
    assert_eq!(code(queued.await.unwrap()), v1::ErrorCode::Forbidden as i32);
    assert!(!h.registry.send(Disconnect(guest.stamp)).await.unwrap());
    let current = room(
        call(
            &h,
            &host,
            command(&r.room_id, 0, Command::Get(v1::Empty {})),
        )
        .await,
    );
    assert_eq!(current.members.len(), 1);
    let joined = join(&h, &replacement, &current).await;
    assert_eq!(joined.members.len(), 2);
}

#[actix_web::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn failed_room_transaction_does_not_publish_or_leave_a_reservation() {
    let h = harness(db().await).await;
    let admin = db_with(true, |_| {}).await;
    let mut host = player(&h).await;
    let r = create(&h, &host, None).await;
    while host.push.try_recv().is_ok() {}
    let mut held = admin.pool().begin().await.unwrap();
    sqlx::query("SELECT user_id FROM patchwork.users WHERE user_id=$1 FOR UPDATE")
        .bind(host.stamp.user)
        .execute(&mut *held)
        .await
        .unwrap();
    let response = call(
        &h,
        &host,
        command(
            &r.room_id,
            r.version,
            Command::SetReady(v1::SetReady { ready: true }),
        ),
    )
    .await;
    assert_eq!(code(response), v1::ErrorCode::ServiceUnavailable as i32);
    assert!(host.push.try_recv().is_err());
    held.commit().await.unwrap();
    let current = room(
        call(
            &h,
            &host,
            command(&r.room_id, 0, Command::Get(v1::Empty {})),
        )
        .await,
    );
    assert_eq!(current.version, r.version);
    assert!(!current.members[0].ready);
    assert!(ready(&h, &host, &current).await.members[0].ready);
    assert_eq!(h.lobby.send(backend::game::PendingCount).await.unwrap(), 0);
}
