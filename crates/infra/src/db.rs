use sqlx::postgres::{PgPool, PgPoolOptions, PgConnectOptions};
use sqlx::ConnectOptions;
use std::str::FromStr;

pub struct DbPools {
    pub sys: PgPool,      // 使用 role_sys
    pub storage: PgPool,  // 使用 role_storage
}

pub async fn init_db(db_url: &str) -> DbPools {
    // 1. 预检查并创建数据库与基础角色
    prepare_database(db_url).await;

    // 2. 使用管理员权限连接并运行迁移脚本
    let admin_pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(db_url)
        .await
        .expect("Failed to connect to database for migrations");
    
    run_migrations(&admin_pool).await;
    // 显式关闭管理池
    admin_pool.close().await;

    // 3. 创建应用专用的受限连接池
    // sys 专用连接池 (使用 role_sys)
    let sys_pool = create_pool(db_url, "role_sys", "sys, public").await;
    
    // storage 专用连接池 (使用 role_storage)
    let storage_pool = create_pool(db_url, "role_storage", "storage, public").await;

    // 4. 如果存在旧的 SQLite 数据库，执行数据迁移
    crate::data_migration::migrate_from_sqlite_if_needed(&sys_pool).await;

    DbPools {
        sys: sys_pool,
        storage: storage_pool,
    }
}

async fn create_pool(db_url: &str, role: &str, search_path: &str) -> PgPool {
    let pool_role = role.to_string();
    let r_clone = role.to_string();
    let s_clone = search_path.to_string();
    
    PgPoolOptions::new()
        .max_connections(5)
        .after_connect(move |conn, _meta| {
            let r = r_clone.clone();
            let s = s_clone.clone();
            Box::pin(async move {
                // 设置角色
                sqlx::query(&format!("SET ROLE {}", r)).execute(&mut *conn).await?;
                // 设置搜索路径
                sqlx::query(&format!("SET search_path TO {}", s)).execute(&mut *conn).await?;
                Ok(())
            })
        })
        .connect(db_url)
        .await
        .expect(&format!("Failed to connect to database for role {}", pool_role))
}

async fn prepare_database(db_url: &str) {
    let opts = PgConnectOptions::from_str(db_url).expect("Invalid DATABASE_URL");
    let target_db = opts.get_database().unwrap_or("pnas_db");
    let target_user = opts.get_username();

    // 尝试连接到默认的 postgres 数据库来执行管理操作
    let admin_opts = opts.clone().database("postgres");
    
    let mut conn = match admin_opts.connect().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Warning: Could not connect to postgres database for admin tasks: {}", e);
            return;
        }
    };

    // 1. 创建数据库
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT FROM pg_database WHERE datname = $1)")
        .bind(target_db)
        .fetch_one(&mut conn)
        .await
        .unwrap_or(false);

    if !exists {
        println!("Database {} does not exist, creating...", target_db);
        let query = format!("CREATE DATABASE \"{}\"", target_db);
        let _ = sqlx::query(&query).execute(&mut conn).await;
    }

    // 2. 创建角色 (RBAC 基础)
    println!("Ensuring database roles exist...");
    let role_setup = r#"
        DO $$
        BEGIN
            IF NOT EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'role_sys') THEN
                CREATE ROLE role_sys;
            END IF;
            IF NOT EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = 'role_storage') THEN
                CREATE ROLE role_storage;
            END IF;
        END
        $$;
    "#;
    
    if let Err(e) = sqlx::query(role_setup).execute(&mut conn).await {
        eprintln!("Warning: Failed to create roles: {}", e);
    }

    // 3. 将角色授予应用用户，以便后端可以切换角色
    let grant_query = format!("GRANT role_sys, role_storage TO \"{}\"", target_user);
    let _ = sqlx::query(&grant_query).execute(&mut conn).await;

    // 4. 连接到目标数据库授予角色对 schema public 的权限 (Postgres 15+ 默认限制)
    drop(conn); // 断开 postgres 库连接
    
    let target_opts = opts.clone().database(target_db);
    let mut target_conn = match target_opts.connect().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Warning: Could not connect to target database {} for permission setup: {}", target_db, e);
            return;
        }
    };

    println!("Setting up base permissions for role_sys on {}...", target_db);
    let perms = [
        format!(r#"GRANT CONNECT ON DATABASE "{}" TO role_sys"#, target_db),
        format!(r#"GRANT CONNECT ON DATABASE "{}" TO role_storage"#, target_db),
        "GRANT ALL ON SCHEMA public TO role_sys".to_string(),
        format!(r#"GRANT CREATE ON DATABASE "{}" TO role_sys"#, target_db),
    ];
    
    for perm in perms {
        if let Err(e) = sqlx::query(&perm).execute(&mut target_conn).await {
            eprintln!("Warning: Failed to setup base permission ({}): {}", perm, e);
        }
    }
}

async fn run_migrations(db: &PgPool) {
    println!("Running database migrations...");
    // 这里的 migrations 文件夹会被嵌入到二进制文件中
    sqlx::migrate!("./migrations")
        .run(db)
        .await
        .expect("Failed to run database migrations");
    println!("Database migrations completed");
}
