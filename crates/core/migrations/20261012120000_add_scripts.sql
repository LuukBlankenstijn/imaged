CREATE TABLE scripts (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  name       TEXT    NOT NULL UNIQUE,
  body       TEXT    NOT NULL,
  updated_at TEXT    NOT NULL
) STRICT;

CREATE TABLE script_inputs (
  script_id     INTEGER NOT NULL REFERENCES scripts(id) ON DELETE CASCADE,
  position      INTEGER NOT NULL,
  name          TEXT    NOT NULL,
  label         TEXT    NOT NULL,
  kind          TEXT    NOT NULL,
  required      INTEGER NOT NULL,
  default_value TEXT,
  per_machine   INTEGER NOT NULL,
  choices       TEXT    NOT NULL,
  PRIMARY KEY (script_id, position),
  UNIQUE (script_id, name)
) STRICT;

CREATE TABLE image_scripts (
  image_id  INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
  position  INTEGER NOT NULL,
  script_id INTEGER NOT NULL REFERENCES scripts(id),
  PRIMARY KEY (image_id, position)
) STRICT;

CREATE INDEX idx_image_scripts_script_id ON image_scripts(script_id);

ALTER TABLE images ADD COLUMN root_partition INTEGER;
