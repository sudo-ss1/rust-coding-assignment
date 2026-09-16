use crate::api;
use crate::state::AppState;
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use serde_json::json;

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok" }))
}

pub fn build_router(state: AppState) -> Router {
    let mut router = Router::new()
        .route("/health", get(health))
        .route("/seed/users", post(api::users::seed_users))
        .route("/auth/login", post(api::auth::login))
        .route("/auth/verify-2fa", post(api::auth::verify_2fa))
        .route("/auth/logout", post(api::auth::logout))
        .route("/tasks", post(api::tasks::create_task).get(api::tasks::list_tasks))
        .route("/tasks/assign", post(api::tasks::assign_tasks))
        .route("/tasks/view-my-tasks", get(api::tasks::view_my_tasks))
        .route("/tasks/{id}", get(api::tasks::get_task))
        .route("/tasks/{id}", patch(api::tasks::update_task))
        .route("/tasks/{id}", delete(api::tasks::delete_task));

    // The dev mail view does not exist outside development, rather than existing
    // and refusing. There is no flag to get wrong in production.
    if state.config.is_dev() {
        router = router.route("/dev/email-logs/latest", get(api::dev::latest_email));
    }

    router.with_state(state)
}
