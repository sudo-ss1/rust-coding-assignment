mod common;

use serde_json::{Value, json};

async fn body(res: reqwest::Response) -> Value {
    res.json().await.expect("json body")
}

#[tokio::test]
async fn first_read_misses_and_second_read_hits() {
    let app = common::spawn_app().await;
    let seeded = app.seed().await;
    let bond_id = seeded["staff"]["id"].as_str().unwrap();

    let admin = app.login(common::ADMIN_EMAIL, common::ADMIN_PASSWORD).await;
    let task = app.create_task(&admin, "Recon", "high").await;
    app.post_auth("/tasks/assign", &admin, &json!({ "task_ids": [task], "assigned_to_id": bond_id }))
        .await;

    let bond = app.login(common::BOND_EMAIL, common::BOND_PASSWORD).await;

    let first = body(app.get_auth("/tasks/view-my-tasks", &bond).await).await;
    assert_eq!(first["cache"]["hit"], false);

    let second = body(app.get_auth("/tasks/view-my-tasks", &bond).await).await;
    assert_eq!(second["cache"]["hit"], true);

    // A hit must replay the same data, not merely return something. Comparing
    // only the flag would pass even if the cache served an empty list.
    assert_eq!(first["tasks"], second["tasks"], "a cache hit must return identical data");
    assert_eq!(first["user"], second["user"]);
    assert_eq!(first["summary"], second["summary"]);
}

#[tokio::test]
async fn the_cache_is_per_user() {
    let app = common::spawn_app().await;
    let seeded = app.seed().await;
    let bond_id = seeded["staff"]["id"].as_str().unwrap();

    let admin = app.login(common::ADMIN_EMAIL, common::ADMIN_PASSWORD).await;
    let task = app.create_task(&admin, "Bond only", "high").await;
    app.post_auth("/tasks/assign", &admin, &json!({ "task_ids": [task], "assigned_to_id": bond_id }))
        .await;

    let bond = app.login(common::BOND_EMAIL, common::BOND_PASSWORD).await;
    let bond_view = body(app.get_auth("/tasks/view-my-tasks", &bond).await).await;
    assert_eq!(bond_view["summary"]["total_assigned_tasks"], 1);

    // Warming Bond's cache must not make the admin's first read a hit, nor show
    // the admin Bond's tasks.
    let admin_view = body(app.get_auth("/tasks/view-my-tasks", &admin).await).await;
    assert_eq!(admin_view["cache"]["hit"], false, "one user's cache must not serve another");
    assert_eq!(admin_view["summary"]["total_assigned_tasks"], 0);
}

#[tokio::test]
async fn assignment_invalidates_the_cache() {
    let app = common::spawn_app().await;
    let seeded = app.seed().await;
    let bond_id = seeded["staff"]["id"].as_str().unwrap();

    let admin = app.login(common::ADMIN_EMAIL, common::ADMIN_PASSWORD).await;
    let first_task = app.create_task(&admin, "First", "high").await;
    app.post_auth("/tasks/assign", &admin, &json!({ "task_ids": [first_task], "assigned_to_id": bond_id }))
        .await;

    let bond = app.login(common::BOND_EMAIL, common::BOND_PASSWORD).await;
    assert_eq!(body(app.get_auth("/tasks/view-my-tasks", &bond).await).await["cache"]["hit"], false);
    assert_eq!(body(app.get_auth("/tasks/view-my-tasks", &bond).await).await["cache"]["hit"], true);

    let second_task = app.create_task(&admin, "Second", "low").await;
    app.post_auth("/tasks/assign", &admin, &json!({ "task_ids": [second_task], "assigned_to_id": bond_id }))
        .await;

    let after = body(app.get_auth("/tasks/view-my-tasks", &bond).await).await;
    assert_eq!(after["cache"]["hit"], false, "assignment must evict the assignee's cached list");
    assert_eq!(after["summary"]["total_assigned_tasks"], 2);
}

#[tokio::test]
async fn an_update_that_changes_no_assignment_still_invalidates() {
    let app = common::spawn_app().await;
    let seeded = app.seed().await;
    let bond_id = seeded["staff"]["id"].as_str().unwrap();

    let admin = app.login(common::ADMIN_EMAIL, common::ADMIN_PASSWORD).await;
    let task = app.create_task(&admin, "Old title", "medium").await;
    app.post_auth("/tasks/assign", &admin, &json!({ "task_ids": [task], "assigned_to_id": bond_id }))
        .await;

    let bond = app.login(common::BOND_EMAIL, common::BOND_PASSWORD).await;
    app.get_auth("/tasks/view-my-tasks", &bond).await;
    assert_eq!(body(app.get_auth("/tasks/view-my-tasks", &bond).await).await["cache"]["hit"], true);

    app.patch_auth(&format!("/tasks/{task}"), &admin, &json!({ "title": "New title" }))
        .await;

    let after = body(app.get_auth("/tasks/view-my-tasks", &bond).await).await;
    assert_eq!(after["cache"]["hit"], false, "an edit must evict, because the payload embeds task fields");
    assert_eq!(after["tasks"][0]["title"], "New title", "the cached list must not keep serving the old title");
}

#[tokio::test]
async fn reassignment_invalidates_the_previous_assignee() {
    // The classic reassignment bug: the task moves, but the former assignee's
    // cached list keeps showing it until the TTL lapses.
    let app = common::spawn_app().await;
    let seeded = app.seed().await;
    let bond_id = seeded["staff"]["id"].as_str().unwrap();
    let admin_id = seeded["admin"]["id"].as_str().unwrap();

    let admin = app.login(common::ADMIN_EMAIL, common::ADMIN_PASSWORD).await;
    let task = app.create_task(&admin, "Handover", "high").await;
    app.post_auth("/tasks/assign", &admin, &json!({ "task_ids": [task], "assigned_to_id": bond_id }))
        .await;

    let bond = app.login(common::BOND_EMAIL, common::BOND_PASSWORD).await;
    app.get_auth("/tasks/view-my-tasks", &bond).await;
    assert_eq!(body(app.get_auth("/tasks/view-my-tasks", &bond).await).await["cache"]["hit"], true);

    // Move it away from Bond.
    app.post_auth("/tasks/assign", &admin, &json!({ "task_ids": [task], "assigned_to_id": admin_id }))
        .await;

    let after = body(app.get_auth("/tasks/view-my-tasks", &bond).await).await;
    assert_eq!(after["cache"]["hit"], false);
    assert_eq!(
        after["summary"]["total_assigned_tasks"], 0,
        "a stale cache would still show the task that moved away"
    );
}

#[tokio::test]
async fn deletion_invalidates_the_cache() {
    let app = common::spawn_app().await;
    let seeded = app.seed().await;
    let bond_id = seeded["staff"]["id"].as_str().unwrap();

    let admin = app.login(common::ADMIN_EMAIL, common::ADMIN_PASSWORD).await;
    let task = app.create_task(&admin, "Doomed", "low").await;
    app.post_auth("/tasks/assign", &admin, &json!({ "task_ids": [task], "assigned_to_id": bond_id }))
        .await;

    let bond = app.login(common::BOND_EMAIL, common::BOND_PASSWORD).await;
    app.get_auth("/tasks/view-my-tasks", &bond).await;
    assert_eq!(body(app.get_auth("/tasks/view-my-tasks", &bond).await).await["cache"]["hit"], true);

    let res = app
        .client
        .delete(app.url(&format!("/tasks/{task}")))
        .bearer_auth(&admin)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);

    let after = body(app.get_auth("/tasks/view-my-tasks", &bond).await).await;
    assert_eq!(after["cache"]["hit"], false);
    assert_eq!(after["summary"]["total_assigned_tasks"], 0);
}

#[tokio::test]
async fn the_response_comes_from_the_database() {
    // Guard against a hardcoded payload: a user with no assignments must get an
    // empty list, and the counts must track whatever the database actually holds.
    let app = common::spawn_app().await;
    let seeded = app.seed().await;
    let bond_id = seeded["staff"]["id"].as_str().unwrap();

    let bond = app.login(common::BOND_EMAIL, common::BOND_PASSWORD).await;
    let empty = body(app.get_auth("/tasks/view-my-tasks", &bond).await).await;
    assert_eq!(empty["summary"]["total_assigned_tasks"], 0);
    assert_eq!(empty["tasks"].as_array().unwrap().len(), 0);

    let admin = app.login(common::ADMIN_EMAIL, common::ADMIN_PASSWORD).await;
    let mut ids = Vec::new();
    for i in 0..4 {
        ids.push(app.create_task(&admin, &format!("Task {i}"), "medium").await);
    }
    app.post_auth("/tasks/assign", &admin, &json!({ "task_ids": ids, "assigned_to_id": bond_id }))
        .await;

    let filled = body(app.get_auth("/tasks/view-my-tasks", &bond).await).await;
    assert_eq!(filled["summary"]["total_assigned_tasks"], 4);
    assert_eq!(filled["tasks"].as_array().unwrap().len(), 4);
}
