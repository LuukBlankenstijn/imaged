-- Add migration script here
--
CREATE TABLE images (
  id                     INTEGER PRIMARY KEY AUTOINCREMENT,
  name                   TEXT    NOT NULL,
  captured_at            TEXT,
  status                 TEXT    NOT NULL,
  error                  TEXT
) STRICT;

CREATE TABLE image_partitions (
  id                INTEGER PRIMARY KEY AUTOINCREMENT,
  image_id          INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
  partition_number  INTEGER NOT NULL,
  fstype            TEXT    NOT NULL,
  size_bytes        INTEGER NOT NULL,
  UNIQUE (image_id, partition_number)
) STRICT;
