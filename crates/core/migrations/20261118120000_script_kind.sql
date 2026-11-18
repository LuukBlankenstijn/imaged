ALTER TABLE scripts ADD COLUMN kind TEXT NOT NULL DEFAULT 'post_install'
  CHECK (kind IN ('post_install', 'client'));
