CREATE TABLE hosts (
  id              INTEGER PRIMARY KEY,
  mac             TEXT    NOT NULL UNIQUE,
  name            TEXT    NOT NULL,
  disk_size_bytes INTEGER NOT NULL
) STRICT;

CREATE TABLE images (
  id                     INTEGER PRIMARY KEY,
  name                   TEXT    NOT NULL,
  captured_from_host_id  INTEGER REFERENCES hosts(id) ON DELETE SET NULL,
  captured_at            INTEGER NOT NULL,
  status                 TEXT    NOT NULL
) STRICT;

CREATE TABLE image_partitions (
  id                INTEGER PRIMARY KEY,
  image_id          INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
  partition_number  INTEGER NOT NULL,
  fstype            TEXT    NOT NULL,
  size_bytes        INTEGER NOT NULL,
  file_path         TEXT    NOT NULL,
  sha256            TEXT,
  UNIQUE (image_id, partition_number)
) STRICT;

CREATE TABLE tasks (
  id           INTEGER PRIMARY KEY,
  type         TEXT    NOT NULL,
  host_id      INTEGER NOT NULL REFERENCES hosts(id),
  image_id     INTEGER REFERENCES images(id),
  state        TEXT    NOT NULL,
  created_at   INTEGER NOT NULL,
  started_at   INTEGER,
  finished_at  INTEGER,
  error        TEXT
) STRICT;

CREATE INDEX idx_tasks_host_state ON tasks(host_id, state);
CREATE INDEX idx_image_partitions ON image_partitions(image_id);
