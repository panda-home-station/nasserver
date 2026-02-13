use sqlx::postgres::{PgPool, PgPoolOptions, PgConnectOptions};
use sqlx::ConnectOptions;
use std::str::FromStr;

pub async fn init_db(db_url: &str) -> PgPool {
    // 1. 预检查并创建数据库
    ensure_database_exists(db_url).await;

    // 2. 建立正式连接池
    let db = match PgPoolOptions::new()
        .max_connections(5)
        .after_connect(|conn, _meta| Box::pin(async move {
            // 每次连接自动设置搜索路径，这样代码里就不需要写 sys.users，直接写 users
            sqlx::query("SET search_path TO sys, storage, public")
                .execute(conn)
                .await?;
            Ok(())
        }))
        .connect(db_url)
        .await 
    {
        Ok(pool) => {
            println!("Database connected successfully");
            pool
        },
        Err(e) => {
            eprintln!("Failed to connect to database: {}. Error: {}", db_url, e);
            std::process::exit(1);
        }
    };
    
    // 3. 运行嵌入式迁移脚本 (创建 Schema 和表)
    run_migrations(&db).await;

    // 4. 如果存在旧的 SQLite 数据库，执行数据迁移
    crate::data_migration::migrate_from_sqlite_if_needed(&db).await;

    db
}

async fn ensure_database_exists(db_url: &str) {
    let opts = PgConnectOptions::from_str(db_url).expect("Invalid DATABASE_URL");
    let target_db = opts.get_database().unwrap_or("pnas_db");

    // 尝试连接到默认的 postgres 数据库来检查目标库是否存在
    let admin_opts = opts.clone().database("postgres");
    
    let mut conn = match admin_opts.connect().await {
        Ok(c) => c,
        Err(_) => return, // 如果连不上 postgres 库，可能权限不足，跳过自动创建尝试
    };

    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT FROM pg_database WHERE datname = $1)")
        .bind(target_db)
        .fetch_one(&mut conn)
        .await
        .unwrap_or(false);

    if !exists {
        println!("Database {} does not exist, creating...", target_db);
        // 注意：CREATE DATABASE 不能在事务中执行
        let query = format!("CREATE DATABASE \"{}\"", target_db);
        let _ = sqlx::query(&query).execute(&mut conn).await;
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
