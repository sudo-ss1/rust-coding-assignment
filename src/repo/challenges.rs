use crate::domain::TwoFactorChallenge;
use chrono::{Duration, Utc};
use sqlx::PgPool;
use uuid::Uuid;

pub async fn create(
    pool: &PgPool,
    user_id: Uuid,
    code_hash: &str,
    ttl_seconds: i64,
) -> Result<TwoFactorChallenge, sqlx::Error> {
    sqlx::query_as::<_, TwoFactorChallenge>(
        "insert into two_factor_challenges (id, user_id, code_hash, expires_at) \
         values ($1, $2, $3, $4) \
         returning id, user_id, code_hash, attempts, expires_at, consumed_at, created_at",
    )
    .bind(Uuid::new_v4())
    .bind(user_id)
    .bind(code_hash)
    .bind(Utc::now() + Duration::seconds(ttl_seconds))
    .fetch_optional(pool)
    .await?
    .ok_or(sqlx::Error::RowNotFound)
}

pub async fn find(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<TwoFactorChallenge>, sqlx::Error> {
    sqlx::query_as::<_, TwoFactorChallenge>(
        "select id, user_id, code_hash, attempts, expires_at, consumed_at, created_at \
         from two_factor_challenges where id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn increment_attempts(pool: &PgPool, id: Uuid) -> Result<i32, sqlx::Error> {
    let row: (i32,) = sqlx::query_as(
        "update two_factor_challenges set attempts = attempts + 1 \
         where id = $1 returning attempts",
    )
    .bind(id)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}

/// Mark the challenge used and return its owner, but only if it was not already
/// consumed.
///
/// The row is marked rather than deleted, which is what lets a replayed code be
/// reported as "already used" instead of being indistinguishable from an unknown
/// id. Because the guard is inside the UPDATE, two concurrent submissions of the
/// same valid code cannot both receive a token: the second gets no row back.
pub async fn consume(pool: &PgPool, id: Uuid) -> Result<Option<Uuid>, sqlx::Error> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        "update two_factor_challenges set consumed_at = now() \
         where id = $1 and consumed_at is null returning user_id",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| r.0))
}
