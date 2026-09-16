# AI Usage

This project was built in a single session with **Claude Opus 5, via Claude Code**
(CLI). This document records what the AI did, what a human decided, and how the
output was verified — so a reviewer can judge the work knowing how it was made.

## Summary

Effectively all of the source, tests, and documentation in this repository were
written by the AI. Every requirement, and every decision that shaped the design,
came from the human. Nothing was accepted because it looked plausible: the crate
APIs were probed against the real libraries, and the behaviour is verified by 34
integration tests that run against a live PostgreSQL and Redis.

## Workflow

The session followed a design-then-build sequence rather than prompting for code
directly.

1. **Requirements dialogue.** The AI classified the task as architectural (a new
   project with no existing code to change) and asked clarifying questions one at
   a time — storage, second-factor mechanism, session model, role model, staff
   permissions — presenting concrete options with trade-offs at each step.
2. **Design spec.** Once the requirements were settled, the AI wrote
   `docs/superpowers/specs/2026-09-16-task-management-api-design.md` and the
   human approved it before any code was written.
3. **Implementation.** The human asked to skip a formal implementation plan for
   time, so the AI implemented directly from the approved spec.
4. **Verification.** The AI ran the full suite against live services and reported
   the actual counts.

## Who decided what

Every functional requirement in this repository originated with the human. The
AI proposed options and wrote the code; it did not choose the product.

**Human decisions:**

- Postgres + Redis over in-memory or hybrid alternatives
- Emailed one-time code as the second factor, rather than TOTP
- JWT access tokens with a Redis denylist, rather than opaque sessions
- Exactly two roles, `admin` and `staff`
- The complete API shape: `POST /seed/users`, `POST /auth/login`,
  `GET /dev/email-logs/latest`, `POST /auth/verify-2fa`, `POST /tasks`,
  `POST /tasks/assign`, `GET /tasks/view-my-tasks`
- The exact `view-my-tasks` response envelope, including `priority`,
  `assigned_to` as an email, and the `summary` / `cache` blocks
- The minimum data model and its field names — `full_name`,
  `hashed_password`, `created_by_id`, `assigned_to_id`, and the
  `LoginChallenge` / `EmailLog` entities
- All ten business rules and testing expectations
- The git identity used for commits

**AI decisions, made from those requirements:**

- Module layout and the one-directional dependency rule between layers
- Persisting two-factor challenges in Postgres rather than Redis, so that a
  replayed code is distinguishable from an unknown one
- Expressing the role check as an `AdminUser` extractor so admin-only routes
  enforce it in the handler signature
- Returning 404 rather than 403 when staff read a task they are not assigned to
- Centralising cache invalidation, and invalidating both the previous and the new
  assignee on reassignment
- Error taxonomy and status-code mapping
- Test structure and the specific cases covered

The human reviewed and approved the design before implementation, and raised no
objection to these during review.

## Verification, not trust

The AI's training data predates the versions of several crates used here, so the
API surfaces were checked rather than recalled. Before writing any project code,
a throwaway probe crate was built in a scratch directory that exercised axum,
sqlx, redis, jsonwebtoken, and argon2 together, and ran against the actual
PostgreSQL and Redis on this machine.

That probe caught two defects that would otherwise have shipped, both of which
fail **silently**:

1. **`citext` is not case-insensitive on a bound parameter.** sqlx binds a Rust
   `&str` as a `text` parameter, and `citext = text` resolves to the
   case-sensitive `texteq`. An untyped SQL literal matches; a bound parameter
   returns zero rows. Logging in as `Admin@example.com` would have failed with
   "invalid credentials" while the account existed. Every email lookup now binds
   `$1::citext`, and `tests/auth.rs::login_is_case_insensitive_on_email` guards
   against a regression.
2. **`jsonwebtoken` 11 ships no crypto provider by default.** Its default
   features are `["use_pem"]` alone. The crate compiles cleanly and then panics
   at runtime on the first token issued. The `rust_crypto` feature is now
   explicit.

Both were found by running code, not by reading it.

## Mistakes the AI made

Recorded because they are part of an honest account of the process.

- **A wrong SQL construction.** `assign_batch` was first written to read the
  previous assignee via a subquery inside `RETURNING`. That is unreliable before
  PostgreSQL 18, which this project does not target. Caught on review before it
  compiled, and rewritten as a `SELECT … FOR UPDATE` followed by a set-based
  update inside one transaction.
- **Two stale crate APIs.** The first draft used the argon2 0.5 two-argument
  `hash_password(password, &salt)` form, and a `redis::ErrorKind` variant that no
  longer exists. Both were compile errors, found and fixed.
- **Two errors in the design spec**, found by the AI's own review pass before the
  human read it: the response example used `"summary": { "cache": 3 }` instead of
  `total_assigned_tasks`, and two environment variables were used in one section
  but missing from the configuration table.
- **An incorrect connection target.** The spec initially named the PostgreSQL on
  port 5433, which turned out to be unusable (peer authentication, no matching
  role). Corrected to the Docker instance on 5432 after actually attempting a
  connection.
- **An abandoned artefact.** An implementation plan was started and reached only
  its first task before the human redirected to implementation. It was deleted
  rather than left in the repository half-finished.

## What was verified, and what was not

**Verified by execution:**

- All 34 integration tests pass against live PostgreSQL and Redis — 14 auth, 12
  permissions, 7 cache, 1 end-to-end validation flow. Nothing is mocked; each
  test starts the real router on an ephemeral port.
- The full fifteen-step validation flow, covering every stated testing
  expectation.
- `citext` case-insensitive uniqueness, confirmed by a rejected insert.
- Argon2 hashing, JWT round-trips including signature rejection, and Redis
  get/set/delete semantics including deletion of absent keys.

**Not verified:**

- No load, soak, or concurrency testing beyond what the correctness tests cover.
  The guarded `UPDATE` that makes challenge consumption single-use is argued from
  its SQL semantics and is not proven by a concurrent test.
- No security audit or penetration testing.
- Not run against any PostgreSQL other than 16, or any Redis other than 8.8.
- The timing-attack mitigation on unknown emails performs a dummy Argon2
  verification, but the equality of the two paths' timing was not measured.

## Reproducing this assessment

```bash
cargo test    # 34 tests, requires PostgreSQL and Redis per README section 1
```

The design spec records the reasoning behind each decision, including the
alternatives rejected and why.
