ALTER TABLE storage.cloud_files ADD COLUMN blob_hash VARCHAR(64);
CREATE INDEX IF NOT EXISTS idx_cloud_files_blob_hash ON storage.cloud_files (blob_hash);
