-- Move task status onto the membership (per-host) and soft-delete images.
--
-- Status now lives on task_hosts (one state machine per (task_id, host_id));
-- the task-level status/timestamps/error are gone. Images are soft-deleted so
-- tasks.image_id is never nulled: NULL means "no image", a resolvable row with
-- deleted_at set means "deleted".

ALTER TABLE images ADD COLUMN deleted_at TEXT; -- NULL = live

DROP VIEW tasks_with_hosts;
DROP TABLE task_hosts;
DROP TABLE tasks;

CREATE TABLE tasks (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  type       TEXT    NOT NULL,    -- 'deploy' | 'capture' | 'multicast' | 'reboot'
  image_id   INTEGER REFERENCES images(id), -- images are soft-deleted; never nulled
  created_at TEXT    NOT NULL
) STRICT;

CREATE TABLE task_hosts (
  task_id     INTEGER REFERENCES tasks(id) ON DELETE CASCADE,
  host_id     INTEGER REFERENCES hosts(id) ON DELETE CASCADE,
  state       TEXT    NOT NULL,   -- 'pending' | 'running' | 'done' | 'failed' | 'cancelled'
  error       TEXT,
  started_at  TEXT,
  finished_at TEXT,
  PRIMARY KEY (task_id, host_id)
) STRICT;

CREATE INDEX idx_task_hosts_host_id ON task_hosts(host_id);
CREATE INDEX idx_task_hosts_state   ON task_hosts(state);

CREATE VIEW tasks_with_hosts AS
SELECT
    t.id,
    t.type,
    t.image_id,
    t.created_at,
    i.name                       AS image_name,
    (i.deleted_at IS NOT NULL)   AS image_deleted,
    COALESCE(
        (
            SELECT json_group_array(json_object(
                'host_id',     th.host_id,
                'state',       th.state,
                'error',       th.error,
                'started_at',  strftime('%Y-%m-%dT%H:%M:%fZ', th.started_at),
                'finished_at', strftime('%Y-%m-%dT%H:%M:%fZ', th.finished_at)
            ))
            FROM task_hosts th
            WHERE th.task_id = t.id
        ),
        json('[]')
    ) AS hosts
FROM tasks t
LEFT JOIN images i ON i.id = t.image_id;
