-- Add migration script here
CREATE TABLE tasks (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  type        TEXT    NOT NULL,    -- 'deploy' | 'capture' | 'multicast'
  image_id    INTEGER REFERENCES images(id) ON DELETE SET NULL,
  state       TEXT    NOT NULL,    -- 'pending' | 'running' | 'done' | 'failed' | 'cancelled'
  created_at  TEXT    NOT NULL,
  started_at  TEXT,
  finished_at TEXT,
  error       TEXT
) STRICT;

CREATE INDEX idx_tasks_state      ON tasks(state);

CREATE TABLE task_hosts (
  task_id INTEGER REFERENCES tasks(id) ON DELETE CASCADE,
  host_id INTEGER REFERENCES hosts(id) ON DELETE CASCADE,
  PRIMARY KEY (task_id, host_id)
) STRICT;

CREATE INDEX idx_task_hosts_host_id ON task_hosts(host_id);

CREATE TABLE groups (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT    NOT NULL
) STRICT;

CREATE TABLE group_hosts (
  group_id INTEGER REFERENCES groups(id) ON DELETE CASCADE,
  host_id INTEGER REFERENCES hosts(id) ON DELETE CASCADE,
  PRIMARY KEY (group_id, host_id)
) STRICT;

CREATE VIEW tasks_with_hosts AS
SELECT
    t.id,
    t.type,
    t.image_id,
    t.state,
    t.created_at,
    t.started_at,
    t.finished_at,
    t.error,
    COALESCE(
        (
            SELECT json_group_array(th.host_id)
            FROM task_hosts th
            WHERE th.task_id = t.id
        ),
        json('[]')
    ) AS host_ids
FROM tasks t;
