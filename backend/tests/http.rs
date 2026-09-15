use actix::Actor;
use actix_web::{App, http::StatusCode, test, web};
use backend::{AppState, api, game::LobbyManager};
use serde_json::Value;

fn state() -> web::Data<AppState> {
    web::Data::new(AppState::new(
        b"test-only-signing-key-not-for-production",
        LobbyManager::default().start(),
    ))
}

#[actix_web::test]
async fn missing_database_does_not_issue_ephemeral_identities() {
    let app = test::init_service(App::new().app_data(state()).configure(api::configure)).await;
    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/create")
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[actix_web::test]
async fn invalid_auth_and_json_never_echo_credentials() {
    let app = test::init_service(
        App::new()
            .app_data(state())
            .app_data(api::json_config())
            .configure(api::configure),
    )
    .await;
    let marker = "credential-marker-must-not-be-reflected";
    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/auth/verify")
            .insert_header(("Authorization", format!("Bearer {marker}")))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = test::read_body(response).await;
    assert!(!String::from_utf8_lossy(&body).contains(marker));
    let response = test::call_service(
        &app,
        test::TestRequest::put()
            .uri("/api/auth/nickname")
            .insert_header(("Content-Type", "application/json"))
            .set_payload(format!("{{bad-{marker}"))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(!String::from_utf8_lossy(&test::read_body(response).await).contains(marker));
}

#[actix_web::test]
async fn liveness_is_not_database_or_room_readiness() {
    let app = test::init_service(App::new().app_data(state()).configure(api::configure)).await;
    assert_eq!(
        test::call_service(&app, test::TestRequest::get().uri("/healthz").to_request())
            .await
            .status(),
        StatusCode::OK
    );
    let response =
        test::call_service(&app, test::TestRequest::get().uri("/readyz").to_request()).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = test::read_body_json(response).await;
    assert_eq!(body["lobby"], "ok");
    assert_eq!(body["database"], "not_configured");
}

#[actix_web::test]
async fn github_origin_put_preflight_and_foreign_origin_rejection() {
    let app = test::init_service(
        App::new()
            .wrap(api::cors(&["https://ckiddo.github.io".into()]))
            .configure(api::configure),
    )
    .await;
    for (origin, expected) in [
        ("https://ckiddo.github.io", StatusCode::OK),
        ("https://untrusted.invalid", StatusCode::BAD_REQUEST),
    ] {
        let response = test::call_service(
            &app,
            test::TestRequest::default()
                .method(actix_web::http::Method::OPTIONS)
                .uri("/api/auth/nickname")
                .insert_header(("Origin", origin))
                .insert_header(("Access-Control-Request-Method", "PUT"))
                .insert_header((
                    "Access-Control-Request-Headers",
                    "authorization,content-type",
                ))
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), expected);
    }
}
