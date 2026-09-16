CREATE EXTENSION IF NOT EXISTS citext;

CREATE TYPE user_role     AS ENUM ('admin', 'staff');
CREATE TYPE task_status   AS ENUM ('todo', 'in_progress', 'done');
CREATE TYPE task_priority AS ENUM ('high', 'medium', 'low');

CREATE TABLE users (
    id              uuid        PRIMARY KEY,
    full_name       text        NOT NULL,
    email           citext      NOT NULL UNIQUE,
    hashed_password text        NOT NULL,
    role            user_role   NOT NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE tasks (
    id             uuid          PRIMARY KEY,
    title          text          NOT NULL,
    description    text          NOT NULL DEFAULT '',
    status         task_status   NOT NULL DEFAULT 'todo',
    priority       task_priority NOT NULL DEFAULT 'medium',
    created_by_id  uuid          NOT NULL REFERENCES users(id),
    assigned_to_id uuid          NULL     REFERENCES users(id),
    created_at     timestamptz   NOT NULL DEFAULT now(),
    updated_at     timestamptz   NOT NULL DEFAULT now()
);
CREATE INDEX tasks_assigned_to_id_idx ON tasks (assigned_to_id);

CREATE TABLE two_factor_challenges (
    id          uuid        PRIMARY KEY,
    user_id     uuid        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    code_hash   text        NOT NULL,
    attempts    int         NOT NULL DEFAULT 0,
    expires_at  timestamptz NOT NULL,
    consumed_at timestamptz NULL,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX two_factor_challenges_user_id_idx ON two_factor_challenges (user_id);

CREATE TABLE email_logs (
    id         uuid        PRIMARY KEY,
    to_email   text        NOT NULL,
    subject    text        NOT NULL,
    body       text        NOT NULL,
    code       text        NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX email_logs_created_at_idx ON email_logs (created_at DESC);

CREATE OR REPLACE FUNCTION set_updated_at() RETURNS trigger AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER users_set_updated_at BEFORE UPDATE ON users
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();
CREATE TRIGGER tasks_set_updated_at BEFORE UPDATE ON tasks
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();
