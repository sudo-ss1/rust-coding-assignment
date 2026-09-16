use crate::auth::extractors::{AdminUser, AuthUser};
use crate::cache::client::Cache;
use crate::cache::keys;
use crate::domain::{AssignedTaskView, Priority, Role, Task, TaskStatus};
use crate::error::{AppError, AppResult};
use crate::repo::tasks::{self, TaskUpdate};
use crate::repo::users;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct CreateTaskRequest {
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub priority: Priority,
}

/// Create a task. `AdminUser` in the signature is the whole authorization rule:
/// a staff caller is rejected during extraction and never enters this body.
pub async fn create_task(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Json(body): Json<CreateTaskRequest>,
) -> AppResult<(axum::http::StatusCode, Json<Task>)> {
    if body.title.trim().is_empty() {
        return Err(AppError::Validation("title must not be empty".into()));
    }
    let task = tasks::create(
        &state.pg,
        body.title.trim(),
        &body.description,
        body.priority,
        admin.id,
    )
    .await?;
    Ok((axum::http::StatusCode::CREATED, Json(task)))
}

pub async fn list_tasks(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
) -> AppResult<Json<Vec<Task>>> {
    Ok(Json(tasks::list_all(&state.pg).await?))
}

#[derive(Debug, Deserialize)]
pub struct AssignRequest {
    pub task_ids: Vec<Uuid>,
    pub assigned_to_id: Uuid,
}

#[derive(Debug, Serialize)]
pub struct AssignResponse {
    pub assigned: usize,
    pub assigned_to_id: Uuid,
}

/// Assign a batch of tasks to one user. All or nothing: an unknown task id fails
/// the whole request rather than leaving the caller unsure which half applied.
pub async fn assign_tasks(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Json(body): Json<AssignRequest>,
) -> AppResult<Json<AssignResponse>> {
    if body.task_ids.is_empty() {
        return Err(AppError::Validation("task_ids must not be empty".into()));
    }
    if !users::exists(&state.pg, body.assigned_to_id).await? {
        return Err(AppError::NotFound);
    }

    let affected = tasks::assign_batch(&state.pg, &body.task_ids, body.assigned_to_id)
        .await?
        .ok_or(AppError::NotFound)?;

    invalidate(&state, affected).await;

    Ok(Json(AssignResponse {
        assigned: body.task_ids.len(),
        assigned_to_id: body.assigned_to_id,
    }))
}

#[derive(Debug, Deserialize)]
pub struct UpdateTaskRequest {
    pub title: Option<String>,
    pub description: Option<String>,
    pub status: Option<TaskStatus>,
    pub priority: Option<Priority>,
}

/// Update a task. Admins may change anything; a staff caller may move only the
/// status, and only on a task assigned to them.
pub async fn update_task(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateTaskRequest>,
) -> AppResult<Json<Task>> {
    let patch = TaskUpdate {
        title: body.title,
        description: body.description,
        status: body.status,
        priority: body.priority,
    };
    if patch.is_empty() {
        return Err(AppError::Validation("no fields to update".into()));
    }

    let existing = tasks::find(&state.pg, id).await?;

    let existing = match (existing, user.role) {
        (Some(t), Role::Admin) => t,
        // A staff caller must not learn whether a task they cannot touch exists,
        // so a missing task and someone else's task give the same answer.
        (Some(t), Role::Staff) if t.assigned_to_id == Some(user.id) => t,
        (Some(_), Role::Staff) | (None, _) => return Err(AppError::NotFound),
    };

    if user.role == Role::Staff && patch.touches_more_than_status() {
        return Err(AppError::Forbidden);
    }

    let updated = tasks::update(&state.pg, id, &patch).await?;

    // The cached payload embeds each task's fields, so an edit that changes no
    // assignment still makes the assignee's cached list wrong.
    let mut affected = Vec::new();
    affected.extend(existing.assigned_to_id);
    affected.extend(updated.assigned_to_id);
    invalidate(&state, affected).await;

    Ok(Json(updated))
}

pub async fn delete_task(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Path(id): Path<Uuid>,
) -> AppResult<axum::http::StatusCode> {
    let existing = tasks::find(&state.pg, id).await?.ok_or(AppError::NotFound)?;
    if !tasks::delete(&state.pg, id).await? {
        return Err(AppError::NotFound);
    }
    invalidate(&state, existing.assigned_to_id).await;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// Single-task read, scoped for staff.
pub async fn get_task(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Task>> {
    let task = tasks::find(&state.pg, id).await?.ok_or(AppError::NotFound)?;
    match user.role {
        Role::Admin => Ok(Json(task)),
        Role::Staff if task.assigned_to_id == Some(user.id) => Ok(Json(task)),
        // 404 rather than 403: the endpoint must not confirm which ids exist.
        Role::Staff => Err(AppError::NotFound),
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct UserSummary {
    pub email: String,
    pub role: Role,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct TasksSummary {
    pub total_assigned_tasks: usize,
}

/// Everything in the `view-my-tasks` response except `cache`. This is exactly
/// what gets stored in Redis, so a hit and a miss differ in the `cache` block
/// and nowhere else.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct MyTasksPayload {
    pub user: UserSummary,
    pub tasks: Vec<AssignedTaskView>,
    pub summary: TasksSummary,
}

#[derive(Debug, Serialize)]
pub struct CacheMeta {
    pub hit: bool,
}

#[derive(Debug, Serialize)]
pub struct MyTasksResponse {
    #[serde(flatten)]
    pub payload: MyTasksPayload,
    pub cache: CacheMeta,
}

/// The caller's assigned tasks, served from Redis when warm.
///
/// `cache.hit` is derived from whether Redis returned a value and is never part
/// of the stored payload, so a cold cache cannot serve a stale `true`.
pub async fn view_my_tasks(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<Json<MyTasksResponse>> {
    let cache = Cache::new(state.redis.clone());
    let key = keys::my_tasks(user.id);

    if let Some(payload) = cache.get_json::<MyTasksPayload>(&key).await? {
        return Ok(Json(MyTasksResponse { payload, cache: CacheMeta { hit: true } }));
    }

    let me = users::find_by_id(&state.pg, user.id)
        .await?
        .ok_or(AppError::NotFound)?;
    let rows = tasks::list_assigned_to(&state.pg, user.id).await?;

    let payload = MyTasksPayload {
        user: UserSummary { email: me.email, role: me.role },
        summary: TasksSummary { total_assigned_tasks: rows.len() },
        tasks: rows,
    };

    cache
        .set_json(&key, &payload, state.config.cache_ttl_seconds)
        .await?;

    Ok(Json(MyTasksResponse { payload, cache: CacheMeta { hit: false } }))
}

/// Invalidation must not fail a write that already committed: the cache is a
/// read optimisation with a bounded TTL, so a brief Redis outage costs staleness,
/// not a failed request that the client would retry against already-changed data.
async fn invalidate(state: &AppState, user_ids: impl IntoIterator<Item = Uuid>) {
    if let Err(e) = Cache::new(state.redis.clone())
        .invalidate_my_tasks(user_ids)
        .await
    {
        tracing::error!(error = %e, "cache invalidation failed; entries will expire by TTL");
    }
}
