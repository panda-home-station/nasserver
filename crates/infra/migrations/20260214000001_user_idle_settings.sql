-- Add idle_timeout and idle_action to users table
ALTER TABLE sys.users ADD COLUMN IF NOT EXISTS idle_timeout INTEGER DEFAULT 0;
ALTER TABLE sys.users ADD COLUMN IF NOT EXISTS idle_action TEXT DEFAULT 'lock';
