-- storage: Trash schema (random UUID filenames)
CREATE SCHEMA IF NOT EXISTS storage;

-- Single table for trash items. blob_hash stores the random UUID filename
-- under: vol1/User/<user>/.Trash/blobs/<uuid>
CREATE TABLE IF NOT EXISTS storage.trash_items(
  id UUID PRIMARY KEY,
  user_id UUID NOT NULL REFERENCES sys.users(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  original_dir TEXT NOT NULL,
  is_dir BOOLEAN NOT NULL,
  size BIGINT NOT NULL DEFAULT 0,
  mime TEXT NOT NULL,
  deleted_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
  blob_hash TEXT
);

-- Indices for efficient listing and lookups
CREATE INDEX IF NOT EXISTS idx_trash_items_user_dir ON storage.trash_items(user_id, original_dir);
CREATE INDEX IF NOT EXISTS idx_trash_items_user_dir_name ON storage.trash_items(user_id, original_dir, name);
CREATE INDEX IF NOT EXISTS idx_trash_items_deleted_at ON storage.trash_items(deleted_at);
CREATE INDEX IF NOT EXISTS idx_trash_items_blob_hash ON storage.trash_items(blob_hash);
