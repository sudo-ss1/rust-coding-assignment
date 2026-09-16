use crate::auth::extractors::AuthUser;
use crate::auth::jwt::{Claims, encode_token};
use crate::auth::password::{hash_password, verify_dummy, verify_password};
use crate::auth::twofa::generate_code;
use crate::cache::client::Cache;
use crate::cache::keys;
use crate::error::{AppError, AppResult, TwoFactorError};
use crate::notify::{DbNotifier, EmailNotifier};
use crate::repo::{challenges, users};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub challenge_id: Uuid,
    pub expires_in: i64,
    pub message: String,
}

/// Step one of login. A correct password alone yields nothing usable: the
/// response carries a challenge id, never a token and never the code.
pub async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> AppResult<Json<LoginResponse>> {
    let user = users::find_by_email(&state.pg, &body.email).await?;

    let user = match user {
        Some(u) if verify_password(&body.password, &u.hashed_password) => u,
        Some(_) => return Err(AppError::InvalidCredentials),
        None => {
            // Spend the same work as a real verification so that response timing
            // does not disclose which emails are registered.
            verify_dummy();
            return Err(AppError::InvalidCredentials);
        }
    };

    let code = generate_code();
    let code_hash = hash_password(&code)
        .map_err(|e| AppError::Internal(format!("hashing 2fa code: {e}")))?;

    let challenge = challenges::create(
        &state.pg,
        user.id,
        &code_hash,
        state.config.twofa_ttl_seconds,
    )
    .await?;

    DbNotifier::new(state.pg.clone())
        .send_code(&user.email, &code)
        .await?;

    Ok(Json(LoginResponse {
        challenge_id: challenge.id,
        expires_in: state.config.twofa_ttl_seconds,
        message: "verification code sent".to_string(),
    }))
}

#[derive(Debug, Deserialize)]
pub struct VerifyRequest {
    pub challenge_id: Uuid,
    pub code: String,
}

#[derive(Debug, Serialize)]
pub struct VerifyResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: i64,
}

/// Step two. Checks run in the order attempts, expiry, consumed, code, so a
/// burned or expired challenge is rejected before any work is spent hashing the
/// submitted code.
pub async fn verify_2fa(
    State(state): State<AppState>,
    Json(body): Json<VerifyRequest>,
) -> AppResult<Json<VerifyResponse>> {
    let challenge = challenges::find(&state.pg, body.challenge_id)
        .await?
        .ok_or(TwoFactorError::InvalidChallenge)?;

    if challenge.attempts >= state.config.twofa_max_attempts {
        return Err(AppError::TooManyAttempts);
    }
    if challenge.expires_at <= Utc::now() {
        return Err(TwoFactorError::Expired.into());
    }
    if challenge.consumed_at.is_some() {
        return Err(TwoFactorError::AlreadyUsed.into());
    }
    if !verify_password(&body.code, &challenge.code_hash) {
        challenges::increment_attempts(&state.pg, challenge.id).await?;
        return Err(TwoFactorError::InvalidCode.into());
    }

    // Consuming is guarded inside the UPDATE, so of two concurrent submissions of
    // the same valid code exactly one gets a row back and mints a token.
    let user_id = challenges::consume(&state.pg, challenge.id)
        .await?
        .ok_or(TwoFactorError::AlreadyUsed)?;

    let user = users::find_by_id(&state.pg, user_id)
        .await?
        .ok_or(AppError::NotFound)?;

    let claims = Claims::new(user.id, user.role, state.config.jwt_ttl_seconds);
    let token = encode_token(&state.config.jwt_secret, &claims)
        .map_err(|e| AppError::Internal(format!("encoding token: {e}")))?;

    Ok(Json(VerifyResponse {
        access_token: token,
        token_type: "Bearer".to_string(),
        expires_in: state.config.jwt_ttl_seconds,
    }))
}

#[derive(Debug, Serialize)]
pub struct LogoutResponse {
    pub revoked: bool,
}

/// Revoke the presented token. The denylist entry expires exactly when the token
/// would have anyway, so the list cannot grow without bound.
pub async fn logout(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<Json<LogoutResponse>> {
    let ttl = user.claims.seconds_remaining();
    if ttl > 0 {
        Cache::new(state.redis.clone())
            .set_marker(&keys::denylist(user.jti), ttl)
            .await?;
    }
    Ok(Json(LogoutResponse { revoked: true }))
}
