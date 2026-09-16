use crate::domain::EmailLog;
use crate::error::{AppError, AppResult};
use crate::repo::email_logs;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Query, State};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct LatestQuery {
    /// Optionally scope to one recipient, so a test that logs in as two users in
    /// sequence can read the right code rather than whichever was most recent.
    pub email: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct EmailLogResponse {
    pub to_email: String,
    pub subject: String,
    pub body: String,
    pub code: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<EmailLog> for EmailLogResponse {
    fn from(l: EmailLog) -> Self {
        Self {
            to_email: l.to_email,
            subject: l.subject,
            body: l.body,
            code: l.code,
            created_at: l.created_at,
        }
    }
}

/// Development-only view of the mock mail transport. This route is mounted only
/// when `APP_ENV=dev`; in any other environment it does not exist at all, rather
/// than existing and refusing, so there is nothing to misconfigure.
pub async fn latest_email(
    State(state): State<AppState>,
    Query(q): Query<LatestQuery>,
) -> AppResult<Json<EmailLogResponse>> {
    let log = match q.email {
        Some(email) => email_logs::latest_for(&state.pg, &email).await?,
        None => email_logs::latest(&state.pg).await?,
    };
    log.map(|l| Json(l.into())).ok_or(AppError::NotFound)
}
