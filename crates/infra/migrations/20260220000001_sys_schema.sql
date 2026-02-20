-- Create Schema
CREATE SCHEMA IF NOT EXISTS sys;

-- Users Table
CREATE TABLE IF NOT EXISTS sys.users (
    id UUID PRIMARY KEY,
    username TEXT UNIQUE,
    email TEXT,
    password_hash TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT 'user',
    wallpaper TEXT,
    idle_timeout INTEGER DEFAULT 0,
    idle_action TEXT DEFAULT 'lock',
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

-- System Config
CREATE TABLE IF NOT EXISTS sys.system_config (
    key TEXT PRIMARY KEY,
    value TEXT,
    updated_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

-- Insert initial setup state
INSERT INTO sys.system_config (key, value) VALUES ('setup_completed', 'false') ON CONFLICT (key) DO NOTHING;

-- System Stats
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

-- App Permissions
CREATE TABLE IF NOT EXISTS sys.app_permissions (
    id SERIAL PRIMARY KEY,
    app_name TEXT NOT NULL,
    username TEXT NOT NULL,
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(app_name, username)
);
INSERT INTO sys.app_permissions (app_name, username) VALUES ('jellyfin', 'zac') ON CONFLICT (app_name, username) DO NOTHING;

-- Permissions for role_sys (Owner/Admin)
GRANT USAGE ON SCHEMA sys TO role_sys;
GRANT ALL PRIVILEGES ON ALL TABLES IN SCHEMA sys TO role_sys;
GRANT ALL PRIVILEGES ON ALL SEQUENCES IN SCHEMA sys TO role_sys;

-- Permissions for role_storage (Cross-access)
GRANT USAGE ON SCHEMA sys TO role_storage;
GRANT SELECT ON sys.users TO role_storage;
GRANT SELECT ON sys.app_permissions TO role_storage;

-- Permissions for role_agent (Cross-access)
GRANT USAGE ON SCHEMA sys TO role_agent;
GRANT SELECT ON sys.users TO role_agent;

-- Revoke public access
REVOKE ALL ON SCHEMA sys FROM PUBLIC;

-- Default privileges for future tables created by migration user
ALTER DEFAULT PRIVILEGES IN SCHEMA sys GRANT ALL ON TABLES TO role_sys;
