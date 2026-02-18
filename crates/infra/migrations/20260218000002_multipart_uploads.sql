CREATE TABLE IF NOT EXISTS storage.multipart_uploads (
    upload_id VARCHAR(255) PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES sys.users(id),
    dir VARCHAR(4096) NOT NULL,
    name VARCHAR(255) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_multipart_uploads_created_at ON storage.multipart_uploads(created_at);

CREATE TABLE IF NOT EXISTS storage.upload_parts (
    upload_id VARCHAR(255) NOT NULL REFERENCES storage.multipart_uploads(upload_id) ON DELETE CASCADE,
    part_number INT NOT NULL,
    etag VARCHAR(255) NOT NULL,
    size BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (upload_id, part_number)
);
