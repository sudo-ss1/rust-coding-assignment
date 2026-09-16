# Postman collection

`Task-Management-API.postman_collection.json` — 37 requests across 9 folders,
carrying 86 assertions. Import it into Postman, or run it headlessly with
newman.

## Running it

The collection needs the API up with `APP_ENV=dev`, because folder 2 reads the
one-time code from the dev-only mail log route.

```bash
# terminal 1
cargo run

# terminal 2 — headless
npx newman run postman/Task-Management-API.postman_collection.json
```

In the Postman UI: **Import** the file, then open the **Collection Runner** and
run the whole collection **in order**. Nothing needs to be pasted by hand —
every token, user id, and task id is captured into a collection variable by a
post-response script.

Override the base URL with `--env-var baseUrl=http://127.0.0.1:3000` (newman) or
by editing the `baseUrl` collection variable (UI).

## What it covers

| Folder | Covers |
|---|---|
| 0. Health | Server is up |
| 1. Setup | Seeds admin and James Bond; captures their ids |
| 2. Admin login | Challenge without a token, wrong code, correct code, replay |
| 3. Create 5 tasks | Five creates at high/medium/low, then a count check |
| 4. Assign | Exactly 3 tasks to Bond |
| 5. Bond login | The same two-step flow as the admin |
| 6. Validation | Bond blocked from creating; miss then hit; reassign and edit both evict |
| 7. Permissions | The negative RBAC cases, 404-not-403 scoping, input validation |
| 8. Logout | Revocation, and reuse of the revoked token |

Folder 8 runs last because it revokes the token the earlier folders depend on.

## Notes

Folders 3 onward depend on state created by earlier folders, so run the
collection top to bottom. Seeding is idempotent, but tasks accumulate across
runs — the "exactly 5 tasks exist" assertion in folder 3 expects a fresh
database. To reset:

```bash
psql "$DATABASE_URL" -c 'drop schema public cascade; create schema public;'
redis-cli FLUSHDB
```

Migrations reapply on the next `cargo run`.

## Verified

Last run against a live server: **37/37 requests, 86/86 assertions, 0 failures.**
