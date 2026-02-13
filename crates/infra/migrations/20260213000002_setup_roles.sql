-- 1. 创建角色 (Role)
-- role_sys: 负责系统核心业务，如用户管理、配置、日志等
DO $$
BEGIN
    IF NOT EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'role_sys') THEN
        CREATE ROLE role_sys;
    END IF;
    -- role_storage: 负责文件存储、下载、任务管理等
    IF NOT EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'role_storage') THEN
        CREATE ROLE role_storage;
    END IF;
END
$$;

-- 2. Schema 级权限分配
-- 授予 Schema 的访问权限
GRANT USAGE ON SCHEMA sys TO role_sys;
GRANT USAGE ON SCHEMA sys TO role_storage; -- role_storage 需要进入 sys 模式以查询 users 表
GRANT USAGE ON SCHEMA storage TO role_storage;

-- 3. 表级权限分配
-- role_sys 拥有 sys 和 storage 下所有表的全部权限 (作为管理角色)
GRANT ALL PRIVILEGES ON ALL TABLES IN SCHEMA sys TO role_sys;
GRANT ALL PRIVILEGES ON ALL SEQUENCES IN SCHEMA sys TO role_sys;
GRANT ALL PRIVILEGES ON ALL TABLES IN SCHEMA storage TO role_sys;
GRANT ALL PRIVILEGES ON ALL SEQUENCES IN SCHEMA storage TO role_sys;

-- role_storage 仅拥有 storage 下所有表的全部权限
GRANT ALL PRIVILEGES ON ALL TABLES IN SCHEMA storage TO role_storage;
GRANT ALL PRIVILEGES ON ALL SEQUENCES IN SCHEMA storage TO role_storage;

-- 4. 数据复用权限 (关键隔离点)
-- role_storage 需要能够查询用户信息（用于校验权限），但不能修改用户信息
GRANT SELECT ON sys.users TO role_storage;
GRANT SELECT ON sys.app_permissions TO role_storage;

-- 5. 限制默认权限 (安全加固)
-- 撤销 public 角色对 sys 和 storage 的默认权限（如果存在）
REVOKE ALL ON SCHEMA sys FROM PUBLIC;
REVOKE ALL ON SCHEMA storage FROM PUBLIC;

-- 6. 自动为未来创建的表分配权限
ALTER DEFAULT PRIVILEGES IN SCHEMA sys GRANT ALL ON TABLES TO role_sys;
ALTER DEFAULT PRIVILEGES IN SCHEMA storage GRANT ALL ON TABLES TO role_storage;

-- 7. 将角色授予当前连接用户（确保迁移执行者和未来的后端进程有权使用这些角色）
-- 注意：这里假设后端进程使用的用户就是执行迁移的用户
GRANT role_sys, role_storage TO CURRENT_USER;
