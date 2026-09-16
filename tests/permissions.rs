mod common;

use serde_json::json;

#[tokio::test]
async fn staff_cannot_create_a_task() {
    let app = common::spawn_app().await;
    app.seed().await;
    let bond = app.login(common::BOND_EMAIL, common::BOND_PASSWORD).await;

    let res = app
        .post_auth("/tasks", &bond, &json!({ "title": "Steal the plans", "priority": "high" }))
        .await;
    assert_eq!(res.status(), 403, "task creation is admin-only");

    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"], "forbidden");
}

#[tokio::test]
async fn staff_cannot_list_all_tasks() {
    let app = common::spawn_app().await;
    app.seed().await;
    let bond = app.login(common::BOND_EMAIL, common::BOND_PASSWORD).await;

    assert_eq!(app.get_auth("/tasks", &bond).await.status(), 403);
}

#[tokio::test]
async fn staff_cannot_assign_tasks() {
    let app = common::spawn_app().await;
    let seeded = app.seed().await;
    let bond_id = seeded["staff"]["id"].as_str().unwrap();

    let admin = app.login(common::ADMIN_EMAIL, common::ADMIN_PASSWORD).await;
    let task = app.create_task(&admin, "Brief M", "high").await;

    let bond = app.login(common::BOND_EMAIL, common::BOND_PASSWORD).await;
    let res = app
        .post_auth(
            "/tasks/assign",
            &bond,
            &json!({ "task_ids": [task], "assigned_to_id": bond_id }),
        )
        .await;
    assert_eq!(res.status(), 403, "staff must not be able to assign work to themselves");
}

#[tokio::test]
async fn staff_reading_an_unassigned_task_gets_404_not_403() {
    let app = common::spawn_app().await;
    app.seed().await;

    let admin = app.login(common::ADMIN_EMAIL, common::ADMIN_PASSWORD).await;
    let task = app.create_task(&admin, "Classified", "high").await;

    let bond = app.login(common::BOND_EMAIL, common::BOND_PASSWORD).await;
    let res = app.get_auth(&format!("/tasks/{task}"), &bond).await;

    // 403 would confirm the id exists. The endpoint must not disclose that.
    assert_eq!(res.status(), 404);
}

#[tokio::test]
async fn staff_may_update_the_status_of_their_own_task() {
    let app = common::spawn_app().await;
    let seeded = app.seed().await;
    let bond_id = seeded["staff"]["id"].as_str().unwrap();

    let admin = app.login(common::ADMIN_EMAIL, common::ADMIN_PASSWORD).await;
    let task = app.create_task(&admin, "Tail the courier", "medium").await;
    app.post_auth("/tasks/assign", &admin, &json!({ "task_ids": [task], "assigned_to_id": bond_id }))
        .await;

    let bond = app.login(common::BOND_EMAIL, common::BOND_PASSWORD).await;
    let res = app
        .patch_auth(&format!("/tasks/{task}"), &bond, &json!({ "status": "in_progress" }))
        .await;
    assert_eq!(res.status(), 200);

    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["status"], "in_progress");
}

#[tokio::test]
async fn staff_may_not_rewrite_fields_other_than_status() {
    let app = common::spawn_app().await;
    let seeded = app.seed().await;
    let bond_id = seeded["staff"]["id"].as_str().unwrap();

    let admin = app.login(common::ADMIN_EMAIL, common::ADMIN_PASSWORD).await;
    let task = app.create_task(&admin, "Original title", "low").await;
    app.post_auth("/tasks/assign", &admin, &json!({ "task_ids": [task], "assigned_to_id": bond_id }))
        .await;

    let bond = app.login(common::BOND_EMAIL, common::BOND_PASSWORD).await;
    let res = app
        .patch_auth(&format!("/tasks/{task}"), &bond, &json!({ "title": "Rewritten" }))
        .await;
    assert_eq!(res.status(), 403, "staff may move status only");
}

#[tokio::test]
async fn staff_cannot_update_someone_elses_task() {
    let app = common::spawn_app().await;
    app.seed().await;

    let admin = app.login(common::ADMIN_EMAIL, common::ADMIN_PASSWORD).await;
    let task = app.create_task(&admin, "Not yours", "high").await;

    let bond = app.login(common::BOND_EMAIL, common::BOND_PASSWORD).await;
    let res = app
        .patch_auth(&format!("/tasks/{task}"), &bond, &json!({ "status": "done" }))
        .await;
    assert_eq!(res.status(), 404);
}

#[tokio::test]
async fn staff_cannot_delete_a_task() {
    let app = common::spawn_app().await;
    let seeded = app.seed().await;
    let bond_id = seeded["staff"]["id"].as_str().unwrap();

    let admin = app.login(common::ADMIN_EMAIL, common::ADMIN_PASSWORD).await;
    let task = app.create_task(&admin, "Persistent", "medium").await;
    app.post_auth("/tasks/assign", &admin, &json!({ "task_ids": [task], "assigned_to_id": bond_id }))
        .await;

    let bond = app.login(common::BOND_EMAIL, common::BOND_PASSWORD).await;
    let res = app
        .client
        .delete(app.url(&format!("/tasks/{task}")))
        .bearer_auth(&bond)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403, "deletion is admin-only even for one's own task");
}

#[tokio::test]
async fn assignment_is_all_or_nothing() {
    let app = common::spawn_app().await;
    let seeded = app.seed().await;
    let bond_id = seeded["staff"]["id"].as_str().unwrap();

    let admin = app.login(common::ADMIN_EMAIL, common::ADMIN_PASSWORD).await;
    let real = app.create_task(&admin, "Real task", "high").await;
    let ghost = uuid::Uuid::new_v4();

    let res = app
        .post_auth(
            "/tasks/assign",
            &admin,
            &json!({ "task_ids": [real, ghost], "assigned_to_id": bond_id }),
        )
        .await;
    assert_eq!(res.status(), 404, "an unknown id must fail the whole batch");

    // The valid half must not have been applied.
    let bond = app.login(common::BOND_EMAIL, common::BOND_PASSWORD).await;
    let view = app.get_auth("/tasks/view-my-tasks", &bond).await;
    let body: serde_json::Value = view.json().await.unwrap();
    assert_eq!(
        body["summary"]["total_assigned_tasks"], 0,
        "a rejected batch must leave nothing assigned"
    );
}

#[tokio::test]
async fn assigning_to_an_unknown_user_is_rejected() {
    let app = common::spawn_app().await;
    app.seed().await;

    let admin = app.login(common::ADMIN_EMAIL, common::ADMIN_PASSWORD).await;
    let task = app.create_task(&admin, "Orphan", "low").await;

    let res = app
        .post_auth(
            "/tasks/assign",
            &admin,
            &json!({ "task_ids": [task], "assigned_to_id": uuid::Uuid::new_v4() }),
        )
        .await;
    assert_eq!(res.status(), 404);
}

#[tokio::test]
async fn an_empty_title_is_rejected() {
    let app = common::spawn_app().await;
    app.seed().await;
    let admin = app.login(common::ADMIN_EMAIL, common::ADMIN_PASSWORD).await;

    let res = app
        .post_auth("/tasks", &admin, &json!({ "title": "   ", "priority": "high" }))
        .await;
    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn an_invalid_priority_is_rejected() {
    let app = common::spawn_app().await;
    app.seed().await;
    let admin = app.login(common::ADMIN_EMAIL, common::ADMIN_PASSWORD).await;

    let res = app
        .post_auth("/tasks", &admin, &json!({ "title": "Bad priority", "priority": "urgent" }))
        .await;
    assert_eq!(res.status(), 422, "an out-of-range enum must not reach the database");
}
