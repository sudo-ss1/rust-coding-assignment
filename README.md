# Task Management API

A Rust backend for a small task-management workflow: password authentication,
two-factor login via an emailed one-time code, admin/staff role-based
permissions, admin-driven task assignment, and a Redis-cached read with explicit
invalidation.

## Stack

axum 0.8 · tokio · sqlx 0.9 (PostgreSQL) · redis · jsonwebtoken (HS256) ·
Argon2id · Rust 1.96, edition 2024.

## 1. Setup

Requires Rust 1.96+, a PostgreSQL, and a Redis. Both services must be reachable
before the server starts; it fails loudly at boot rather than at the first
request.

```bash
git clone <this repo> && cd rust-coding-assignment
cp .env.example .env
```

`.env` controls everything. Startup fails with a named error if a required
variable is missing, unparseable, or — for `JWT_SECRET` — shorter than 32 bytes.

| Variable | Default | Meaning |
|---|---|---|
| `DATABASE_URL` | `postgres://test:test@127.0.0.1:5432/task_management` | Postgres connection |
| `REDIS_URL` | `redis://127.0.0.1:6379` | Redis connection |
| `JWT_SECRET` | — | HS256 signing key, **minimum 32 bytes** |
| `JWT_TTL_SECONDS` | `900` | Access-token lifetime |
| `TWOFA_TTL_SECONDS` | `300` | How long a 2FA challenge stays valid |
| `TWOFA_MAX_ATTEMPTS` | `5` | Failed verifications before a challenge burns |
| `CACHE_TTL_SECONDS` | `60` | `view-my-tasks` cache TTL |
| `APP_ENV` | `production` | `dev` additionally mounts the dev mail route |
| `BIND_ADDR` | `127.0.0.1:3000` | Listen address |
| `SEED_ADMIN_PASSWORD` | `admin123` | Password for the seeded admin |
| `SEED_BOND_PASSWORD` | `bond007` | Password for the seeded staff user |

Create the databases (the second is only needed for the test suite):

```bash
psql "$DATABASE_URL" -c '\q' 2>/dev/null || \
  psql -h 127.0.0.1 -p 5432 -U test -d postgres \
    -c "CREATE DATABASE task_management;" \
    -c "CREATE DATABASE task_management_test;"
```

## 2. Migrations

There is no separate migrate step: `cargo run` applies every pending migration
at startup via `sqlx::migrate!`, before the listener binds. A boot that cannot
migrate does not start serving.

`migrations/0001_init.sql` creates the `citext` extension, the three enums
(`user_role`, `task_status`, `task_priority`), the four tables (`users`,
`tasks`, `two_factor_challenges`, `email_logs`), their indexes, and an
`updated_at` trigger shared by `users` and `tasks` so no handler can forget to
maintain it.

To apply them without starting the server, run the test suite, which migrates
the test database on setup.

## 3. Run

```bash
cargo run
```

```
INFO live_coding: listening on http://127.0.0.1:3000
```

Check it is up:

```bash
curl -s http://127.0.0.1:3000/health     # {"status":"ok"}
```

## 4. Seed

```bash
curl -s -X POST http://127.0.0.1:3000/seed/users | jq
```

```json
{
  "admin": { "id": "…", "email": "admin@example.com",     "role": "admin" },
  "staff": { "id": "…", "email": "jamesbond@example.com", "role": "staff" }
}
```

Idempotent — calling it again returns the same two ids and changes nothing. It
takes no role parameter and can create no account other than these two, so
although it is unauthenticated it is not a privilege-escalation route. In a real
deployment it would be `APP_ENV`-gated like the dev mail route, with the admin
seeded by migration instead.

## 5. Validation walkthrough

The whole flow by hand. Run with `APP_ENV=dev` so the mail log route exists.

```bash
BASE=http://127.0.0.1:3000

# --- accounts
IDS=$(curl -s -X POST $BASE/seed/users)
ADMIN_ID=$(echo "$IDS" | jq -r .admin.id)
BOND_ID=$(echo  "$IDS" | jq -r .staff.id)

# --- log in as admin: step one returns a challenge, deliberately NOT a token
CHALLENGE=$(curl -s -X POST $BASE/auth/login -H 'content-type: application/json' \
  -d '{"email":"admin@example.com","password":"admin123"}' | jq -r .challenge_id)

# --- read the code the mock transport "sent"
CODE=$(curl -s "$BASE/dev/email-logs/latest?email=admin@example.com" | jq -r .code)

# --- a wrong code is rejected
curl -s -X POST $BASE/auth/verify-2fa -H 'content-type: application/json' \
  -d "{\"challenge_id\":\"$CHALLENGE\",\"code\":\"000000\"}" | jq .error.code
# "invalid_code"

# --- step two returns the JWT
ADMIN=$(curl -s -X POST $BASE/auth/verify-2fa -H 'content-type: application/json' \
  -d "{\"challenge_id\":\"$CHALLENGE\",\"code\":\"$CODE\"}" | jq -r .access_token)

# --- replaying that same code now fails
curl -s -X POST $BASE/auth/verify-2fa -H 'content-type: application/json' \
  -d "{\"challenge_id\":\"$CHALLENGE\",\"code\":\"$CODE\"}" | jq .error.code
# "challenge_already_used"

# --- admin creates 5 tasks
for spec in "Infiltrate the casino:high" "Decode the transmission:medium" \
            "Recover the briefcase:low"  "Debrief with Q branch:high" \
            "File the expense report:low"; do
  curl -s -X POST $BASE/tasks -H "authorization: Bearer $ADMIN" \
    -H 'content-type: application/json' \
    -d "{\"title\":\"${spec%:*}\",\"priority\":\"${spec##*:}\"}" | jq -r .id
done > /tmp/task_ids

# --- assign exactly 3 of them to Bond
THREE=$(head -3 /tmp/task_ids | jq -R . | jq -sc .)
curl -s -X POST $BASE/tasks/assign -H "authorization: Bearer $ADMIN" \
  -H 'content-type: application/json' \
  -d "{\"task_ids\":$THREE,\"assigned_to_id\":\"$BOND_ID\"}" | jq

# --- Bond logs in the same two-step way
BC=$(curl -s -X POST $BASE/auth/login -H 'content-type: application/json' \
  -d '{"email":"jamesbond@example.com","password":"bond007"}' | jq -r .challenge_id)
BCODE=$(curl -s "$BASE/dev/email-logs/latest?email=jamesbond@example.com" | jq -r .code)
BOND=$(curl -s -X POST $BASE/auth/verify-2fa -H 'content-type: application/json' \
  -d "{\"challenge_id\":\"$BC\",\"code\":\"$BCODE\"}" | jq -r .access_token)

# --- Bond cannot create a task
curl -s -o /dev/null -w '%{http_code}\n' -X POST $BASE/tasks \
  -H "authorization: Bearer $BOND" -H 'content-type: application/json' \
  -d '{"title":"Self-assigned mission","priority":"high"}'
# 403

# --- Bond sees exactly 3 tasks; first read misses, second hits
curl -s $BASE/tasks/view-my-tasks -H "authorization: Bearer $BOND" | jq '{n:.summary.total_assigned_tasks, hit:.cache.hit}'
# { "n": 3, "hit": false }
curl -s $BASE/tasks/view-my-tasks -H "authorization: Bearer $BOND" | jq '{n:.summary.total_assigned_tasks, hit:.cache.hit}'
# { "n": 3, "hit": true }

# --- reassigning one away invalidates his cache
ONE=$(head -1 /tmp/task_ids)
curl -s -X POST $BASE/tasks/assign -H "authorization: Bearer $ADMIN" \
  -H 'content-type: application/json' \
  -d "{\"task_ids\":[\"$ONE\"],\"assigned_to_id\":\"$ADMIN_ID\"}" > /dev/null
curl -s $BASE/tasks/view-my-tasks -H "authorization: Bearer $BOND" | jq '{n:.summary.total_assigned_tasks, hit:.cache.hit}'
# { "n": 2, "hit": false }
```

Everything above is also asserted automatically — see
`tests/validation_flow.rs`, which runs the same fifteen steps against a live
Postgres and Redis.

### The final response

This is the actual output of the last step, captured from a running server —
not a hand-written example. `GET /tasks/view-my-tasks` as James Bond, after the
admin has created 5 tasks and assigned 3 of them:

```http
GET /tasks/view-my-tasks
Authorization: Bearer <JAMES_BOND_TOKEN>
```

```json
{
  "user": {
    "email": "jamesbond@example.com",
    "role": "staff"
  },
  "tasks": [
    {
      "id": "3572e32f-30a6-4a39-b12e-474c4106239c",
      "title": "Infiltrate the casino",
      "status": "todo",
      "priority": "high",
      "assigned_to": "jamesbond@example.com"
    },
    {
      "id": "9f9fb12b-72a9-42cd-b7fc-bfb8d8994880",
      "title": "Decode the transmission",
      "status": "todo",
      "priority": "medium",
      "assigned_to": "jamesbond@example.com"
    },
    {
      "id": "1922665b-fc11-48b8-a825-af0a78b5e21c",
      "title": "Recover the briefcase",
      "status": "todo",
      "priority": "low",
      "assigned_to": "jamesbond@example.com"
    }
  ],
  "summary": {
    "total_assigned_tasks": 3
  },
  "cache": {
    "hit": false
  }
}
```

Three tasks, all assigned to Bond, none of the other two leaking through. The
task ids are real uuids from that run and will differ on yours.

**Calling the identical request again** returns the same body with one field
changed:

```json
  "cache": {
    "hit": true
  }
```

Everything outside the `cache` block is byte-identical between the two calls —
verified by diffing them, and asserted in
`tests/cache.rs::first_read_misses_and_second_read_hits`. A hit replays the
previous database result; it does not return a different or emptier list.

**After the admin reassigns one task away from Bond**, his next call shows the
cache correctly evicted rather than serving a stale three:

```json
{
  "tasks": ["Decode the transmission", "Recover the briefcase"],
  "summary": { "total_assigned_tasks": 2 },
  "cache": { "hit": false }
}
```

This is the step that distinguishes a cache wired to the data from one that is
merely warm: a stale cache would still report 3.

## 6. Endpoints

| Endpoint | Auth | Purpose |
|---|---|---|
| `POST /seed/users` | none | Idempotently create Admin and James Bond |
| `POST /auth/login` | none | Verify password, create a 2FA challenge, send the code |
| `GET /dev/email-logs/latest` | none, **dev only** | Latest "sent" mail, including the code |
| `POST /auth/verify-2fa` | none | Exchange a valid code for a JWT |
| `POST /auth/logout` | bearer | Revoke the presented token |
| `POST /tasks` | admin | Create a task |
| `GET /tasks` | admin | List every task |
| `GET /tasks/{id}` | admin, or assignee | Read one task |
| `POST /tasks/assign` | admin | Batch-assign tasks to one user |
| `PATCH /tasks/{id}` | admin; assignee for status only | Update a task |
| `DELETE /tasks/{id}` | admin | Delete a task |
| `GET /tasks/view-my-tasks` | bearer | The caller's tasks, with cache metadata |

`GET /dev/email-logs/latest` is mounted only when `APP_ENV=dev`. Outside
development the route does not exist at all, rather than existing and refusing.

### `GET /tasks/view-my-tasks`

```json
{
  "user":    { "email": "jamesbond@example.com", "role": "staff" },
  "tasks": [
    { "id": "…", "title": "…", "status": "todo",
      "priority": "high", "assigned_to": "jamesbond@example.com" }
  ],
  "summary": { "total_assigned_tasks": 3 },
  "cache":   { "hit": false }
}
```

Calling it twice in a row returns `hit: false` then `hit: true`, with identical
data. The rows always originate from Postgres; a hit replays a previous database
result and nothing else.

## 7. Permissions

| Action | admin | staff |
|---|---|---|
| create task | yes | no (403) |
| list all tasks | yes | no (403) |
| view own assigned tasks | yes | yes, scoped to self |
| read one task | yes | only if assignee, else 404 |
| assign / reassign | yes | no (403) |
| update status | yes | only if assignee |
| update title/description/priority | yes | no (403) |
| delete task | yes | no (403) |

Staff scoping is a `WHERE assigned_to_id = $1` in SQL, not a filter applied to a
wider result set in Rust, so no code path reads another user's tasks into memory
for a staff caller. A staff request for a task they are not assigned to returns
404 rather than 403, so the endpoint does not confirm which ids exist.

## 8. Design notes

**Two-factor challenges live in Postgres, not Redis.** A successful verification
marks the row `consumed_at` instead of deleting it, so a replayed code is
reported as `challenge_already_used` — distinguishable from an unknown id, which
a deleted Redis key could not be. Consumption is a single guarded `UPDATE … WHERE
consumed_at IS NULL RETURNING`, so two concurrent submissions of the same valid
code cannot both mint a token.

**Authorization is carried by an extractor.** `AdminUser` rejects staff during
extraction, so a handler taking it cannot be reached by a staff caller. The rule
is visible in the function signature rather than in a body an edit could drop.

**Cache invalidation is centralised** in `Cache::invalidate_my_tasks`, called by
assign, update, and delete. Assignment passes both the previous and the new
assignee, so a reassignment does not leave the task visible in the former
assignee's cached list. Invalidation failures are logged but do not fail a write
that already committed — the cache is a read optimisation with a bounded TTL.

**`citext` needs an explicit cast on binds.** The type makes the unique index
case-insensitive, but sqlx binds a Rust `&str` as `text`, and `citext = text`
resolves to the case-sensitive `texteq`. Every lookup by email binds
`$1::citext`; without it, logging in as `Admin@example.com` would fail while the
account exists. `tests/auth.rs::login_is_case_insensitive_on_email` guards this.

**`jsonwebtoken` must enable `rust_crypto`.** Its default features contain no
crypto provider, and the omission surfaces as a runtime panic rather than a
compile error.

## 9. Tests

```bash
cp .env.example .env
psql -c "CREATE DATABASE task_management_test;"
DATABASE_URL=postgres://test:test@127.0.0.1:5432/task_management_test cargo test
```

Integration tests run against a real Postgres and a real Redis — nothing is
mocked. Each test starts the actual router on an ephemeral port, truncates the
tables, and flushes the cache. They share one database, so `.cargo/config.toml`
pins `RUST_TEST_THREADS=1`; `cargo test` is therefore correct on its own.

| Suite | Covers |
|---|---|
| `tests/auth.rs` | Login yields no token; wrong / expired / reused / capped codes; logout revocation; tampered tokens; case-insensitive email |
| `tests/permissions.rs` | The full RBAC matrix; 404-not-403 scoping; atomic assignment; enum validation |
| `tests/cache.rs` | Miss then hit with identical payloads; per-user isolation; invalidation on assign, reassign, update, delete |
| `tests/validation_flow.rs` | The 15-step acceptance flow end to end |

## 10. Documentation

- Design spec: `docs/superpowers/specs/2026-09-16-task-management-api-design.md`
