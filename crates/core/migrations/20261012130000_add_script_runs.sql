CREATE TABLE task_host_inputs (
  task_id INTEGER NOT NULL,
  host_id INTEGER NOT NULL,
  name    TEXT    NOT NULL,
  value   TEXT    NOT NULL,
  PRIMARY KEY (task_id, host_id, name),
  FOREIGN KEY (task_id, host_id) REFERENCES task_hosts(task_id, host_id) ON DELETE CASCADE
) STRICT;

CREATE TABLE script_runs (
  task_id     INTEGER NOT NULL,
  host_id     INTEGER NOT NULL,
  position    INTEGER NOT NULL,
  script_name TEXT    NOT NULL,
  output      TEXT    NOT NULL,
  exit_code   INTEGER,
  started_at  TEXT    NOT NULL,
  finished_at TEXT,
  PRIMARY KEY (task_id, host_id, position),
  FOREIGN KEY (task_id, host_id) REFERENCES task_hosts(task_id, host_id) ON DELETE CASCADE
) STRICT;
