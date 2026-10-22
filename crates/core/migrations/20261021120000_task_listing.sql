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
                'finished_at', strftime('%Y-%m-%dT%H:%M:%fZ', th.finished_at)
            ))
            FROM task_hosts th
            JOIN hosts h ON h.id = th.host_id
            WHERE th.task_id = t.id
        ),
        json('[]')
    ) AS hosts
FROM tasks t
LEFT JOIN images i ON i.id = t.image_id;
