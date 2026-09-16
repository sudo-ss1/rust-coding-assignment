# Task Management API Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Rust backend API with password + emailed-OTP two-factor login, admin/staff role-based permissions, admin-driven task assignment, and a Redis-cached `view-my-tasks` read with correct invalidation.

**Architecture:** An axum HTTP layer whose handlers contain no SQL and no Redis calls; repositories own all SQL; a cache module owns all Redis. Two-factor challenges live in Postgres so that replay is detectable, leaving Redis responsible only for the read cache and the logout denylist. Authorization is carried by an `AdminUser` extractor, so an admin-only route enforces its rule in the handler signature rather than in a body that can be edited.

**Tech Stack:** Rust 1.96 (edition 2024), axum 0.8, tokio, sqlx 0.9 (Postgres), redis 1.x, jsonwebtoken 11, argon2 0.6, serde, thiserror 2, tracing.

**Spec:** `docs/superpowers/specs/2026-09-16-task-management-api-design.md`

## Global Constraints

Every task's requirements implicitly include this section.

- **Rust edition 2024**, toolchain 1.96. Do not change `edition` in `Cargo.toml`.
- **Exact dependency versions and features** are fixed by Task 1. These were resolved and compile-verified against this machine — do not substitute versions from memory.
- **`jsonwebtoken` MUST enable the `rust_crypto` feature.** Its default features are `["use_pem"]` only, which contains no crypto provider. Without it the crate compiles and then **panics at runtime** with "Could not automatically determine the process-level CryptoProvider". Verified on this machine.
- **`argon2` 0.6 API:** `Argon2::default().hash_password(pw.as_bytes())` takes **no salt argument** (it generates one via `getrandom`) and returns `password_hash::phc::PasswordHash`. Verification is `Argon2::default().verify_password(pw.as_bytes(), hash_str)` where `hash_str: &str` — there is no need to parse a `PasswordHash` first. Do not write the argon2 0.5 two-argument form; it does not compile.
- **Every SQL lookup by email MUST bind `$1::citext`, not `$1`.** sqlx binds `&str` as `text`, and `citext = text` resolves to the case-sensitive `texteq`. Verified: without the cast the query returns zero rows for a case variant while the row exists.
- **axum 0.8 `FromRequestParts` needs no `#[async_trait]`.** Write `impl FromRequestParts<AppState> for T` with a plain `async fn from_request_parts`.
- **No `unwrap()` or `expect()` in `src/`** outside `main.rs` startup, where failing loudly is intended. Handlers return `Result<_, AppError>`.
- **Database:** `postgres://test:test@127.0.0.1:5432/task_management`. Tests use `task_management_test`. The Postgres on port **5433** is a different, unusable instance (peer auth, no matching role) — do not target it.
- **Redis:** `redis://127.0.0.1:6379`.
- **Counts in the validation flow are equalities, never lower bounds.** Assert `== 5`, `== 3`, `== 3`. "At least 3" would pass while authorization leaks a fourth task.
- **Commit after every task.** Message format `feat: <what>` or `test: <what>`.

---

## File Structure

| File | Responsibility |
|---|---|
| `Cargo.toml` | Pinned dependencies and features |
| `.env.example` | Documented environment variables |
| `src/main.rs` | Startup only: load config, build pools, bind listener |
| `src/config.rs` | Environment → typed `Config`, fails loudly |
| `src/error.rs` | `AppError` + the single `IntoResponse` mapping |
| `src/state.rs` | `AppState { pg, redis, config }` |
| `src/routes.rs` | Router assembly; mounts dev-only routes |
| `src/domain/mod.rs` | `Role`, `TaskStatus`, `TaskPriority`, `User`, `Task` — pure types |
| `src/api/auth.rs` | login, verify-2fa, logout |
| `src/api/users.rs` | seeding |
| `src/api/tasks.rs` | create, list, assign, update, delete, view-my-tasks |
| `src/api/dev.rs` | email-logs/latest |
| `src/auth/password.rs` | Argon2 hash/verify |
| `src/auth/jwt.rs` | Claims, encode, decode |
| `src/auth/twofa.rs` | Code generation and challenge verification rules |
| `src/auth/extractors.rs` | `AuthUser`, `AdminUser` |
| `src/repo/users.rs` | All user SQL |
| `src/repo/tasks.rs` | All task SQL |
| `src/repo/challenges.rs` | All challenge SQL |
| `src/repo/email_logs.rs` | All email-log SQL |
| `src/cache/keys.rs` | Key construction — one function per key shape |
| `src/cache/client.rs` | Typed get/set/delete |
| `src/notify/mod.rs` | `EmailNotifier` trait + DB-backed implementation |
| `migrations/0001_init.sql` | Enums, tables, indexes, `updated_at` trigger |
| `tests/common/mod.rs` | Test harness: DB per run, truncation, HTTP client |
| `tests/auth.rs` | Auth and 2FA behaviour |
| `tests/permissions.rs` | RBAC matrix |
| `tests/cache.rs` | Cache hit/miss and invalidation |
| `tests/validation_flow.rs` | The 15-step end-to-end flow |

Dependencies run one way: `api` → {`repo`, `cache`, `auth`, `domain`}; those → `domain`.

---

## Task 1: Project scaffold, config, error mapping, health check

**Files:**
- Modify: `Cargo.toml`
- Create: `.env.example`, `src/config.rs`, `src/error.rs`, `src/state.rs`, `src/routes.rs`, `src/main.rs`, `src/lib.rs`
- Test: `tests/health.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `Config { database_url: String, redis_url: String, jwt_secret: String, jwt_ttl_seconds: i64, twofa_ttl_seconds: i64, twofa_max_attempts: i32, cache_ttl_seconds: u64, app_env: String, bind_addr: String, seed_admin_password: String, seed_bond_password: String }`
  - `Config::from_env() -> Result<Config, ConfigError>`
  - `Config::is_dev(&self) -> bool`
  - `AppError` enum with variants `Validation(String)`, `InvalidCredentials`, `TwoFactor(TwoFactorError)`, `Unauthenticated`, `Forbidden`, `NotFound`, `Conflict(String)`, `TooManyAttempts`, `Database(sqlx::Error)`, `Cache(redis::RedisError)`, `Internal(String)`
  - `TwoFactorError` enum with variants `InvalidChallenge`, `Expired`, `AlreadyUsed`, `InvalidCode`
  - `AppState { pg: PgPool, redis: ConnectionManager, config: Arc<Config> }`
  - `build_router(state: AppState) -> Router`

- [ ] **Step 1: Pin dependencies**

Replace the `[dependencies]` section of `Cargo.toml` with exactly this. These versions were resolved and compile-verified together on this machine.

```toml
[dependencies]
axum = "0.8"
tokio = { version = "1", features = ["full"] }
tower = "0.5"
tower-http = { version = "0.7", features = ["trace"] }
sqlx = { version = "0.9", default-features = false, features = [
    "runtime-tokio", "tls-rustls", "postgres", "uuid", "chrono", "macros", "migrate",
] }
redis = { version = "1", features = ["tokio-comp", "connection-manager"] }
jsonwebtoken = { version = "11", features = ["rust_crypto"] }
argon2 = "0.6"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
uuid = { version = "1", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
thiserror = "2"
rand = "0.9"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

[dev-dependencies]
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
```

- [ ] **Step 2: Write the failing test**

Create `tests/health.rs`:

```rust
mod common;

#[tokio::test]
async fn health_returns_ok() {
    let app = common::spawn_app().await;
    let res = reqwest::get(format!("{}/health", app.base_url)).await.unwrap();
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["status"], "ok");
}
```

Create `tests/common/mod.rs` with the minimal harness (Task 2 extends it):

```rust
use live_coding::{config::Config, routes::build_router, state::AppState};
use std::sync::Arc;

pub struct TestApp {
    pub base_url: String,
}

pub async fn spawn_app() -> TestApp {
    let config = Arc::new(Config::from_env().expect("test env must be configured"));
    let pg = sqlx::postgres::PgPoolOptions::new()
        .connect(&config.database_url).await.expect("connect postgres");
    let redis_client = redis::Client::open(config.redis_url.clone()).expect("redis url");
    let redis = redis::aio::ConnectionManager::new(redis_client).await.expect("connect redis");

    let state = AppState { pg, redis, config };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, build_router(state)).await.unwrap();
    });
    TestApp { base_url: format!("http://{addr}") }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test --test health`
Expected: FAIL — `live_coding` has no `config`, `routes`, or `state` module.

- [ ] **Step 4: Create `src/config.rs`**

```rust
use std::env;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("missing environment variable: {0}")]
    Missing(&'static str),
    #[error("invalid value for {0}: {1}")]
    Invalid(&'static str, String),
    #[error("JWT_SECRET must be at least 32 bytes, got {0}")]
    WeakSecret(usize),
}

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub redis_url: String,
    pub jwt_secret: String,
    pub jwt_ttl_seconds: i64,
    pub twofa_ttl_seconds: i64,
    pub twofa_max_attempts: i32,
    pub cache_ttl_seconds: u64,
    pub app_env: String,
    pub bind_addr: String,
    pub seed_admin_password: String,
    pub seed_bond_password: String,
}

fn required(key: &'static str) -> Result<String, ConfigError> {
    env::var(key).map_err(|_| ConfigError::Missing(key))
}

fn parsed<T: std::str::FromStr>(key: &'static str, default: T) -> Result<T, ConfigError> {
    match env::var(key) {
        Err(_) => Ok(default),
        Ok(raw) => raw.parse::<T>().map_err(|_| ConfigError::Invalid(key, raw)),
    }
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let jwt_secret = required("JWT_SECRET")?;
        if jwt_secret.len() < 32 {
            return Err(ConfigError::WeakSecret(jwt_secret.len()));
        }
        Ok(Self {
            database_url: required("DATABASE_URL")?,
            redis_url: required("REDIS_URL")?,
            jwt_secret,
            jwt_ttl_seconds: parsed("JWT_TTL_SECONDS", 900)?,
            twofa_ttl_seconds: parsed("TWOFA_TTL_SECONDS", 300)?,
            twofa_max_attempts: parsed("TWOFA_MAX_ATTEMPTS", 5)?,
            cache_ttl_seconds: parsed("CACHE_TTL_SECONDS", 60)?,
            app_env: env::var("APP_ENV").unwrap_or_else(|_| "production".into()),
            bind_addr: env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".into()),
            seed_admin_password: env::var("SEED_ADMIN_PASSWORD").unwrap_or_else(|_| "admin123".into()),
            seed_bond_password: env::var("SEED_BOND_PASSWORD").unwrap_or_else(|_| "bond007".into()),
        })
    }

    pub fn is_dev(&self) -> bool {
        self.app_env == "dev"
    }
}
```

The secret-length check lives here rather than at first use, so a misconfigured deployment fails at startup instead of at the first login.

- [ ] **Step 5: Create `src/error.rs`**

```rust
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum TwoFactorError {
    #[error("invalid challenge")]
    InvalidChallenge,
    #[error("challenge expired")]
    Expired,
    #[error("challenge already used")]
    AlreadyUsed,
    #[error("invalid code")]
    InvalidCode,
}

impl TwoFactorError {
    fn code(&self) -> &'static str {
        match self {
            Self::InvalidChallenge => "invalid_challenge",
            Self::Expired => "challenge_expired",
            Self::AlreadyUsed => "challenge_already_used",
            Self::InvalidCode => "invalid_code",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    Validation(String),
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error(transparent)]
    TwoFactor(#[from] TwoFactorError),
    #[error("authentication required")]
    Unauthenticated,
    #[error("forbidden")]
    Forbidden,
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    Conflict(String),
    #[error("too many attempts")]
    TooManyAttempts,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Cache(#[from] redis::RedisError),
    #[error("{0}")]
    Internal(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code, message) = match &self {
            AppError::Validation(m) => (StatusCode::BAD_REQUEST, "validation_error", m.clone()),
            AppError::InvalidCredentials =>
                (StatusCode::UNAUTHORIZED, "invalid_credentials", "invalid credentials".into()),
            AppError::TwoFactor(e) => (StatusCode::UNAUTHORIZED, e.code(), e.to_string()),
            AppError::Unauthenticated =>
                (StatusCode::UNAUTHORIZED, "unauthenticated", "authentication required".into()),
            AppError::Forbidden =>
                (StatusCode::FORBIDDEN, "forbidden", "forbidden".into()),
            AppError::NotFound =>
                (StatusCode::NOT_FOUND, "not_found", "not found".into()),
            AppError::Conflict(m) => (StatusCode::CONFLICT, "conflict", m.clone()),
            AppError::TooManyAttempts =>
                (StatusCode::TOO_MANY_REQUESTS, "too_many_attempts", "too many attempts".into()),
            AppError::Database(e) => {
                tracing::error!(error = %e, "database error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal_error", "internal error".into())
            }
            AppError::Cache(e) => {
                tracing::error!(error = %e, "cache error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal_error", "internal error".into())
            }
            AppError::Internal(m) => {
                tracing::error!(error = %m, "internal error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal_error", "internal error".into())
            }
        };
        (status, Json(json!({ "error": { "code": code, "message": message } }))).into_response()
    }
}
```

The three 500 variants log the real cause and return a generic message, so SQL text and connection strings never reach a client.

- [ ] **Step 6: Create `src/state.rs`, `src/routes.rs`, `src/lib.rs`, `src/main.rs`**

`src/state.rs`:

```rust
use crate::config::Config;
use redis::aio::ConnectionManager;
use sqlx::PgPool;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub pg: PgPool,
    pub redis: ConnectionManager,
    pub config: Arc<Config>,
}
```

`src/routes.rs`:

```rust
use crate::state::AppState;
use axum::{routing::get, Json, Router};
use serde_json::json;

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok" }))
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .with_state(state)
}
```

`src/lib.rs`:

```rust
pub mod config;
pub mod error;
pub mod routes;
pub mod state;
```

`src/main.rs`:

```rust
use live_coding::{config::Config, routes::build_router, state::AppState};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env()
            .add_directive("live_coding=info".parse()?))
        .init();

    let config = Arc::new(Config::from_env()?);
    let pg = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .connect(&config.database_url).await?;
    sqlx::migrate!("./migrations").run(&pg).await?;

    let redis_client = redis::Client::open(config.redis_url.clone())?;
    let redis = redis::aio::ConnectionManager::new(redis_client).await?;

    let bind = config.bind_addr.clone();
    let state = AppState { pg, redis, config };
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!("listening on {bind}");
    axum::serve(listener, build_router(state)).await?;
    Ok(())
}
```

`sqlx::migrate!` requires `migrations/` to exist. Create it now with a placeholder that Task 2 replaces:

```bash
mkdir -p migrations && touch migrations/.gitkeep
```

- [ ] **Step 7: Create `.env.example` and the local `.env`**

```bash
cat > .env.example <<'EOF'
DATABASE_URL=postgres://test:test@127.0.0.1:5432/task_management
REDIS_URL=redis://127.0.0.1:6379
JWT_SECRET=dev-secret-that-is-at-least-32-bytes-long
JWT_TTL_SECONDS=900
TWOFA_TTL_SECONDS=300
TWOFA_MAX_ATTEMPTS=5
CACHE_TTL_SECONDS=60
APP_ENV=dev
BIND_ADDR=127.0.0.1:3000
SEED_ADMIN_PASSWORD=admin123
SEED_BOND_PASSWORD=bond007
EOF
cp .env.example .env
printf '/target\n.env\n' > .gitignore
```

Create the two databases:

```bash
PGPASSWORD=test psql -h 127.0.0.1 -p 5432 -U test -d postgres \
  -c "CREATE DATABASE task_management;" -c "CREATE DATABASE task_management_test;"
```

- [ ] **Step 8: Run the test**

The test harness reads the environment, so export it first:

```bash
set -a && . ./.env && set +a && \
DATABASE_URL=postgres://test:test@127.0.0.1:5432/task_management_test \
cargo test --test health
```

Expected: PASS, 1 test.

- [ ] **Step 9: Commit**

```bash
git add Cargo.toml Cargo.lock .gitignore .env.example src/ tests/ migrations/
git commit -m "feat: scaffold config, error mapping, and health check"
```

---
