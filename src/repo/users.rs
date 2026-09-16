use crate::domain::{Role, User};
use sqlx::PgPool;
use uuid::Uuid;

// SQL is written as literal strings throughout: sqlx 0.9 requires it, and it
// keeps every query greppable in full rather than assembled at runtime.

/// Look up a user by email.
///
/// The `$1::citext` cast is required, not cosmetic. sqlx binds a Rust `&str` as
/// a `text` parameter, and `citext = text` resolves to the case-sensitive
/// `texteq`, so without the cast this returns no row for `Admin@example.com`
/// while that account exists.
pub async fn find_by_email(pool: &PgPool, email: &str) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as::<_, User>(
        "select id, full_name, email::text as email, hashed_password, role, created_at, updated_at \
         from users where email = $1::citext",
    )
    .bind(email)
    .fetch_optional(pool)
    .await
}

pub async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as::<_, User>(
        "select id, full_name, email::text as email, hashed_password, role, created_at, updated_at \
         from users where id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn exists(pool: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let row: Option<(Uuid,)> = sqlx::query_as("select id from users where id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

/// Create a user, or return the existing one if the email is already taken.
///
/// `on conflict do nothing` plus a follow-up read makes seeding idempotent
/// without a read-then-write race between two concurrent seed calls.
pub async fn create_if_absent(
    pool: &PgPool,
    full_name: &str,
    email: &str,
    hashed_password: &str,
    role: Role,
) -> Result<User, sqlx::Error> {
    let inserted = sqlx::query_as::<_, User>(
        "insert into users (id, full_name, email, hashed_password, role) \
         values ($1, $2, $3::citext, $4, $5) \
         on conflict (email) do nothing \
         returning id, full_name, email::text as email, hashed_password, role, created_at, updated_at",
    )
    .bind(Uuid::new_v4())
    .bind(full_name)
    .bind(email)
    .bind(hashed_password)
    .bind(role)
    .fetch_optional(pool)
    .await?;

    match inserted {
        Some(user) => Ok(user),
        None => find_by_email(pool, email)
            .await?
            .ok_or(sqlx::Error::RowNotFound),
    }
}
