mod common;

use serde_json::{Value, json};

/// The full acceptance flow, in order. Every count is asserted as an equality:
/// "at least 3" would pass while authorization leaked a fourth task into Bond's
/// view, which is exactly the failure this exists to catch.
#[tokio::test]
async fn full_validation_flow() {
    let app = common::spawn_app().await;

    // 1. Admin and James Bond can be created.
    let seeded = app.seed().await;
    let admin_id = seeded["admin"]["id"].as_str().expect("admin id").to_string();
    let bond_id = seeded["staff"]["id"].as_str().expect("staff id").to_string();
    assert_eq!(seeded["admin"]["role"], "admin");
    assert_eq!(seeded["staff"]["role"], "staff");
    assert_eq!(seeded["staff"]["email"], "jamesbond@example.com");

    // 2. Login creates a 2FA challenge and does not immediately return a JWT.
    let res = app
        .post("/auth/login", &json!({ "email": common::ADMIN_EMAIL, "password": common::ADMIN_PASSWORD }))
        .await;
    assert_eq!(res.status(), 200);
    let login_body: Value = res.json().await.unwrap();
    assert!(login_body["challenge_id"].is_string());
    assert!(login_body.get("access_token").is_none(), "step one must not issue a token");
    let challenge_id = login_body["challenge_id"].as_str().unwrap().to_string();

    // 3. The code is retrievable from the development mail log.
    let code = app.latest_code_for(common::ADMIN_EMAIL).await;
    assert_eq!(code.len(), 6);
    assert!(code.chars().all(|c| c.is_ascii_digit()));

    // 4. An incorrect code is rejected.
    let wrong = app
        .post("/auth/verify-2fa", &json!({ "challenge_id": challenge_id, "code": "000000" }))
        .await;
    assert_eq!(wrong.status(), 401);
    let wrong_body: Value = wrong.json().await.unwrap();
    assert_eq!(wrong_body["error"]["code"], "invalid_code");

    // 5. The correct code returns a JWT.
    let ok = app
        .post("/auth/verify-2fa", &json!({ "challenge_id": challenge_id, "code": code.clone() }))
        .await;
    assert_eq!(ok.status(), 200);
    let ok_body: Value = ok.json().await.unwrap();
    let admin_token = ok_body["access_token"].as_str().expect("token").to_string();
    assert_eq!(ok_body["token_type"], "Bearer");

    // 6. Reusing that same correct code is rejected.
    let replay = app
        .post("/auth/verify-2fa", &json!({ "challenge_id": challenge_id, "code": code }))
        .await;
    assert_eq!(replay.status(), 401);
    let replay_body: Value = replay.json().await.unwrap();
    assert_eq!(replay_body["error"]["code"], "challenge_already_used");

    // 7. An expired code is rejected.
    let expired_challenge = app.begin_login(common::ADMIN_EMAIL, common::ADMIN_PASSWORD).await;
    let expired_code = app.latest_code_for(common::ADMIN_EMAIL).await;
    app.expire_challenge(expired_challenge).await;
    let expired = app
        .post("/auth/verify-2fa", &json!({ "challenge_id": expired_challenge, "code": expired_code }))
        .await;
    assert_eq!(expired.status(), 401);
    let expired_body: Value = expired.json().await.unwrap();
    assert_eq!(expired_body["error"]["code"], "challenge_expired");

    // 8. Admin creates exactly 5 tasks.
    let specs = [
        ("Infiltrate the casino", "high"),
        ("Decode the transmission", "medium"),
        ("Recover the briefcase", "low"),
        ("Debrief with Q branch", "high"),
        ("File the expense report", "low"),
    ];
    let mut task_ids = Vec::new();
    for (title, priority) in specs {
        task_ids.push(app.create_task(&admin_token, title, priority).await);
    }
    assert_eq!(task_ids.len(), 5);

    let all: Value = app.get_auth("/tasks", &admin_token).await.json().await.unwrap();
    assert_eq!(all.as_array().unwrap().len(), 5, "exactly 5 tasks must exist");

    // 9. Admin assigns exactly 3 of them to James Bond, at high/medium/low.
    let assigned: Vec<_> = task_ids.iter().take(3).collect();
    let assign = app
        .post_auth(
            "/tasks/assign",
            &admin_token,
            &json!({ "task_ids": assigned, "assigned_to_id": bond_id }),
        )
        .await;
    assert_eq!(assign.status(), 200);
    let assign_body: Value = assign.json().await.unwrap();
    assert_eq!(assign_body["assigned"], 3);

    // 10. James Bond logs in through the same two-step flow.
    let bond_challenge = app.begin_login(common::BOND_EMAIL, common::BOND_PASSWORD).await;
    let bond_code = app.latest_code_for(common::BOND_EMAIL).await;
    let bond_verify = app
        .post("/auth/verify-2fa", &json!({ "challenge_id": bond_challenge, "code": bond_code }))
        .await;
    assert_eq!(bond_verify.status(), 200);
    let bond_body: Value = bond_verify.json().await.unwrap();
    let bond_token = bond_body["access_token"].as_str().expect("bond token").to_string();

    // 11. James Bond cannot create a task.
    let denied = app
        .post_auth("/tasks", &bond_token, &json!({ "title": "Self-assigned mission", "priority": "high" }))
        .await;
    assert_eq!(denied.status(), 403);

    // 12. Bond sees exactly 3 tasks, all his, and the first read is a miss.
    let first = app.get_auth("/tasks/view-my-tasks", &bond_token).await;
    assert_eq!(first.status(), 200);
    let first_body: Value = first.json().await.unwrap();

    assert_eq!(first_body["user"]["email"], "jamesbond@example.com");
    assert_eq!(first_body["user"]["role"], "staff");
    assert_eq!(first_body["summary"]["total_assigned_tasks"], 3);
    assert_eq!(first_body["cache"]["hit"], false);

    let tasks = first_body["tasks"].as_array().expect("tasks array");
    assert_eq!(tasks.len(), 3, "exactly 3 tasks, not at least 3");

    let mut priorities: Vec<&str> = Vec::new();
    for task in tasks {
        assert_eq!(task["assigned_to"], "jamesbond@example.com");
        assert_eq!(task["status"], "todo");
        assert!(task["id"].is_string());
        assert!(task["title"].is_string());
        priorities.push(task["priority"].as_str().unwrap());

        // The projection carries exactly the five contracted fields.
        let keys: Vec<&String> = task.as_object().unwrap().keys().collect();
        assert_eq!(keys.len(), 5, "unexpected fields in the task projection: {keys:?}");
    }
    assert_eq!(priorities, vec!["high", "medium", "low"]);

    // 13. The identical request is a hit, with identical data.
    let second: Value = app
        .get_auth("/tasks/view-my-tasks", &bond_token)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(second["cache"]["hit"], true);
    assert_eq!(second["tasks"], first_body["tasks"], "a hit must replay the same rows");
    assert_eq!(second["summary"], first_body["summary"]);
    assert_eq!(second["user"], first_body["user"]);

    // 14. Reassigning one away from Bond invalidates his cache.
    let moved = task_ids[0];
    app.post_auth(
        "/tasks/assign",
        &admin_token,
        &json!({ "task_ids": [moved], "assigned_to_id": admin_id }),
    )
    .await;

    let after_reassign: Value = app
        .get_auth("/tasks/view-my-tasks", &bond_token)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(after_reassign["cache"]["hit"], false, "reassignment must evict Bond's cached list");
    assert_eq!(
        after_reassign["summary"]["total_assigned_tasks"], 2,
        "a stale cache would still report 3"
    );

    // 15. A field update that changes no assignment still invalidates.
    let warm: Value = app
        .get_auth("/tasks/view-my-tasks", &bond_token)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(warm["cache"]["hit"], true);

    let still_bonds = task_ids[1];
    let patched = app
        .patch_auth(
            &format!("/tasks/{still_bonds}"),
            &admin_token,
            &json!({ "title": "Decode the transmission (urgent)" }),
        )
        .await;
    assert_eq!(patched.status(), 200);

    let after_update: Value = app
        .get_auth("/tasks/view-my-tasks", &bond_token)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(after_update["cache"]["hit"], false, "an edit must evict even with no reassignment");
    assert_eq!(after_update["summary"]["total_assigned_tasks"], 2);

    let titles: Vec<&str> = after_update["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["title"].as_str().unwrap())
        .collect();
    assert!(
        titles.contains(&"Decode the transmission (urgent)"),
        "the updated title must be visible, not the cached one: {titles:?}"
    );
}
