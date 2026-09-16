# Task Management API — Design

Date: 2026-09-16
Status: Approved for planning

## 1. Purpose

A Rust backend API for a small task-management workflow. It exists to demonstrate,
end to end and against real infrastructure:

- password authentication,
- two-factor login via an emailed one-time code,
- role-based permissions with two roles,
- admin-driven task assignment,
- a read cache with correct invalidation,
- a module structure where HTTP, business rules, SQL, and Redis stay separated.

Success is defined by the validation flow in section 10. If that flow passes against
a live Postgres and Redis, the system is correct.

## 2. Stack and environment

| Concern | Choice |
|---|---|
| Language | Rust 1.96, edition 2024 |
| HTTP | axum 0.8 + tower, tokio runtime |
| Database | PostgreSQL, reachable locally on port 5433 |
| DB driver | sqlx (async, compile-time-checked queries, `uuid` + `chrono` features) |
| Cache | Redis, via the `redis` crate with an async connection manager |
| Passwords | Argon2id (`argon2` crate) |
| Tokens | HS256 JWT (`jsonwebtoken`) |
| Errors | `thiserror` for typed errors, one `IntoResponse` mapping |
| Logging | `tracing` + `tracing-subscriber` |

Configuration comes from environment variables, parsed once at startup into a typed
`Config`. Startup fails loudly if any required variable is missing or unparseable —
there are no silent defaults for security-relevant values.

| Variable | Meaning |
|---|---|
| `DATABASE_URL` | Postgres connection string |
| `REDIS_URL` | Redis connection string |
| `JWT_SECRET` | HS256 signing key; startup fails if shorter than 32 bytes |
| `JWT_TTL_SECONDS` | Access-token lifetime, default 900 |
| `TWOFA_TTL_SECONDS` | Challenge lifetime, default 300 |
| `TWOFA_MAX_ATTEMPTS` | Failed verifications before the challenge burns, default 5 |
| `CACHE_TTL_SECONDS` | `view-my-tasks` cache TTL, default 60 |
| `APP_ENV` | `dev` mounts the dev-only email-log route; anything else does not |
| `BIND_ADDR` | Listen address, default `127.0.0.1:3000` |
| `SEED_ADMIN_PASSWORD` | Password for the seeded Admin account, default `admin123` |
| `SEED_BOND_PASSWORD` | Password for the seeded James Bond account, default `bond007` |

## 3. Data model

All primary keys are `uuid` (v4). All timestamps are `timestamptz`. `updated_at` is
maintained by a shared trigger so no handler can forget it.

Three Postgres enums are mapped to Rust enums, making an out-of-range value
unrepresentable in either layer:

- `user_role`: `admin`, `staff`
- `task_status`: `todo`, `in_progress`, `done`
- `task_priority`: `high`, `medium`, `low`

```sql
users
  id               uuid        primary key
  full_name        text        not null
  email            citext      not null unique
  hashed_password  text        not null
  role             user_role   not null
  created_at       timestamptz not null default now()
  updated_at       timestamptz not null default now()

tasks
  id               uuid          primary key
  title            text          not null
  description      text          not null default ''
  status           task_status   not null default 'todo'
  priority         task_priority not null default 'medium'
  created_by_id    uuid          not null references users(id)
  assigned_to_id   uuid          null     references users(id)
  created_at       timestamptz   not null default now()
  updated_at       timestamptz   not null default now()
  index on (assigned_to_id)

two_factor_challenges
  id               uuid        primary key
  user_id          uuid        not null references users(id) on delete cascade
  code_hash        text        not null
  attempts         int         not null default 0
  expires_at       timestamptz not null
  consumed_at      timestamptz null
  created_at       timestamptz not null default now()
  index on (user_id)

email_logs
  id               uuid        primary key
  to_email         text        not null
  subject          text        not null
  body             text        not null
  code             text        null
  created_at       timestamptz not null default now()
  index on (created_at desc)
```

`citext` gives case-insensitive email uniqueness at the database level, so
`Bond@example.com` cannot become a second account.

The one-time code is stored only as an Argon2 hash. A database dump therefore does
not hand over live second factors. `email_logs.code` holds the plaintext code, which
is acceptable and intentional: that table *is* the development mail transport, and it
is what `GET /dev/email-logs/latest` reads.

## 4. Authentication

### 4.1 Login — step one

`POST /auth/login` with `{email, password}`:

1. Look up the user by email. Verify the password with Argon2id.
2. On failure return `401 invalid credentials`. The same error and the same
   approximate latency are returned whether the email is unknown or the password is
   wrong — when no user exists the handler still performs a dummy Argon2 verification,
   so response timing does not disclose which emails are registered.
3. On success generate a cryptographically random 6-digit code using the OS RNG
   (`rand::rngs::OsRng`), never a time- or PID-seeded generator.
4. Insert a `two_factor_challenges` row holding the Argon2 hash of the code and
   `expires_at = now() + TWOFA_TTL_SECONDS`.
5. Write an `email_logs` row through the notifier and emit a `tracing` event.
6. Return `200 {challenge_id, expires_in}`.

No JWT and no code appear in this response. That is the central property of the flow:
a correct password alone yields nothing usable.

### 4.2 Verify — step two

`POST /auth/verify-2fa` with `{challenge_id, code}`. The five outcomes are distinct:

| Condition | Response |
|---|---|
| Challenge id unknown | `401 invalid challenge` |
| `attempts >= TWOFA_MAX_ATTEMPTS` | `429 too many attempts` |
| `now() > expires_at` | `401 challenge expired` |
| `consumed_at is not null` | `401 challenge already used` |
| `code_hash` mismatch | `401 invalid code`, `attempts += 1` |
| otherwise | `200 {access_token, token_type: "Bearer", expires_in}` |

Reuse is detectable precisely because a successful verification marks the row
`consumed_at = now()` instead of deleting it. Had the challenge lived only in Redis
and been deleted on use, a replayed code would be indistinguishable from a bad id —
the API could not honestly report "already used", and no test could prove single-use
semantics rather than mere absence.

Consumption is one conditional statement:

```sql
update two_factor_challenges
   set consumed_at = now()
 where id = $1 and consumed_at is null
returning user_id
```

If it returns no row, another request consumed the challenge first. Two concurrent
submissions of the same valid code therefore cannot both mint a token.

Checks are ordered attempts → expiry → consumed → code, so a burned or expired
challenge is rejected before any work is spent hashing the submitted code.

### 4.3 Tokens

HS256 JWT with claims `sub` (user id), `role`, `jti` (uuid), `iat`, `exp`. TTL
defaults to 15 minutes.

`POST /auth/logout` writes `denylist:{jti}` to Redis with a TTL equal to the token's
remaining lifetime, so the entry expires exactly when the token would have anyway and
the denylist cannot grow without bound.

Authentication middleware, in order: parse the `Authorization: Bearer` header, verify
the signature and `exp`, check the denylist, then construct `AuthUser { id, role }`.

`role` is read from the claims rather than re-fetched per request. With only two fixed
roles and a 15-minute TTL this is an accepted, bounded staleness: a demotion takes
effect within one token lifetime, or immediately if the session is logged out.

### 4.4 Authorization

Two extractors carry the rules in the type system:

- `AuthUser` — any authenticated caller.
- `AdminUser` — wraps `AuthUser` and rejects `staff` with `403` during extraction.

A handler that takes `AdminUser` cannot be reached by a staff user, so an admin-only
endpoint cannot lose its check to an edited `if` in a handler body. The role rule is
visible in the function signature.

Permission matrix:

| Action | admin | staff |
|---|---|---|
| create task | yes | no (403) |
| list all tasks | yes | no (403) |
| view own assigned tasks | yes | yes, scoped to self |
| get task by id | yes | only if assignee, else 404 |
| assign / reassign | yes | no (403) |
| update status | yes | only if assignee |
| update title/description/priority | yes | no |
| delete task | yes | no |

Staff scoping is enforced in SQL (`where assigned_to_id = $1`), never by filtering a
wider result set in Rust. There is no code path that reads other users' tasks into
memory for a staff caller, so the restriction cannot be lost in a later refactor of
the handler.

A staff caller requesting a task they are not assigned to receives `404`, not `403`,
so the endpoint does not confirm which task ids exist.

## 5. Caching

`GET /tasks/view-my-tasks` is the only cached read.

- Key: `cache:tasks:my:{user_id}`
- Value: the response payload without the `cache` block — `user`, `tasks`, `summary`
- TTL: `CACHE_TTL_SECONDS`, default 60

Flow: on a miss, query Postgres, serialize, `SET` with TTL, respond `cache.hit = false`.
On a hit, deserialize and respond `cache.hit = true`.

`cache.hit` is computed by the handler from whether Redis returned a value. It is never
part of the stored payload, so it cannot be served stale as `true` from a cold cache.

The cached data always originates from the database. Nothing in this endpoint is
hardcoded; a cache hit replays a previous database result and nothing else.

### Invalidation

One function owns it:

```rust
TaskCache::invalidate_for(user_ids: impl IntoIterator<Item = Uuid>)
```

Called by assignment, update, and delete. Assignment passes **both** the previous and
the new `assigned_to_id`. Dropping the previous holder is the classic reassignment bug:
the task moves in the database while the former assignee's cached list keeps showing it
until the TTL lapses. Both keys are deleted, so a reassignment is immediately correct
from both sides.

Invalidation failures are logged and do not fail the write. The cache is a read
optimisation with a bounded TTL; a write that already committed must not report failure
because Redis was briefly unreachable. Worst case is up to `CACHE_TTL_SECONDS` of
staleness on one key.

Redis is therefore not load-bearing for the correctness of authentication. Its jobs are
this cache and the logout denylist; Postgres is the source of truth for everything else.

## 6. API surface

| Endpoint | Auth | Purpose |
|---|---|---|
| `POST /seed/users` | none | Idempotently create the Admin and James Bond accounts |
| `POST /auth/login` | none | Verify password, create challenge, send code |
| `GET /dev/email-logs/latest` | none, dev only | Newest email log row, including the code |
| `POST /auth/verify-2fa` | none | Verify code, return JWT |
| `POST /auth/logout` | bearer | Deny-list the presented `jti` |
| `POST /tasks` | admin | Create a task |
| `GET /tasks` | admin | List all tasks |
| `POST /tasks/assign` | admin | Batch-assign tasks to one user |
| `PATCH /tasks/{id}` | admin, or assignee for status | Update a task |
| `DELETE /tasks/{id}` | admin | Delete a task |
| `GET /tasks/view-my-tasks` | bearer | Caller's tasks plus cache metadata |

### 6.1 Seeding

`POST /seed/users` creates exactly two fixed accounts and returns their ids. It is
idempotent: run twice, the second call returns the same ids and changes nothing.

It takes no role parameter and can create no account other than these two, so it is not
a privilege-escalation route despite being unauthenticated. Passwords come from
`SEED_ADMIN_PASSWORD` and `SEED_BOND_PASSWORD`, defaulting to known development values.

In a real deployment this route would be `APP_ENV`-gated like the email-log route, and
the admin would be seeded by migration. It is mounted unconditionally here because the
validation flow in section 10 calls it.

| Account | Email | Role |
|---|---|---|
| Admin | `admin@example.com` | `admin` |
| James Bond | `jamesbond@example.com` | `staff` |

### 6.2 Assignment

`POST /tasks/assign` with `{task_ids: [uuid, ...], assigned_to_id: uuid}`.

The whole batch applies in a single transaction. If any task id is unknown the request
fails with `404` and nothing is assigned — a partially applied batch would leave the
caller unable to tell which half succeeded. The target user must exist, and is looked up
in the same transaction.

Cache invalidation runs after commit, for the new assignee and for every distinct
previous assignee in the batch.

### 6.3 view-my-tasks response

```json
{
  "user":    { "email": "jamesbond@example.com", "role": "staff" },
  "tasks": [
    { "id": "...", "title": "...", "status": "todo",
      "priority": "high", "assigned_to": "jamesbond@example.com" }
  ],
  "summary": { "total_assigned_tasks": 3 },
  "cache":   { "hit": false }
}
```

The task projection carries exactly five fields: `id`, `title`, `status`, `priority`,
`assigned_to`. `assigned_to` is the assignee's **email**, obtained by joining `users` on
`assigned_to_id` in SQL rather than by issuing one query per task.

`description`, `created_by_id`, and timestamps are deliberately absent from this
projection; they remain available on the admin-facing `GET /tasks`.

Tasks are ordered by `created_at asc, id asc`. The id tiebreak keeps the order total and
therefore reproducible when several rows share a timestamp, which matters because the
cached payload must equal the fresh one byte for byte.

## 7. Errors

One `AppError` enum with a single `IntoResponse` implementation is the only place status
codes are chosen, so the same condition cannot map to different codes in different
handlers.

| Variant | Status |
|---|---|
| `Validation` | 400 |
| `InvalidCredentials`, `TwoFactor(..)`, `Unauthenticated` | 401 |
| `Forbidden` | 403 |
| `NotFound` | 404 |
| `Conflict` | 409 |
| `TooManyAttempts` | 429 |
| `Database`, `Cache`, `Internal` | 500 |

Body: `{"error": {"code": "invalid_code", "message": "..."}}`. The `code` is a stable
machine-readable string; the `message` is for humans.

Internal errors log the underlying cause at `error` level and return a generic message.
SQL text, connection strings, and Redis errors never reach a client.

## 8. Module structure

```
src/
  main.rs            bootstrap and wiring only
  config.rs          environment -> typed Config
  error.rs           AppError and its single IntoResponse
  state.rs           AppState { pg: PgPool, redis: ConnectionManager, config }
  routes.rs          router assembly, dev-only route mounting
  domain/            User, Role, Task, TaskStatus, TaskPriority, TwoFactorChallenge
  api/
    auth.rs          login, verify-2fa, logout
    users.rs         seeding
    tasks.rs         create, list, assign, update, delete, view-my-tasks
    dev.rs           email-logs/latest
  auth/
    password.rs      Argon2 hash and verify
    jwt.rs           encode, decode, claims
    twofa.rs         code generation, challenge lifecycle
    extractors.rs    AuthUser, AdminUser
  repo/
    users.rs         all user SQL
    tasks.rs         all task SQL
    challenges.rs    all challenge SQL
    email_logs.rs    all email-log SQL
  cache/
    client.rs        typed get/set/delete
    keys.rs          key construction, one place per key shape
  notify/
    mod.rs           EmailNotifier trait
    log_notifier.rs  writes an email_logs row and traces
migrations/
tests/
```

The dependency rule is one-directional: `api` depends on `repo`, `cache`, `auth`, and
`domain`; those depend only on `domain`. Handlers contain no SQL and no Redis calls;
repositories know nothing about HTTP status codes.

`domain` holds pure types with no I/O, so the business rules are testable without a
database. `EmailNotifier` is a trait so tests can substitute a capturing fake, and a
real SMTP transport could be added later without touching the auth handlers.

Every cache key is built by a function in `cache/keys.rs`. Key strings are never
formatted inline, which is what makes "invalidate the key the reader reads" verifiable
by inspection rather than by grep.

## 9. Testing

Test-driven: each behaviour gets a failing test before its implementation.

**Unit** — password hash/verify round-trip and rejection; JWT encode/decode, expiry,
tampering; 6-digit code generation range and distribution; the permission matrix as a
table test over (role, action, ownership); cache key construction.

**Integration** — a live Postgres test database and a live Redis, both created and
migrated once per run. Each test runs in a transaction that is rolled back, or truncates
tables, so tests do not observe each other's data. Redis keys are namespaced per test run
and flushed on teardown. Tests drive the real router over HTTP.

Coverage includes: login returns a challenge and never a token; wrong code rejected and
`attempts` incremented; expired challenge rejected; consumed challenge rejected on replay;
attempt cap returns 429; correct code returns a usable JWT; deny-listed token rejected
after logout; staff blocked from create, list-all, and assign; staff reading another's
task gets 404; assignment is atomic on a bad id; cache miss then hit; invalidation on
assign, reassign, update, and delete; reassignment clears the previous assignee's key.

A cache test asserts more than the `hit` flag: it compares the miss and hit payloads for
equality, which is what proves the cache returns the same data rather than merely
returning something.

## 10. Validation flow

One end-to-end integration test, asserting in order:

1. `POST /seed/users` creates Admin and James Bond.
2. `POST /auth/login` as Admin returns a `challenge_id` and **no token**.
3. `GET /dev/email-logs/latest` yields the code.
4. `POST /auth/verify-2fa` with a wrong code is rejected.
5. The same challenge with the correct code succeeds and returns a JWT.
6. Replaying that correct code is rejected as already used.
7. A separate challenge, expired past its TTL, is rejected.
8. Admin creates exactly **5** tasks.
9. Admin assigns exactly **3** of them to James Bond, with priorities high, medium, low.
10. James Bond logs in through the same two-step flow.
11. Bond's `POST /tasks` is rejected with `403`.
12. Bond's `GET /tasks/view-my-tasks` returns exactly 3 tasks, every `assigned_to` equal
    to `jamesbond@example.com`, `summary.total_assigned_tasks == 3`, `cache.hit == false`.
13. The identical request returns `cache.hit == true` and a byte-identical `tasks` array.
14. Admin reassigns one task away from Bond; Bond's next call shows `cache.hit == false`
    and exactly 2 tasks.

15. Bond re-reads to warm the cache (`cache.hit == true`). Admin then `PATCH`es the
    title of one task still assigned to Bond. Bond's next call shows `cache.hit == false`
    and the new title.

Step 14 is the one that proves the cache is wired to the data rather than merely warm: a
stale cache would still report 3. Step 15 covers the other half of the rule — an update
that changes no assignment must still invalidate, because the cached payload embeds the
task's fields, not just its identity.

### Traceability

Every stated testing expectation maps to a step above, and each is asserted explicitly
rather than inferred from a neighbouring assertion.

| Expectation | Step |
|---|---|
| Admin and James Bond can be created | 1 |
| Login creates a 2FA challenge and does not immediately return a JWT | 2 |
| Correct 2FA code returns a JWT | 5, 10 |
| Incorrect 2FA code rejected | 4 |
| Expired 2FA code rejected | 7 |
| Reused 2FA code rejected | 6 |
| Admin can create 5 tasks | 8 |
| Admin can assign exactly 3 tasks to James Bond | 9 |
| James Bond cannot create a task | 11 |
| James Bond can view exactly 3 assigned tasks | 12 |
| view-my-tasks twice shows cache.hit false, then true | 12, 13 |
| Task assignment invalidates the affected cache | 14 |
| Task update invalidates the affected cache | 15 |

The counts in steps 8, 9, and 12 are asserted as equalities (`== 5`, `== 3`, `== 3`), not
as lower bounds. "At least 3" would pass if authorization leaked a fourth task into Bond's
view, which is precisely the failure the expectation exists to catch.

Step 7 needs an expired challenge without a five-minute wait. The test inserts the
challenge, then backdates `expires_at` with a direct SQL update. Expiry is compared
against `now()` in the database rather than against a Rust clock, so this needs no
injectable time source in production code.

## 11. Out of scope

Refresh tokens, password reset, real SMTP, pagination, task comments or attachments,
audit logging, rate limiting beyond the 2FA attempt cap, multi-tenancy, and any role
beyond `admin` and `staff`.
