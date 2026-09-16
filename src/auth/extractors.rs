use crate::auth::jwt::{Claims, decode_token};
use crate::cache::keys;
use crate::domain::Role;
use crate::error::AppError;
use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use redis::AsyncCommands;
use uuid::Uuid;

/// Any authenticated caller.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: Uuid,
    pub role: Role,
    pub jti: Uuid,
    pub claims: Claims,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or(AppError::Unauthenticated)?;

        let token = header
            .strip_prefix("Bearer ")
            .ok_or(AppError::Unauthenticated)?
            .trim();

        let claims = decode_token(&state.config.jwt_secret, token)
            .map_err(|_| AppError::Unauthenticated)?;

        // A revoked token must not authenticate even though its signature is valid.
        let mut redis = state.redis.clone();
        let revoked: bool = redis
            .exists(keys::denylist(claims.jti))
            .await
            .map_err(AppError::Cache)?;
        if revoked {
            return Err(AppError::Unauthenticated);
        }

        Ok(AuthUser {
            id: claims.sub,
            role: claims.role,
            jti: claims.jti,
            claims,
        })
    }
}

/// An authenticated caller who is an admin. A handler taking this cannot be
/// reached by a staff user, so the role rule lives in the signature rather than
/// in a body where a later edit could drop it.
#[derive(Debug, Clone)]
pub struct AdminUser(pub AuthUser);

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let user = AuthUser::from_request_parts(parts, state).await?;
        match user.role {
            Role::Admin => Ok(AdminUser(user)),
            Role::Staff => Err(AppError::Forbidden),
        }
    }
}
