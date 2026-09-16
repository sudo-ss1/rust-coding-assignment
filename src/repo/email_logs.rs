use crate::domain::EmailLog;
use sqlx::PgPool;
use uuid::Uuid;

pub async fn insert(
    pool: &PgPool,
    to_email: &str,
    subject: &str,
    body: &str,
    code: Option<&str>,
) -> Result<EmailLog, sqlx::Error> {
    sqlx::query_as::<_, EmailLog>(
        "insert into email_logs (id, to_email, subject, body, code) \
         values ($1, $2, $3, $4, $5) \
         returning id, to_email, subject, body, code, created_at",
    )
    .bind(Uuid::new_v4())
    .bind(to_email)
    .bind(subject)
    .bind(body)
    .bind(code)
    .fetch_one(pool)
    .await
}

pub async fn latest(pool: &PgPool) -> Result<Option<EmailLog>, sqlx::Error> {
    sqlx::query_as::<_, EmailLog>(
        "select id, to_email, subject, body, code, created_at from email_logs \
         order by created_at desc, id desc limit 1",
    )
    .fetch_optional(pool)
    .await
}

pub async fn latest_for(pool: &PgPool, email: &str) -> Result<Option<EmailLog>, sqlx::Error> {
    sqlx::query_as::<_, EmailLog>(
        "select id, to_email, subject, body, code, created_at from email_logs \
         where to_email = $1 order by created_at desc, id desc limit 1",
    )
    .bind(email)
    .fetch_optional(pool)
    .await
}
