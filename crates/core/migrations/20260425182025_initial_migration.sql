CREATE TABLE hosts (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  mac             TEXT    NOT NULL UNIQUE,
  name            TEXT    NOT NULL,
  disk_size_bytes INTEGER NOT NULL
) STRICT;
