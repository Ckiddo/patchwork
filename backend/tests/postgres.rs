//! Run ONLY with tools/ci/postgres_suite.py. Normal unit tests never open a DB.
use backend::{
    config::Config,
    persistence::{Database, StoreError as E, repository::*},
};
use chrono::{Duration, Utc};
use serde_json::json;
use sqlx::Row;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use uuid::Uuid;

#[path = "support/friends.rs"]
mod friend_tests;
#[path = "support/identity.rs"]
mod identity_tests;
#[path = "support/pg_proxy.rs"]
mod pg_proxy;

#[tokio::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn committed_but_lost_ack_recovers_original_receipt() {
    let direct = db().await;
    let u = user(&direct).await;
    let r = create(u);
    let path = std::env::var("PATCHWORK_TEST_CONFIG").unwrap();
    let config = Config::load(std::path::Path::new(&path))
        .unwrap()
        .database
        .unwrap();
    let (port, armed, proxy) = pg_proxy::start(config.port).await;
    let relayed = db_with(false, |c| {
        c.port = port;
        c.max_connections = 1;
    })
    .await;
    armed.store(true, Ordering::SeqCst);
    assert_eq!(
        relayed.operate(r.clone()).await.unwrap_err(),
        E::CommitUnknown
    );
    assert!(!armed.load(Ordering::SeqCst));
    let recovered = direct
        .recover_operation(r.clone())
        .await
        .unwrap()
        .into_inner()
        .expect("COMMIT succeeded before ACK was dropped");
    assert_eq!(
        direct.operate(r.clone()).await.unwrap().into_inner(),
        recovered
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM patchwork.rooms WHERE room_id=$1")
        .bind(room_id(&r))
        .fetch_one(direct.pool())
        .await
        .unwrap();
    assert_eq!(count, 1);
    relayed.close().await;
    proxy.abort();
}

#[actix_web::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn readiness_requires_database_and_recovered_room_directory() {
    use actix::Actor;
    use actix_web::{App, test, web};
    let db = db().await;
    let state = backend::AppState::new(
        b"test-key-no-production",
        backend::game::LobbyManager::default().start(),
    )
    .with_database(db.clone());
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .configure(backend::api::configure),
    )
    .await;
    // Configure queues an asynchronous recovery pass. A busy test database may
    // legitimately report 503 until that pass has recovered its room directory.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let response = loop {
        let response =
            test::call_service(&app, test::TestRequest::get().uri("/readyz").to_request()).await;
        if response.status() == 200 {
            break response;
        }
        assert_eq!(response.status(), 503);
        assert!(
            std::time::Instant::now() < deadline,
            "room recovery did not become ready"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    };
    let value: serde_json::Value = test::read_body_json(response).await;
    assert_eq!(value["database"], "ok");
    assert_eq!(value["rooms"], "ok");
    db.close().await;
    let response =
        test::call_service(&app, test::TestRequest::get().uri("/readyz").to_request()).await;
    assert_eq!(response.status(), 503);
    let value: serde_json::Value = test::read_body_json(response).await;
    assert_eq!(value["database"], "unavailable");
}

async fn db_with(
    admin: bool,
    configure: impl FnOnce(&mut backend::persistence::config::DatabaseConfig),
) -> Database {
    let name = std::env::var("PATCHWORK_TEST_DATABASE").expect("use isolated suite runner");
    assert!(name.starts_with("patchwork_test_") && name.len() == 27);
    let path = std::env::var(if admin {
        "PATCHWORK_TEST_ADMIN_CONFIG"
    } else {
        "PATCHWORK_TEST_CONFIG"
    })
    .expect("use isolated suite runner");
    let mut config = Config::load(std::path::Path::new(&path))
        .unwrap()
        .database
        .unwrap();
    assert_eq!(config.name, name);
    assert_eq!(config.host, "127.0.0.1");
    if !admin {
        assert_eq!(config.user, "patchwork_app");
    }
    configure(&mut config);
    let db = Database::connect(&config).await.unwrap();
    let actual: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(actual, name);
    db
}
async fn db() -> Database {
    db_with(false, |_| {}).await
}
async fn user(db: &Database) -> Uuid {
    let id = Uuid::new_v4();
    db.ensure_user(id, "测试用户".into()).await.unwrap();
    id
}
fn req(user: Uuid, operation: Operation) -> OperationRequest {
    OperationRequest {
        operation_id: Uuid::new_v4(),
        user,
        request_id: Uuid::new_v4().to_string(),
        operation,
    }
}
fn create(user: Uuid) -> OperationRequest {
    req(
        user,
        Operation::CreateRoom {
            room: Uuid::new_v4(),
            code: Uuid::new_v4().simple().to_string()[..10].to_uppercase(),
            mode: "casual".into(),
            rules: "v1".into(),
        },
    )
}
fn room_id(r: &OperationRequest) -> Uuid {
    match r.operation {
        Operation::CreateRoom { room, .. } => room,
        _ => panic!(),
    }
}
fn queue(user: Uuid, mode: &str) -> OperationRequest {
    req(
        user,
        Operation::Queue {
            ticket: Uuid::new_v4(),
            mode: mode.into(),
            rules: "v1".into(),
            generation: 1,
            expires: Utc::now() + Duration::minutes(5),
        },
    )
}
fn ticket_id(r: &OperationRequest) -> Uuid {
    match r.operation {
        Operation::Queue { ticket, .. } => ticket,
        _ => panic!(),
    }
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn migrations_roles_types_and_constraints() {
    let db = db().await;
    db.check_schema().await.unwrap();
    db.check_runtime_role().await.unwrap();
    assert!(db.healthy().await);
    let e = sqlx::query("CREATE TABLE patchwork.forbidden(id int)")
        .execute(db.pool())
        .await
        .unwrap_err();
    assert_eq!(
        e.as_database_error().unwrap().code().as_deref(),
        Some("42501")
    );
    assert!(
        sqlx::query("UPDATE patchwork._sqlx_migrations SET success=false")
            .execute(db.pool())
            .await
            .is_err()
    );
    let u = user(&db).await;
    db.ensure_user(u, "不能覆盖旧资料".into()).await.unwrap();
    let n: String = sqlx::query_scalar("SELECT nickname FROM patchwork.users WHERE user_id=$1")
        .bind(u)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(n, "测试用户");
    let hash = vec![7u8; 32];
    sqlx::query("INSERT INTO patchwork.sessions(session_id,user_id,refresh_hash,expires_at) VALUES($1,$2,$3,now()+interval '1 hour')")
        .bind(Uuid::new_v4()).bind(u).bind(&hash).execute(db.pool()).await.unwrap();
    assert!(sqlx::query("INSERT INTO patchwork.sessions(session_id,user_id,refresh_hash,expires_at) VALUES($1,$2,$3,now()+interval '1 hour')")
        .bind(Uuid::new_v4()).bind(u).bind(&hash).execute(db.pool()).await.is_err());
    let r = create(u);
    let room = room_id(&r);
    db.operate(r).await.unwrap();
    let outsider = user(&db).await;
    assert!(
        sqlx::query("INSERT INTO patchwork.room_members(room_id,seat,user_id) VALUES($1,2,$2)")
            .bind(room)
            .bind(outsider)
            .execute(db.pool())
            .await
            .is_err()
    );
    assert!(sqlx::query("INSERT INTO patchwork.matchmaking_tickets(ticket_id,user_id,mode,rules_version,connection_generation,status,expires_at) VALUES($1,$2,'casual','v1',1,'queued',now()+interval '1 hour')")
        .bind(Uuid::new_v4()).bind(u).execute(db.pool()).await.is_err());
    assert!(
        sqlx::query(
            "UPDATE patchwork.player_occupancy SET state='idle',room_id=NULL WHERE user_id=$1"
        )
        .bind(u)
        .execute(db.pool())
        .await
        .is_err()
    );
    assert!(
        sqlx::query("UPDATE patchwork.rooms SET version=-1 WHERE room_id=$1")
            .bind(room)
            .execute(db.pool())
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn concurrent_occupancy_and_last_seat_have_one_winner() {
    let db = db().await;
    let u = user(&db).await;
    let (a, b) = tokio::join!(db.operate(create(u)), db.operate(create(u)));
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let owner = user(&db).await;
    let r = create(owner);
    let room = room_id(&r);
    db.operate(r).await.unwrap();
    let (u1, u2) = (user(&db).await, user(&db).await);
    let (a, b) = tokio::join!(
        db.operate(req(
            u1,
            Operation::JoinRoom {
                room,
                expected_version: 0
            }
        )),
        db.operate(req(
            u2,
            Operation::JoinRoom {
                room,
                expected_version: 0
            }
        ))
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM patchwork.room_members WHERE room_id=$1")
            .bind(room)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(count, 2);
    let u = user(&db).await;
    let (a, b) = tokio::join!(db.operate(create(u)), db.operate(queue(u, "race")));
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn operation_receipts_recover_and_reject_reused_payloads() {
    let db = db().await;
    let r = create(user(&db).await);
    assert!(
        db.recover_operation(r.clone())
            .await
            .unwrap()
            .into_inner()
            .is_none()
    );
    let original = db.operate(r.clone()).await.unwrap().into_inner();
    assert_eq!(db.operate(r.clone()).await.unwrap().into_inner(), original);
    assert_eq!(
        db.recover_operation(r.clone()).await.unwrap().into_inner(),
        Some(original)
    );
    let mut bad = r;
    bad.operation = Operation::Queue {
        ticket: Uuid::new_v4(),
        mode: "casual".into(),
        rules: "v1".into(),
        generation: 1,
        expires: Utc::now() + Duration::minutes(1),
    };
    assert_eq!(db.operate(bad).await.unwrap_err(), E::RequestIdConflict);
    assert!(db.rooms_page(None, 1).await.unwrap().len() <= 1);
    assert_eq!(db.rooms_page(None, 101).await.unwrap_err(), E::InvalidInput);
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn pair_transaction_preserves_fifo_and_unique_occupancy() {
    let db = db().await;
    let mode = Uuid::new_v4().simple().to_string();
    let mut users = Vec::new();
    let mut tickets = Vec::new();
    for _ in 0..4 {
        let u = user(&db).await;
        let q = queue(u, &mode);
        tickets.push(ticket_id(&q));
        db.operate(q).await.unwrap();
        users.push(u);
    }
    let pair = |a: usize, b: usize| {
        req(
            users[a],
            Operation::Pair {
                users: [users[a], users[b]],
                tickets: [tickets[a], tickets[b]],
                room: Uuid::new_v4(),
                code: Uuid::new_v4().simple().to_string()[..10].to_uppercase(),
            },
        )
    };
    assert_eq!(db.operate(pair(2, 3)).await.unwrap_err(), E::Conflict);
    let r = pair(0, 1);
    let (a, b) = tokio::join!(db.operate(r.clone()), db.operate(r.clone()));
    assert_eq!(a.unwrap().into_inner(), b.unwrap().into_inner());
    let rows =
        sqlx::query("SELECT state,room_id FROM patchwork.player_occupancy WHERE user_id=ANY($1)")
            .bind(vec![users[0], users[1]])
            .fetch_all(db.pool())
            .await
            .unwrap();
    assert_eq!(
        rows[0].get::<Uuid, _>("room_id"),
        rows[1].get::<Uuid, _>("room_id")
    );
    assert!(rows.iter().all(|r| r.get::<String, _>("state") == "room"));
    assert_eq!(db.operate(pair(0, 2)).await.unwrap_err(), E::Conflict);
}

async fn game(db: &Database) -> (Uuid, Uuid) {
    let u = user(db).await;
    let r = create(u);
    let room = room_id(&r);
    db.operate(r).await.unwrap();
    db.operate(req(
        user(db).await,
        Operation::JoinRoom {
            room,
            expected_version: 0,
        },
    ))
    .await
    .unwrap();
    sqlx::query("UPDATE patchwork.room_members SET ready=true WHERE room_id=$1")
        .bind(room)
        .execute(db.pool())
        .await
        .unwrap();
    let g = Uuid::new_v4();
    db.begin_game(room, g, 1, json!({"turn":0})).await.unwrap();
    (g, u)
}
fn write(game: Uuid, user: Uuid) -> GameWrite {
    GameWrite {
        game,
        user,
        request_id: Uuid::new_v4().to_string(),
        expected_version: 0,
        intent: json!({"advance":true}),
        snapshot: json!({"turn":1}),
        event: json!({"advanced":true}),
        result: None,
    }
}
#[tokio::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn game_commit_is_atomic_idempotent_and_has_one_result() {
    let db = db().await;
    let (g, u) = game(&db).await;
    let w = write(g, u);
    let mut bad = w.clone();
    bad.result = Some(FinalResult {
        score0: 0,
        score1: 0,
        winner_seat: Some(3),
        reason: "completed".into(),
    });
    assert_eq!(db.write_game(bad).await.unwrap_err(), E::Conflict);
    assert_eq!(db.game_snapshot(g).await.unwrap(), json!({"turn":0}));
    assert!(db.events_after(g, 0, 100).await.unwrap().is_empty());
    assert!(
        db.recover_game_write(w.clone())
            .await
            .unwrap()
            .into_inner()
            .is_none()
    );
    let response = db.write_game(w.clone()).await.unwrap().into_inner();
    assert_eq!(
        db.write_game(w.clone()).await.unwrap().into_inner(),
        response
    );
    assert_eq!(
        db.recover_game_write(w.clone()).await.unwrap().into_inner(),
        Some(response)
    );
    let mut changed = w.clone();
    changed.snapshot = json!({"turn":999});
    assert_eq!(
        db.write_game(changed).await.unwrap_err(),
        E::RequestIdConflict
    );
    assert_eq!(
        db.write_game(write(g, u)).await.unwrap_err(),
        E::VersionConflict
    );
    let mut end = write(g, u);
    end.expected_version = 1;
    end.result = Some(FinalResult {
        score0: 10,
        score1: 5,
        winner_seat: Some(0),
        reason: "completed".into(),
    });
    db.write_game(end.clone()).await.unwrap();
    db.write_game(end).await.unwrap();
    assert_eq!(db.events_after(g, 0, 100).await.unwrap().len(), 2);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM patchwork.game_results WHERE game_id=$1")
            .bind(g)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn pool_exhaustion_readonly_and_schema_mismatch_revoke_health() {
    let db = db_with(false, |c| {
        c.max_connections = 1;
        c.acquire_timeout_ms = 1000;
    })
    .await;
    let held = db.pool().acquire().await.unwrap();
    assert!(!db.healthy().await);
    drop(held);
    assert!(db.healthy().await);
    sqlx::query("SET default_transaction_read_only=on")
        .execute(db.pool())
        .await
        .unwrap();
    assert!(!db.healthy().await);
    sqlx::query("SET default_transaction_read_only=off")
        .execute(db.pool())
        .await
        .unwrap();
    assert!(db.healthy().await);
    let admin = db_with(true, |_| {}).await;
    sqlx::query("UPDATE patchwork._sqlx_migrations SET success=false WHERE version=1")
        .execute(admin.pool())
        .await
        .unwrap();
    let healthy = db.healthy().await;
    sqlx::query("UPDATE patchwork._sqlx_migrations SET success=true WHERE version=1")
        .execute(admin.pool())
        .await
        .unwrap();
    assert!(!healthy);
    assert!(db.healthy().await);
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn lock_and_statement_timeouts_do_not_commit_or_publish() {
    let db = db_with(false, |c| {
        c.lock_timeout_ms = 100;
        c.statement_timeout_ms = 300;
    })
    .await;
    let u = user(&db).await;
    let mut held = db.pool().begin().await.unwrap();
    sqlx::query("SELECT user_id FROM patchwork.player_occupancy WHERE user_id=$1 FOR UPDATE")
        .bind(u)
        .execute(&mut *held)
        .await
        .unwrap();
    let r = create(u);
    let result = db.operate(r.clone()).await;
    held.rollback().await.unwrap();
    assert_eq!(result.unwrap_err(), E::Unavailable);
    assert!(
        db.recover_operation(r.clone())
            .await
            .unwrap()
            .into_inner()
            .is_none()
    );
    db.operate(r).await.unwrap();
    let published = Arc::new(AtomicUsize::new(0));
    let result = db
        .transaction(|c| {
            Box::pin(async move {
                sqlx::query("SELECT pg_sleep(1)").execute(c).await?;
                Ok(())
            })
        })
        .await;
    if result.is_ok() {
        published.fetch_add(1, Ordering::SeqCst);
    }
    assert_eq!(result.unwrap_err(), E::Unavailable);
    assert_eq!(published.load(Ordering::SeqCst), 0);
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn connection_loss_during_commit_is_unknown_and_not_replayed() {
    let db = db().await;
    let admin = db_with(true, |_| {}).await;
    let u = user(&db).await;
    let attempts = Arc::new(AtomicUsize::new(0));
    let calls = attempts.clone();
    let result = db
        .transaction(move |c| {
            let admin = admin.clone();
            let calls = calls.clone();
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                sqlx::query("UPDATE patchwork.users SET nickname='uncommitted' WHERE user_id=$1")
                    .bind(u)
                    .execute(&mut *c)
                    .await?;
                let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                    .fetch_one(c)
                    .await?;
                sqlx::query("SELECT pg_terminate_backend($1)")
                    .bind(pid)
                    .execute(admin.pool())
                    .await?;
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                Ok(())
            })
        })
        .await;
    assert_eq!(result.unwrap_err(), E::CommitUnknown);
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    let name: String = sqlx::query_scalar("SELECT nickname FROM patchwork.users WHERE user_id=$1")
        .bind(u)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(name, "测试用户");
    assert!(db.healthy().await);
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn deadlock_is_rolled_back_before_bounded_retry() {
    let db = db_with(false, |c| {
        c.lock_timeout_ms = 5000;
        c.statement_timeout_ms = 10000;
    })
    .await;
    let (a, b) = (user(&db).await, user(&db).await);
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let calls = Arc::new(AtomicUsize::new(0));
    async fn work(
        db: &Database,
        first: Uuid,
        second: Uuid,
        barrier: Arc<tokio::sync::Barrier>,
        calls: Arc<AtomicUsize>,
    ) {
        let mut attempt = 0;
        db.transaction(move |c| {
            attempt += 1;
            let n = attempt;
            let barrier = barrier.clone();
            let calls = calls.clone();
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                sqlx::query(
                    "UPDATE patchwork.users SET auth_version=auth_version+1 WHERE user_id=$1",
                )
                .bind(first)
                .execute(&mut *c)
                .await?;
                if n == 1 {
                    barrier.wait().await;
                }
                sqlx::query(
                    "UPDATE patchwork.users SET auth_version=auth_version+1 WHERE user_id=$1",
                )
                .bind(second)
                .execute(c)
                .await?;
                Ok(())
            })
        })
        .await
        .unwrap();
    }
    tokio::join!(
        work(&db, a, b, barrier.clone(), calls.clone()),
        work(&db, b, a, barrier, calls.clone())
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    let versions: Vec<i64> =
        sqlx::query_scalar("SELECT auth_version FROM patchwork.users WHERE user_id=ANY($1)")
            .bind(vec![a, b])
            .fetch_all(db.pool())
            .await
            .unwrap();
    assert_eq!(versions, vec![2, 2]);
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn serialization_failure_retries_are_bounded_and_rollback_each_attempt() {
    let db = db().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let u = user(&db).await;
    let result=db.transaction(move |c| {let count=count.clone();Box::pin(async move {
        count.fetch_add(1,Ordering::SeqCst);
        sqlx::query("UPDATE patchwork.users SET auth_version=auth_version+1 WHERE user_id=$1").bind(u).execute(&mut *c).await?;
        sqlx::query("DO $$ BEGIN RAISE EXCEPTION USING ERRCODE='40001', MESSAGE='test serialization failure'; END $$").execute(c).await?;Ok(())
    })}).await;
    assert_eq!(result.unwrap_err(), E::Unavailable);
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    let version: i64 =
        sqlx::query_scalar("SELECT auth_version FROM patchwork.users WHERE user_id=$1")
            .bind(u)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(version, 0);
}
