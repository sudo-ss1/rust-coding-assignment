use crate::auth::password::hash_password;
use crate::domain::Role;
use crate::error::{AppError, AppResult};
use crate::repo::users;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use serde::Serialize;
use uuid::Uuid;

pub const ADMIN_EMAIL: &str = "admin@example.com";
pub const BOND_EMAIL: &str = "jamesbond@example.com";

#[derive(Debug, Serialize)]
pub struct SeededUser {
    pub id: Uuid,
    pub email: String,
    pub role: Role,
}

#[derive(Debug, Serialize)]
pub struct SeedResponse {
    pub admin: SeededUser,
    pub staff: SeededUser,
}

/// Create the two fixed validation accounts.
///
/// Unauthenticated, because the validation flow calls it before any token
/// exists. It takes no role parameter and can create no account other than these
/// two, so it is not a privilege-escalation route. In a real deployment it would
/// be `APP_ENV`-gated like the dev email route, and the admin would be seeded by
/// migration instead.
///
/// Idempotent: a second call returns the same ids and changes nothing.
pub async fn seed_users(State(state): State<AppState>) -> AppResult<Json<SeedResponse>> {
    let admin_hash = hash_password(&state.config.seed_admin_password)
        .map_err(|e| AppError::Internal(format!("hashing admin password: {e}")))?;
    let bond_hash = hash_password(&state.config.seed_bond_password)
        .map_err(|e| AppError::Internal(format!("hashing staff password: {e}")))?;

    let admin = users::create_if_absent(
        &state.pg,
        "Admin",
        ADMIN_EMAIL,
        &admin_hash,
        Role::Admin,
    )
    .await?;

    let bond = users::create_if_absent(
        &state.pg,
        "James Bond",
        BOND_EMAIL,
        &bond_hash,
        Role::Staff,
    )
    .await?;

    Ok(Json(SeedResponse {
        admin: SeededUser { id: admin.id, email: admin.email, role: admin.role },
        staff: SeededUser { id: bond.id, email: bond.email, role: bond.role },
    }))
}
