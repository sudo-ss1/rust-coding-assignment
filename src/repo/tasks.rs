use crate::domain::{AssignedTaskView, Priority, Task, TaskStatus};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

pub async fn create(
    pool: &PgPool,
    title: &str,
    description: &str,
    priority: Priority,
    created_by_id: Uuid,
) -> Result<Task, sqlx::Error> {
    sqlx::query_as::<_, Task>(
        "insert into tasks (id, title, description, priority, created_by_id) \
         values ($1, $2, $3, $4, $5) \
         returning id, title, description, status, priority, created_by_id, assigned_to_id, created_at, updated_at",
    )
    .bind(Uuid::new_v4())
    .bind(title)
    .bind(description)
    .bind(priority)
    .bind(created_by_id)
    .fetch_one(pool)
    .await
}

pub async fn list_all(pool: &PgPool) -> Result<Vec<Task>, sqlx::Error> {
    sqlx::query_as::<_, Task>(
        "select id, title, description, status, priority, created_by_id, assigned_to_id, created_at, updated_at from tasks order by created_at asc, id asc",
    )
    .fetch_all(pool)
    .await
}

pub async fn find(pool: &PgPool, id: Uuid) -> Result<Option<Task>, sqlx::Error> {
    sqlx::query_as::<_, Task>("select id, title, description, status, priority, created_by_id, assigned_to_id, created_at, updated_at from tasks where id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// Tasks assigned to one user, projected for `view-my-tasks`.
///
/// The restriction is in the WHERE clause rather than applied to a wider result
/// in Rust, so there is no code path that reads another user's tasks into memory
/// for a staff caller. The assignee's email comes from a join, not from N+1
/// follow-up queries.
///
/// Ordering is total (`created_at`, then `id`) so the payload is byte-identical
/// on repeat reads, which is what makes a cache hit comparable to a miss.
pub async fn list_assigned_to(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<Vec<AssignedTaskView>, sqlx::Error> {
    sqlx::query_as::<_, AssignedTaskView>(
        "select t.id, t.title, t.status, t.priority, u.email::text as assigned_to \
         from tasks t \
         join users u on u.id = t.assigned_to_id \
         where t.assigned_to_id = $1 \
         order by t.created_at asc, t.id asc",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}

/// Assign a batch of tasks to one user inside a single transaction, returning
/// every user whose cached list is now stale: the new assignee plus each
/// distinct previous assignee.
///
/// If any id is unknown the whole batch is rolled back and `None` is returned. A
/// partially applied batch would leave the caller unable to tell which half took
/// effect.
pub async fn assign_batch(
    pool: &PgPool,
    task_ids: &[Uuid],
    assignee_id: Uuid,
) -> Result<Option<Vec<Uuid>>, sqlx::Error> {
    let mut tx: Transaction<'_, Postgres> = pool.begin().await?;

    // Lock the rows and read who holds them now. `for update` keeps a concurrent
    // reassignment from changing the previous assignee between this read and the
    // write below, which would leave that user's cache un-invalidated.
    let existing: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as(
        "select id, assigned_to_id from tasks where id = any($1) for update",
    )
    .bind(task_ids)
    .fetch_all(&mut *tx)
    .await?;

    let distinct_requested: std::collections::HashSet<Uuid> = task_ids.iter().copied().collect();
    if existing.len() != distinct_requested.len() {
        tx.rollback().await?;
        return Ok(None);
    }

    sqlx::query("update tasks set assigned_to_id = $1 where id = any($2)")
        .bind(assignee_id)
        .bind(task_ids)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    // Every user whose cached list is now stale: the new assignee, plus each
    // distinct previous holder.
    let mut affected = vec![assignee_id];
    affected.extend(existing.into_iter().filter_map(|(_, prev)| prev));
    Ok(Some(affected))
}

pub struct TaskUpdate {
    pub title: Option<String>,
    pub description: Option<String>,
    pub status: Option<TaskStatus>,
    pub priority: Option<Priority>,
}

impl TaskUpdate {
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.description.is_none()
            && self.status.is_none()
            && self.priority.is_none()
    }

    /// True when the update touches anything other than `status`. Used to decide
    /// whether a staff caller, who may only move their own task's status, is
    /// permitted to make this change.
    pub fn touches_more_than_status(&self) -> bool {
        self.title.is_some() || self.description.is_some() || self.priority.is_some()
    }
}

/// Apply a partial update. `coalesce` leaves an omitted field untouched, so a
/// PATCH cannot blank a column the caller did not mention.
pub async fn update(pool: &PgPool, id: Uuid, patch: &TaskUpdate) -> Result<Task, sqlx::Error> {
    sqlx::query_as::<_, Task>(
        "update tasks set \
           title       = coalesce($2, title), \
           description = coalesce($3, description), \
           status      = coalesce($4, status), \
           priority    = coalesce($5, priority) \
         where id = $1 \
         returning id, title, description, status, priority, created_by_id, assigned_to_id, created_at, updated_at",
    )
    .bind(id)
    .bind(patch.title.as_deref())
    .bind(patch.description.as_deref())
    .bind(patch.status)
    .bind(patch.priority)
    .fetch_one(pool)
    .await
}

pub async fn delete(pool: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("delete from tasks where id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}
