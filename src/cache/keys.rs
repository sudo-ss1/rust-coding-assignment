use uuid::Uuid;

/// Cached `view-my-tasks` payload for one user.
pub fn my_tasks(user_id: Uuid) -> String {
    format!("cache:tasks:my:{user_id}")
}

/// Revoked JWT id.
pub fn denylist(jti: Uuid) -> String {
    format!("denylist:{jti}")
}
