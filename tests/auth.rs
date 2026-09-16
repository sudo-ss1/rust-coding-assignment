mod common;

use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn seeding_is_idempotent() {
    let app = common::spawn_app().await;
    let first = app.seed().await;
    let second = app.seed().await;

    assert_eq!(first["admin"]["id"], second["admin"]["id"], "re-seeding must not create a second admin");
    assert_eq!(first["staff"]["id"], second["staff"]["id"], "re-seeding must not create a second staff user");
    assert_eq!(first["admin"]["role"], "admin");
    assert_eq!(first["staff"]["role"], "staff");
    assert_eq!(first["staff"]["email"], common::BOND_EMAIL);
}

#[tokio::test]
async fn login_returns_a_challenge_and_never_a_token() {
    let app = common::spawn_app().await;
    app.seed().await;

    let res = app
        .post("/auth/login", &json!({ "email": common::ADMIN_EMAIL, "password": common::ADMIN_PASSWORD }))
        .await;
    assert_eq!(res.status(), 200);

    let body: serde_json::Value = res.json().await.unwrap();
    assert!(body["challenge_id"].is_string(), "must return a challenge id");
    assert!(body.get("access_token").is_none(), "a correct password alone must not yield a token");
    assert!(body.get("code").is_none(), "the code must never appear in the login response");
    assert!(body.get("dev_code").is_none(), "the code must never appear in the login response");
}

#[tokio::test]
async fn login_with_a_wrong_password_is_rejected() {
    let app = common::spawn_app().await;
    app.seed().await;

    let res = app
        .post("/auth/login", &json!({ "email": common::ADMIN_EMAIL, "password": "not-the-password" }))
        .await;
    assert_eq!(res.status(), 401);
}

#[tokio::test]
async fn login_is_case_insensitive_on_email() {
    // Regression guard: citext makes the unique index case-insensitive, but a
    // sqlx-bound `text` parameter compares case-sensitively without an explicit
    // ::citext cast, which would make this login fail while the account exists.
    let app = common::spawn_app().await;
    app.seed().await;

    let res = app
        .post("/auth/login", &json!({ "email": "ADMIN@EXAMPLE.COM", "password": common::ADMIN_PASSWORD }))
        .await;
    assert_eq!(res.status(), 200, "uppercase email must reach the same account");
}

#[tokio::test]
async fn correct_code_returns_a_working_token() {
    let app = common::spawn_app().await;
    app.seed().await;

    let token = app.login(common::ADMIN_EMAIL, common::ADMIN_PASSWORD).await;
    assert!(!token.is_empty());

    let res = app.get_auth("/tasks", &token).await;
    assert_eq!(res.status(), 200, "the issued token must authenticate a real request");
}

#[tokio::test]
async fn incorrect_code_is_rejected() {
    let app = common::spawn_app().await;
    app.seed().await;

    let challenge_id = app.begin_login(common::ADMIN_EMAIL, common::ADMIN_PASSWORD).await;
    let res = app
        .post("/auth/verify-2fa", &json!({ "challenge_id": challenge_id, "code": "000000" }))
        .await;
    assert_eq!(res.status(), 401);

    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"], "invalid_code");
}

#[tokio::test]
async fn a_wrong_code_does_not_burn_the_challenge() {
    let app = common::spawn_app().await;
    app.seed().await;

    let challenge_id = app.begin_login(common::ADMIN_EMAIL, common::ADMIN_PASSWORD).await;
    let code = app.latest_code_for(common::ADMIN_EMAIL).await;

    let wrong = app
        .post("/auth/verify-2fa", &json!({ "challenge_id": challenge_id, "code": "999999" }))
        .await;
    assert_eq!(wrong.status(), 401);

    let right = app
        .post("/auth/verify-2fa", &json!({ "challenge_id": challenge_id, "code": code }))
        .await;
    assert_eq!(right.status(), 200, "a typo must not invalidate the challenge");
}

#[tokio::test]
async fn expired_code_is_rejected() {
    let app = common::spawn_app().await;
    app.seed().await;

    let challenge_id = app.begin_login(common::ADMIN_EMAIL, common::ADMIN_PASSWORD).await;
    let code = app.latest_code_for(common::ADMIN_EMAIL).await;
    app.expire_challenge(challenge_id).await;

    let res = app
        .post("/auth/verify-2fa", &json!({ "challenge_id": challenge_id, "code": code }))
        .await;
    assert_eq!(res.status(), 401);

    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"], "challenge_expired", "expiry must be reported distinctly");
}

#[tokio::test]
async fn reused_code_is_rejected() {
    let app = common::spawn_app().await;
    app.seed().await;

    let challenge_id = app.begin_login(common::ADMIN_EMAIL, common::ADMIN_PASSWORD).await;
    let code = app.latest_code_for(common::ADMIN_EMAIL).await;

    let first = app
        .post("/auth/verify-2fa", &json!({ "challenge_id": challenge_id, "code": code.clone() }))
        .await;
    assert_eq!(first.status(), 200);

    let replay = app
        .post("/auth/verify-2fa", &json!({ "challenge_id": challenge_id, "code": code }))
        .await;
    assert_eq!(replay.status(), 401, "a correct code must work exactly once");

    let body: serde_json::Value = replay.json().await.unwrap();
    assert_eq!(
        body["error"]["code"], "challenge_already_used",
        "replay must be distinguishable from an unknown challenge"
    );
}

#[tokio::test]
async fn unknown_challenge_is_rejected() {
    let app = common::spawn_app().await;
    app.seed().await;

    let res = app
        .post("/auth/verify-2fa", &json!({ "challenge_id": Uuid::new_v4(), "code": "123456" }))
        .await;
    assert_eq!(res.status(), 401);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"], "invalid_challenge");
}

#[tokio::test]
async fn attempt_cap_burns_the_challenge() {
    let app = common::spawn_app().await;
    app.seed().await;

    let challenge_id = app.begin_login(common::ADMIN_EMAIL, common::ADMIN_PASSWORD).await;
    let code = app.latest_code_for(common::ADMIN_EMAIL).await;

    for _ in 0..5 {
        let res = app
            .post("/auth/verify-2fa", &json!({ "challenge_id": challenge_id, "code": "111111" }))
            .await;
        assert_eq!(res.status(), 401);
    }

    // Even the correct code must now fail: the challenge is burned.
    let res = app
        .post("/auth/verify-2fa", &json!({ "challenge_id": challenge_id, "code": code }))
        .await;
    assert_eq!(res.status(), 429);
}

#[tokio::test]
async fn logout_revokes_the_token() {
    let app = common::spawn_app().await;
    app.seed().await;

    let token = app.login(common::ADMIN_EMAIL, common::ADMIN_PASSWORD).await;
    assert_eq!(app.get_auth("/tasks", &token).await.status(), 200);

    let out = app.post_auth("/auth/logout", &token, &json!({})).await;
    assert_eq!(out.status(), 200);

    let after = app.get_auth("/tasks", &token).await;
    assert_eq!(after.status(), 401, "a revoked token must not authenticate even though it is unexpired");
}

#[tokio::test]
async fn requests_without_a_token_are_rejected() {
    let app = common::spawn_app().await;
    app.seed().await;

    assert_eq!(app.get("/tasks/view-my-tasks").await.status(), 401);
    assert_eq!(app.get("/tasks").await.status(), 401);
}

#[tokio::test]
async fn a_tampered_token_is_rejected() {
    let app = common::spawn_app().await;
    app.seed().await;

    let token = app.login(common::ADMIN_EMAIL, common::ADMIN_PASSWORD).await;
    let mut parts: Vec<&str> = token.split('.').collect();
    let forged = "eyJzdWIiOiJmb3JnZWQifQ";
    parts[1] = forged;
    let tampered = parts.join(".");

    assert_eq!(app.get_auth("/tasks", &tampered).await.status(), 401);
}
