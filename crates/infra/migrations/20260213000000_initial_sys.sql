CREATE SCHEMA IF NOT EXISTS sys;

CREATE TABLE IF NOT EXISTS sys.users (
    id UUID PRIMARY KEY,
    username TEXT UNIQUE,
    email TEXT,
    password_hash TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT 'user',
    wallpaper TEXT,
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS sys.system_config (
    key TEXT PRIMARY KEY,
    value TEXT,
    updated_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

-- 初始化引导状态
INSERT INTO sys.system_config (key, value) VALUES ('setup_completed', 'false') ON CONFLICT (key) DO NOTHING;

CREATE TABLE IF NOT EXISTS sys.system_stats (
    id SERIAL PRIMARY KEY,
    cpu_usage DOUBLE PRECISION,
    memory_usage DOUBLE PRECISION,
    gpu_usage DOUBLE PRECISION,
    net_recv_kbps DOUBLE PRECISION,
    net_sent_kbps DOUBLE PRECISION,
    disk_usage DOUBLE PRECISION,
    disk_read_kbps DOUBLE PRECISION,
    disk_write_kbps DOUBLE PRECISION,
    gpu_memory_usage DOUBLE PRECISION,
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS sys.app_permissions (
    id SERIAL PRIMARY KEY,
    app_name TEXT NOT NULL,
    username TEXT NOT NULL,
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(app_name, username)
);

INSERT INTO sys.app_permissions (app_name, username) VALUES ('jellyfin', 'zac') ON CONFLICT (app_name, username) DO NOTHING;
