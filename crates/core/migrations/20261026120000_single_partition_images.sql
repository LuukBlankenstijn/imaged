CREATE TABLE host_partitions (
  host_id    INTEGER NOT NULL REFERENCES hosts(id) ON DELETE CASCADE,
  number     INTEGER NOT NULL,
  partuuid   TEXT    NOT NULL,
  partlabel  TEXT,
  label      TEXT,
  uuid       TEXT,
  fstype     TEXT,
  size_bytes INTEGER NOT NULL,
  UNIQUE (host_id, number)
) STRICT;

ALTER TABLE images ADD COLUMN kind TEXT NOT NULL DEFAULT 'disk' CHECK (kind IN ('disk', 'partition'));

ALTER TABLE image_partitions ADD COLUMN fs_uuid TEXT;

ALTER TABLE image_scripts ADD COLUMN root_partition INTEGER;

ALTER TABLE task_hosts ADD COLUMN partuuid TEXT;

DROP VIEW tasks_with_hosts;

CREATE VIEW tasks_with_hosts AS
SELECT
    t.id,
    t.type,
    t.image_id,
    t.created_at,
    i.name                       AS image_name,
    (i.deleted_at IS NOT NULL)   AS image_deleted,
    CASE
        WHEN NOT EXISTS (SELECT 1 FROM task_hosts th WHERE th.task_id = t.id)
            THEN 'cancelled'
        WHEN EXISTS (
            SELECT 1 FROM task_hosts th
            WHERE th.task_id = t.id AND th.state IN ('pending', 'running')
        )
            THEN CASE
                WHEN EXISTS (
                    SELECT 1 FROM task_hosts th
                    WHERE th.task_id = t.id AND th.state <> 'pending'
                ) THEN 'running'
                ELSE 'pending'
            END
        WHEN NOT EXISTS (
            SELECT 1 FROM task_hosts th WHERE th.task_id = t.id AND th.state <> 'done'
        )
            THEN 'done'
        WHEN NOT EXISTS (
            SELECT 1 FROM task_hosts th WHERE th.task_id = t.id AND th.state <> 'cancelled'
        )
            THEN 'cancelled'
        WHEN NOT EXISTS (
            SELECT 1 FROM task_hosts th WHERE th.task_id = t.id AND th.state = 'done'
        )
            THEN 'failed'
        ELSE 'partial'
    END AS state,
    COALESCE(
        (
            SELECT json_group_array(json_object(
                'host_id',     th.host_id,
                'host_name',   h.name,
                'mac',         h.mac,
                'state',       th.state,
                'error',       th.error,
                'started_at',  strftime('%Y-%m-%dT%H:%M:%fZ', th.started_at),
                'finished_at', strftime('%Y-%m-%dT%H:%M:%fZ', th.finished_at),
                'partuuid',    th.partuuid
            ))
            FROM task_hosts th
            JOIN hosts h ON h.id = th.host_id
            WHERE th.task_id = t.id
        ),
        json('[]')
    ) AS hosts
FROM tasks t
LEFT JOIN images i ON i.id = t.image_id;
