-- Create Schema
CREATE SCHEMA IF NOT EXISTS storage;

-- File Tasks
CREATE TABLE IF NOT EXISTS storage.file_tasks (
    id UUID PRIMARY KEY,
    type TEXT NOT NULL,
    name TEXT NOT NULL,
    dir TEXT,
    progress INTEGER DEFAULT 0,
    status TEXT NOT NULL,
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

-- Downloads
CREATE TABLE IF NOT EXISTS storage.downloads (
    id UUID PRIMARY KEY,
    url TEXT NOT NULL,
    path TEXT NOT NULL,
    filename TEXT NOT NULL,
    status TEXT NOT NULL,
    progress DOUBLE PRECISION DEFAULT 0,
    total_bytes BIGINT DEFAULT 0,
    downloaded_bytes BIGINT DEFAULT 0,
    speed BIGINT DEFAULT 0,
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    error_msg TEXT
);

-- Cloud Files
CREATE TABLE IF NOT EXISTS storage.cloud_files (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL,
    name TEXT NOT NULL,
    dir TEXT,
    size BIGINT DEFAULT 0,
    mime TEXT,
    checksum TEXT,
    storage TEXT NOT NULL DEFAULT 'file',
    blob_hash VARCHAR(64),
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(user_id, dir, name)
);

-- Indexes for Cloud Files
CREATE INDEX IF NOT EXISTS idx_cloud_files_user_dir ON storage.cloud_files(user_id, dir);
CREATE INDEX IF NOT EXISTS idx_cloud_files_user_dir_name ON storage.cloud_files(user_id, dir, name);
CREATE INDEX IF NOT EXISTS idx_cloud_files_checksum ON storage.cloud_files(checksum);
CREATE INDEX IF NOT EXISTS idx_cloud_files_blob_hash ON storage.cloud_files (blob_hash);

-- Multipart Uploads
CREATE TABLE IF NOT EXISTS storage.multipart_uploads (
    upload_id VARCHAR(255) PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES sys.users(id),
    dir VARCHAR(4096) NOT NULL,
    name VARCHAR(255) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_multipart_uploads_created_at ON storage.multipart_uploads(created_at);

-- Upload Parts
CREATE TABLE IF NOT EXISTS storage.upload_parts (
    upload_id VARCHAR(255) NOT NULL REFERENCES storage.multipart_uploads(upload_id) ON DELETE CASCADE,
    part_number INT NOT NULL,
    etag VARCHAR(255) NOT NULL,
    size BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (upload_id, part_number)
);

-- Permissions for role_storage (Owner)
GRANT USAGE ON SCHEMA storage TO role_storage;
GRANT ALL PRIVILEGES ON ALL TABLES IN SCHEMA storage TO role_storage;
GRANT ALL PRIVILEGES ON ALL SEQUENCES IN SCHEMA storage TO role_storage;

-- Permissions for role_sys (Admin)
GRANT USAGE ON SCHEMA storage TO role_sys;
GRANT ALL PRIVILEGES ON ALL TABLES IN SCHEMA storage TO role_sys;
GRANT ALL PRIVILEGES ON ALL SEQUENCES IN SCHEMA storage TO role_sys;

-- Revoke public access
REVOKE ALL ON SCHEMA storage FROM PUBLIC;

-- Default privileges
ALTER DEFAULT PRIVILEGES IN SCHEMA storage GRANT ALL ON TABLES TO role_storage;
