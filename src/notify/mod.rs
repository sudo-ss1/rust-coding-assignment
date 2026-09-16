use crate::repo::email_logs;
use sqlx::PgPool;

/// Outbound mail. A trait so tests can substitute a capturing fake and a real
/// SMTP transport can be added later without touching the auth handlers.
pub trait EmailNotifier {
    fn send_code(
        &self,
        to_email: &str,
        code: &str,
    ) -> impl std::future::Future<Output = Result<(), sqlx::Error>> + Send;
}

/// The development transport: every "sent" message becomes an `email_logs` row,
/// which is what `GET /dev/email-logs/latest` reads.
#[derive(Clone)]
pub struct DbNotifier {
    pool: PgPool,
}

impl DbNotifier {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl EmailNotifier for DbNotifier {
    async fn send_code(&self, to_email: &str, code: &str) -> Result<(), sqlx::Error> {
        let subject = "Your verification code";
        let body = format!("Your verification code is {code}. It expires shortly.");
        email_logs::insert(&self.pool, to_email, subject, &body, Some(code)).await?;
        tracing::info!(to = %to_email, "verification code sent");
        Ok(())
    }
}
