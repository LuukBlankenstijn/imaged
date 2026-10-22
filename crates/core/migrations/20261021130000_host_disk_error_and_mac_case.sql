ALTER TABLE hosts ADD COLUMN disk_error TEXT;

CREATE TEMP TABLE host_merge AS
WITH ranked AS (
    SELECT
        h.id,
        lower(replace(h.mac, '-', ':')) AS canonical,
        row_number() OVER (
            PARTITION BY lower(replace(h.mac, '-', ':'))
            ORDER BY (SELECT count(*) FROM task_hosts th WHERE th.host_id = h.id) DESC, h.id
        ) AS rank
    FROM hosts h
)
SELECT duplicate.id AS duplicate, survivor.id AS survivor
FROM ranked duplicate
JOIN ranked survivor ON survivor.canonical = duplicate.canonical AND survivor.rank = 1
WHERE duplicate.rank > 1;

CREATE TEMP TABLE moved_task_hosts AS
SELECT th.task_id, m.duplicate, m.survivor
FROM task_hosts th
JOIN host_merge m ON m.duplicate = th.host_id
WHERE NOT EXISTS (
    SELECT 1 FROM task_hosts kept WHERE kept.task_id = th.task_id AND kept.host_id = m.survivor
);

INSERT INTO task_hosts (task_id, host_id, state, error, started_at, finished_at)
SELECT th.task_id, mv.survivor, th.state, th.error, th.started_at, th.finished_at
FROM task_hosts th
JOIN moved_task_hosts mv ON mv.task_id = th.task_id AND mv.duplicate = th.host_id;

INSERT INTO task_host_inputs (task_id, host_id, name, value)
SELECT i.task_id, mv.survivor, i.name, i.value
FROM task_host_inputs i
JOIN moved_task_hosts mv ON mv.task_id = i.task_id AND mv.duplicate = i.host_id;

INSERT INTO script_runs (task_id, host_id, position, script_name, exit_code, started_at, finished_at, output_bytes)
SELECT r.task_id, mv.survivor, r.position, r.script_name, r.exit_code, r.started_at, r.finished_at, r.output_bytes
FROM script_runs r
JOIN moved_task_hosts mv ON mv.task_id = r.task_id AND mv.duplicate = r.host_id;

INSERT INTO script_run_output (task_id, host_id, position, start, text)
SELECT o.task_id, mv.survivor, o.position, o.start, o.text
FROM script_run_output o
JOIN moved_task_hosts mv ON mv.task_id = o.task_id AND mv.duplicate = o.host_id;

INSERT OR IGNORE INTO group_hosts (group_id, host_id)
SELECT g.group_id, m.survivor
FROM group_hosts g
JOIN host_merge m ON m.duplicate = g.host_id;

DELETE FROM hosts WHERE id IN (SELECT duplicate FROM host_merge);

DROP TABLE moved_task_hosts;
DROP TABLE host_merge;

UPDATE hosts SET name = lower(name) WHERE name = replace(mac, ':', '-');

UPDATE hosts SET mac = lower(replace(mac, '-', ':'));
