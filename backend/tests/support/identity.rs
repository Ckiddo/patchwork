use super::{db, db_with, pg_proxy};
use actix::Actor;
use actix_web::{App, test, web};
use backend::{
    AppState, api,
    game::LobbyManager,
    identity::{self, AccessClaims, AuthConfig},
    persistence::{Database, StoreError as E},
    sessions::*,
};
use serde_json::{Value, json};
use util_lib::{
    Claims,
    protocol::{VERSION, v1},
};
use uuid::Uuid;
const KEY: &[u8] = b"isolated-session-test-signing-key-only";

#[tokio::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn create_retry_rechecks_hash_after_waiting_for_user_lock() {
    let db = db_with(false, |c| {
        c.max_connections = 1;
    })
    .await;
    let admin = db_with(true, |_| {}).await;
    let (sid, token, p) = account(&db).await;
    let old = identity::token_hash(&token).unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(db.pool())
        .await
        .unwrap();
    let mut held = admin.pool().begin().await.unwrap();
    sqlx::query("SELECT user_id FROM patchwork.users WHERE user_id=$1 FOR UPDATE")
        .bind(p.id)
        .execute(&mut *held)
        .await
        .unwrap();
    let operation = tokio::spawn(async move { db.create_identity_session(sid, old, None).await });
    let mut waiting = false;
    for _ in 0..50 {
        waiting = sqlx::query_scalar(
            "SELECT coalesce(wait_event_type='Lock',false) FROM pg_stat_activity WHERE pid=$1",
        )
        .bind(pid)
        .fetch_one(admin.pool())
        .await
        .unwrap();
        if waiting {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let next = identity::token_hash(&identity::random_token()).unwrap();
    sqlx::query("UPDATE patchwork.sessions SET previous_refresh_hash=refresh_hash,refresh_hash=$2,rotation_id=$3 WHERE session_id=$1").bind(sid).bind(next).bind(Uuid::new_v4()).execute(&mut *held).await.unwrap();
    held.commit().await.unwrap();
    assert!(
        waiting,
        "create retry did not reach the intended lock boundary"
    );
    assert!(matches!(operation.await.unwrap(), Err(E::Permission)));
}
fn state(db: Database, legacy: bool) -> web::Data<AppState> {
    web::Data::new(
        AppState::new(KEY, LobbyManager::default().start())
            .with_database(db)
            .with_auth(
                AuthConfig {
                    allow_legacy_migration: legacy,
                    ..Default::default()
                },
                vec!["https://ckiddo.github.io".into()],
            ),
    )
}
fn signed(claims: &AccessClaims) -> String {
    jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        claims,
        &jsonwebtoken::EncodingKey::from_secret(KEY),
    )
    .unwrap()
}
async fn account(db: &Database) -> (Uuid, String, backend::persistence::identity::Profile) {
    let sid = Uuid::new_v4();
    let token = identity::random_token();
    let p = db
        .create_identity_session(sid, identity::token_hash(&token).unwrap(), None)
        .await
        .unwrap();
    (sid, token, p)
}

#[actix_web::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn persistent_http_contract_refresh_and_logout() {
    let db = db().await;
    let state = state(db.clone(), false);
    let app = test::init_service(
        App::new()
            .app_data(state.clone())
            .app_data(api::json_config())
            .configure(api::configure),
    )
    .await;
    let created: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/create")
            .to_request(),
    )
    .await;
    let token = created["jwt"].as_str().unwrap();
    let bearer = format!("Bearer {token}");
    let nickname = "拼布".repeat(10);
    let updated: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::put()
            .uri("/api/auth/nickname")
            .insert_header(("Authorization", bearer.clone()))
            .set_json(json!({"nickname":nickname}))
            .to_request(),
    )
    .await;
    assert_eq!(
        updated["identity"]["user_id"],
        created["identity"]["user_id"]
    );
    assert_eq!(
        updated["identity"]["created_at"],
        created["identity"]["created_at"]
    );
    let verified: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/verify")
            .insert_header(("Authorization", bearer.clone()))
            .to_request(),
    )
    .await;
    assert_eq!(verified["identity"]["nickname"], nickname);
    let mut expired = identity::decode_token(&state, token).unwrap();
    expired.claims.iat -= 2000;
    expired.claims.exp = chrono::Utc::now().timestamp() - 1;
    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/verify")
            .insert_header(("Authorization", format!("Bearer {}", signed(&expired))))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), 401);
    let rotation = json!({"session_id":created["session_id"],"refresh_token":created["refresh_token"],"next_refresh_token":identity::random_token(),"rotation_id":Uuid::new_v4()});
    let refreshed: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/refresh")
            .set_json(&rotation)
            .to_request(),
    )
    .await;
    assert_eq!(refreshed["identity"], updated["identity"]);
    let replay: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/refresh")
            .set_json(&rotation)
            .to_request(),
    )
    .await;
    assert_eq!(replay["identity"], updated["identity"]);
    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/logout")
            .insert_header(("Authorization", bearer.clone()))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), 200);
    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/verify")
            .insert_header(("Authorization", bearer))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), 401);
    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/refresh")
            .set_json(rotation)
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), 401);
}

#[actix_web::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn legacy_import_requires_valid_signature_opt_in_and_preserves_database_profile() {
    let db = db().await;
    let id = Uuid::new_v4();
    let now = chrono::Utc::now().timestamp();
    let claims = AccessClaims {
        claims: Claims {
            sub: id.to_string(),
            nickname: "旧昵称".into(),
            iat: now - 3600,
            exp: now + 3600,
        },
        sid: None,
        auth_version: None,
    };
    let disabled = state(db.clone(), false);
    assert!(
        identity::authenticate(&disabled, &signed(&claims))
            .await
            .is_err()
    );
    let enabled = state(db.clone(), true);
    let wrong = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(b"wrong-key"),
    )
    .unwrap();
    assert!(identity::authenticate(&enabled, &wrong).await.is_err());
    let mut expired = claims.clone();
    expired.claims.exp = now - 1;
    assert!(
        identity::authenticate(&enabled, &signed(&expired))
            .await
            .is_err()
    );
    let p = identity::authenticate(&enabled, &signed(&claims))
        .await
        .unwrap();
    assert_eq!(p.profile.created_at, now - 3600);
    sqlx::query("UPDATE patchwork.users SET nickname='数据库昵称' WHERE user_id=$1")
        .bind(id)
        .execute(db.pool())
        .await
        .unwrap();
    let app = test::init_service(
        App::new()
            .app_data(enabled)
            .app_data(api::json_config())
            .configure(api::configure),
    )
    .await;
    let candidate = json!({"session_id":Uuid::new_v4(),"refresh_token":identity::random_token()});
    for _ in 0..2 {
        let upgraded: Value = test::call_and_read_body_json(
            &app,
            test::TestRequest::post()
                .uri("/api/auth/session")
                .insert_header(("Authorization", format!("Bearer {}", signed(&claims))))
                .set_json(&candidate)
                .to_request(),
        )
        .await;
        assert_eq!(upgraded["identity"]["user_id"], id.to_string());
        assert_eq!(upgraded["identity"]["nickname"], "数据库昵称");
        assert_eq!(upgraded["identity"]["created_at"], now - 3600);
    }
    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/session")
            .insert_header(("Authorization", format!("Bearer {}", signed(&claims))))
            .set_json(json!({"session_id":Uuid::new_v4(),"refresh_token":identity::random_token()}))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), 401);
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn concurrent_refresh_has_one_winner_and_only_hashes_are_stored() {
    let db = db().await;
    let (sid, token, p) = account(&db).await;
    let old = identity::token_hash(&token).unwrap();
    let a = identity::token_hash(&identity::random_token()).unwrap();
    let b = identity::token_hash(&identity::random_token()).unwrap();
    let (a, b) = tokio::join!(
        db.rotate_session(sid, old.clone(), a, Uuid::new_v4()),
        db.rotate_session(sid, old, b, Uuid::new_v4())
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let hash: Vec<u8> =
        sqlx::query_scalar("SELECT refresh_hash FROM patchwork.sessions WHERE session_id=$1")
            .bind(sid)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(hash.len(), 32);
    assert!(hash != token.as_bytes());
    db.revoke_session(p.id, sid).await.unwrap();
    assert!(matches!(
        db.session_profile(p.id, sid, 0).await,
        Err(E::Permission)
    ));
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn lost_create_and_refresh_commit_ack_are_recoverable() {
    let db = db().await;
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
    let sid = Uuid::new_v4();
    let token = identity::random_token();
    let hash = identity::token_hash(&token).unwrap();
    armed.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(matches!(
        relay.create_identity_session(sid, hash.clone(), None).await,
        Err(E::CommitUnknown)
    ));
    let p = db
        .create_identity_session(sid, hash.clone(), None)
        .await
        .unwrap();
    let next = identity::token_hash(&identity::random_token()).unwrap();
    let rotation = Uuid::new_v4();
    armed.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(matches!(
        relay
            .rotate_session(sid, hash.clone(), next.clone(), rotation)
            .await,
        Err(E::CommitUnknown)
    ));
    let recovered = db.rotate_session(sid, hash, next, rotation).await.unwrap();
    assert_eq!(recovered.id, p.id);
    relay.close().await;
    proxy.abort();
}

#[actix_web::test]
#[ignore = "requires isolated PostgreSQL runner"]
async fn takeover_fences_old_dispatch_disconnect_and_survives_registry_restart() {
    let db = db().await;
    let (sid, _, p) = account(&db).await;
    let registry = SessionRegistry::default().start();
    let (kick1, mut rx1) = tokio::sync::watch::channel(false);
    let (kick2, _rx2) = tokio::sync::watch::channel(false);
    let register = |kick| Register {
        database: db.clone(),
        user: p.id,
        session: sid,
        auth_version: p.auth_version,
        expires: chrono::Utc::now().timestamp() + 60,
        kick,
    };
    let first = registry.send(register(kick1)).await.unwrap().unwrap();
    let second = registry.send(register(kick2)).await.unwrap().unwrap();
    rx1.changed().await.unwrap();
    assert!(*rx1.borrow());
    assert!(second.generation > first.generation);
    let ping = || v1::ClientEnvelope {
        protocol_version: VERSION,
        request_id: "ping".into(),
        payload: Some(v1::client_envelope::Payload::Ping(v1::Ping { nonce: 7 })),
    };
    assert!(matches!(
        registry
            .send(Dispatch {
                stamp: first,
                message: ping()
            })
            .await
            .unwrap(),
        Err(E::Permission)
    ));
    assert!(!registry.send(Disconnect(first)).await.unwrap());
    assert!(
        registry
            .send(Dispatch {
                stamp: second,
                message: ping()
            })
            .await
            .unwrap()
            .is_ok()
    );
    let restarted = SessionRegistry::default().start();
    let (kick3, _rx3) = tokio::sync::watch::channel(false);
    let third = restarted.send(register(kick3)).await.unwrap().unwrap();
    assert!(third.generation > second.generation);
    db.revoke_session(p.id, sid).await.unwrap();
    restarted
        .send(Revoke {
            user: p.id,
            session: sid,
        })
        .await
        .unwrap();
    assert!(matches!(
        restarted
            .send(Dispatch {
                stamp: third,
                message: ping()
            })
            .await
            .unwrap(),
        Err(E::Permission)
    ));
    let (kick4, _rx4) = tokio::sync::watch::channel(false);
    assert!(matches!(
        restarted.send(register(kick4)).await.unwrap(),
        Err(E::Permission)
    ));
}
