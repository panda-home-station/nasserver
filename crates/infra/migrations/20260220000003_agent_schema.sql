-- Create Schema
CREATE SCHEMA IF NOT EXISTS agent;

-- Chat Sessions
CREATE TABLE IF NOT EXISTS agent.chat_sessions (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES sys.users(id) ON DELETE CASCADE,
    agent_id TEXT NOT NULL,
    title TEXT NOT NULL,
    last_message TEXT,
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

-- Chat Messages
CREATE TABLE IF NOT EXISTS agent.chat_messages (
    id UUID PRIMARY KEY,
    session_id UUID NOT NULL REFERENCES agent.chat_sessions(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    tool_calls JSONB,
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

-- Permissions for role_agent (Owner)
GRANT USAGE ON SCHEMA agent TO role_agent;
GRANT ALL PRIVILEGES ON ALL TABLES IN SCHEMA agent TO role_agent;
GRANT ALL PRIVILEGES ON ALL SEQUENCES IN SCHEMA agent TO role_agent;

-- Permissions for role_sys (Admin)
GRANT USAGE ON SCHEMA agent TO role_sys;
GRANT ALL PRIVILEGES ON ALL TABLES IN SCHEMA agent TO role_sys;
GRANT ALL PRIVILEGES ON ALL SEQUENCES IN SCHEMA agent TO role_sys;

-- Revoke public access
REVOKE ALL ON SCHEMA agent FROM PUBLIC;

-- Default privileges
ALTER DEFAULT PRIVILEGES IN SCHEMA agent GRANT ALL ON TABLES TO role_agent;
