-- Add migration script here
CREATE TABLE tasks (
  id          INTEGER PRIMARY KEY,
  type        TEXT    NOT NULL,    -- 'deploy' | 'capture'
  host_id   INTEGER REFERENCES hosts(id) ON DELETE SET NULL,
  image_id  INTEGER REFERENCES images(id) ON DELETE SET NULL,
  state       TEXT    NOT NULL,    -- 'pending' | 'running' | 'done' | 'failed' | 'cancelled'
  created_at  TEXT    NOT NULL,
  started_at  TEXT,
  finished_at TEXT,
  error       TEXT
) STRICT;

CREATE INDEX idx_tasks_host_state ON tasks(host_id, state);
CREATE INDEX idx_tasks_state      ON tasks(state);

