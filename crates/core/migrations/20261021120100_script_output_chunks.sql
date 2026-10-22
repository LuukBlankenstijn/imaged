CREATE TABLE script_run_output (
  task_id  INTEGER NOT NULL,
  host_id  INTEGER NOT NULL,
  position INTEGER NOT NULL,
  start    INTEGER NOT NULL,
  text     TEXT    NOT NULL,
  PRIMARY KEY (task_id, host_id, position, start),
  FOREIGN KEY (task_id, host_id, position)
    REFERENCES script_runs(task_id, host_id, position) ON DELETE CASCADE
) STRICT;

ALTER TABLE script_runs ADD COLUMN output_bytes INTEGER NOT NULL DEFAULT 0;

INSERT INTO script_run_output (task_id, host_id, position, start, text)
SELECT task_id, host_id, position, 0, output FROM script_runs WHERE output <> '';

UPDATE script_runs SET output_bytes = length(CAST(output AS BLOB));

ALTER TABLE script_runs DROP COLUMN output;

CREATE TRIGGER close_script_runs_when_host_leaves_running
AFTER UPDATE OF state ON task_hosts
WHEN NEW.state NOT IN ('pending', 'running')
BEGIN
  UPDATE script_runs SET finished_at = NEW.finished_at
  WHERE task_id = NEW.task_id AND host_id = NEW.host_id AND finished_at IS NULL;
END;
