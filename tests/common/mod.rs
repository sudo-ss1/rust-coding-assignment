#![allow(dead_code)]

use live_coding::{config::Config, routes::build_router, state::AppState};
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;

pub const ADMIN_EMAIL: &str = "admin@example.com";
pub const BOND_EMAIL: &str = "jamesbond@example.com";
pub const ADMIN_PASSWORD: &str = "admin123";
pub const BOND_PASSWORD: &str = "bond007";

pub struct TestApp {
    pub base_url: String,
    pub client: reqwest::Client,
    pub pg: sqlx::PgPool,
}

/// Start the real router on an ephemeral port against the test database.
///
/// Each app truncates the tables and clears this run's cache keys first, so tests
/// do not observe each other's data regardless of the order they run in.
pub async fn spawn_app() -> TestApp {
    unsafe {
        std::env::set_var("APP_ENV", "dev");
        if std::env::var("JWT_SECRET").is_err() {
            std::env::set_var("JWT_SECRET", "test-secret-that-is-at-least-32-bytes-long");
        }
        if std::env::var("DATABASE_URL").is_err() {
            std::env::set_var(
                "DATABASE_URL",
                "postgres://test:test@127.0.0.1:5432/task_management_test",
            );
        }
        if std::env::var("REDIS_URL").is_err() {
            std::env::set_var("REDIS_URL", "redis://127.0.0.1:6379");
        }
    }

    let config = Arc::new(Config::from_env().expect("test environment must be configured"));

    let pg = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&config.database_url)
        .await
        .expect("connect to the test database");

    sqlx::migrate!("./migrations")
        .run(&pg)
        .await
        .expect("run migrations");

    sqlx::query("truncate table email_logs, two_factor_challenges, tasks, users cascade")
        .execute(&pg)
        .await
        .expect("truncate");

    let redis_client = redis::Client::open(config.redis_url.clone()).expect("redis url");
    let redis = redis::aio::ConnectionManager::new(redis_client)
        .await
        .expect("connect to redis");

    // Truncating users does not clear Redis, and a cached list keyed by a user id
    // from a previous run would make a fresh request report a spurious hit.
    {
        let mut conn = redis.clone();
        let _: () = redis::cmd("FLUSHDB")
            .query_async(&mut conn)
            .await
            .expect("flush test cache");
    }

    let state = AppState { pg: pg.clone(), redis, config };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");

    tokio::spawn(async move {
        axum::serve(listener, build_router(state)).await.expect("serve");
    });

    TestApp {
        base_url: format!("http://{addr}"),
        client: reqwest::Client::new(),
        pg,
    }
}

impl TestApp {
    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    pub async fn post(&self, path: &str, body: &Value) -> reqwest::Response {
        self.client
            .post(self.url(path))
            .json(body)
            .send()
            .await
            .expect("request")
    }

    pub async fn post_auth(&self, path: &str, token: &str, body: &Value) -> reqwest::Response {
        self.client
            .post(self.url(path))
            .bearer_auth(token)
            .json(body)
            .send()
            .await
            .expect("request")
    }

    pub async fn patch_auth(&self, path: &str, token: &str, body: &Value) -> reqwest::Response {
        self.client
            .patch(self.url(path))
            .bearer_auth(token)
            .json(body)
            .send()
            .await
            .expect("request")
    }

    pub async fn get(&self, path: &str) -> reqwest::Response {
        self.client.get(self.url(path)).send().await.expect("request")
    }

    pub async fn get_auth(&self, path: &str, token: &str) -> reqwest::Response {
        self.client
            .get(self.url(path))
            .bearer_auth(token)
            .send()
            .await
            .expect("request")
    }

    pub async fn seed(&self) -> Value {
        let res = self.post("/seed/users", &json!({})).await;
        assert_eq!(res.status(), 200, "seeding must succeed");
        res.json().await.expect("seed json")
    }

    /// Read the code the mock transport "sent" to this address.
    pub async fn latest_code_for(&self, email: &str) -> String {
        let res = self
            .get(&format!("/dev/email-logs/latest?email={email}"))
            .await;
        assert_eq!(res.status(), 200, "dev email log must be available");
        let body: Value = res.json().await.expect("email log json");
        body["code"].as_str().expect("code present").to_string()
    }

    /// Start login and return the challenge id, asserting that no token is issued.
    pub async fn begin_login(&self, email: &str, password: &str) -> Uuid {
        let res = self
            .post("/auth/login", &json!({ "email": email, "password": password }))
            .await;
        assert_eq!(res.status(), 200, "login step one must succeed");
        let body: Value = res.json().await.expect("login json");
        assert!(
            body.get("access_token").is_none(),
            "login must not return a token before the second factor"
        );
        Uuid::parse_str(body["challenge_id"].as_str().expect("challenge_id"))
            .expect("challenge id is a uuid")
    }

    /// Complete both factors and return the access token.
    pub async fn login(&self, email: &str, password: &str) -> String {
        let challenge_id = self.begin_login(email, password).await;
        let code = self.latest_code_for(email).await;
        let res = self
            .post(
                "/auth/verify-2fa",
                &json!({ "challenge_id": challenge_id, "code": code }),
            )
            .await;
        assert_eq!(res.status(), 200, "verifying a correct code must succeed");
        let body: Value = res.json().await.expect("verify json");
        body["access_token"].as_str().expect("access_token").to_string()
    }

    pub async fn create_task(
        &self,
        token: &str,
        title: &str,
        priority: &str,
    ) -> Uuid {
        let res = self
            .post_auth(
                "/tasks",
                token,
                &json!({ "title": title, "description": "", "priority": priority }),
            )
            .await;
        assert_eq!(res.status(), 201, "admin must be able to create a task");
        let body: Value = res.json().await.expect("task json");
        Uuid::parse_str(body["id"].as_str().expect("id")).expect("task id is a uuid")
    }

    /// Force a challenge past its expiry without waiting out the TTL. Expiry is
    /// compared against the database clock, so this needs no injectable time
    /// source in production code.
    pub async fn expire_challenge(&self, challenge_id: Uuid) {
        sqlx::query("update two_factor_challenges set expires_at = now() - interval '1 second' where id = $1")
            .bind(challenge_id)
            .execute(&self.pg)
            .await
            .expect("backdate challenge");
    }
}
