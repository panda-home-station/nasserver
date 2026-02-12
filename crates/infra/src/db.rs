use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};

pub async fn init_db(db_url: &str) -> SqlitePool {
    // Proactively create DB file if missing to avoid SQLITE_CANTOPEN (code 14)
    if let Some(path) = db_url.strip_prefix("sqlite://") {
        let db_path = if path.starts_with('/') { path.to_string() } else { format!("/{}", path) };
        if let Some(parent) = std::path::Path::new(&db_path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if !std::path::Path::new(&db_path).exists() {
            let _ = std::fs::File::create(&db_path);
        }
    }
    
    let db = match SqlitePoolOptions::new().max_connections(1).connect(db_url).await {
        Ok(pool) => {
            println!("Database connected successfully: {}", db_url);
            let _ = sqlx::query("PRAGMA journal_mode=WAL;")
                .execute(&pool)
                .await;
            pool
        },
        Err(e) => {
            eprintln!("Failed to connect to database: {}. Error: {}", db_url, e);
            std::process::exit(1);
        }
    };
    
    verify_schema(&db).await;
    db
}

async fn verify_schema(db: &SqlitePool) {
    println!("Initializing database schema...");
    
    // Users table
    sqlx::query(
        "create table if not exists users (
            id text primary key,
            username text unique,
            email text,
            password_hash text not null,
            created_at timestamp default CURRENT_TIMESTAMP
        )",
    )
    .execute(db)
    .await
    .expect("Failed to create users table");

    // Check if 'role' column exists
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM pragma_table_info('users') WHERE name = 'role'")
        .fetch_one(db)
        .await
        .expect("Failed to query pragma_table_info for users table");

    if row.0 == 0 {
        sqlx::query("ALTER TABLE users ADD COLUMN role TEXT NOT NULL DEFAULT 'user'")
            .execute(db)
            .await
            .expect("Failed to alter users table to add role column");
    }

    // Wallpaper column
    let row_wp: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM pragma_table_info('users') WHERE name = 'wallpaper'")
        .fetch_one(db)
        .await
        .expect("Failed to query pragma_table_info for users table (wallpaper)");
    if row_wp.0 == 0 {
        sqlx::query("ALTER TABLE users ADD COLUMN wallpaper TEXT")
            .execute(db)
            .await
            .expect("Failed to alter users table to add wallpaper column");
    }

    // System config
    sqlx::query(
        "create table if not exists system_config (
            key text primary key,
            value text
        )",
    )
    .execute(db)
    .await
    .expect("Failed to create system_config table");
    
    // File tasks
    sqlx::query(
        "create table if not exists file_tasks (
            id text primary key,
            type text not null,
            name text not null,
            dir text,
            progress integer default 0,
            status text not null,
            created_at timestamp default CURRENT_TIMESTAMP,
            updated_at timestamp default CURRENT_TIMESTAMP
        )",
    )
    .execute(db)
    .await
    .expect("Failed to create file_tasks table");

    // Downloads
    sqlx::query(
        "create table if not exists downloads (
            id text primary key,
            url text not null,
            path text not null,
            filename text not null,
            status text not null,
            progress real default 0,
            total_bytes integer default 0,
            downloaded_bytes integer default 0,
            speed integer default 0,
            created_at timestamp default CURRENT_TIMESTAMP,
            updated_at timestamp default CURRENT_TIMESTAMP,
            error_msg text
        )"
    )
    .execute(db)
    .await
    .expect("Failed to create downloads table");
    
    // Cloud files
    sqlx::query(
        "create table if not exists cloud_files (
            id text primary key,
            user_id text not null,
            name text not null,
            dir text,
            size integer default 0,
            mime text,
            checksum text,
            storage text not null default 'file',
            created_at timestamp default CURRENT_TIMESTAMP,
            updated_at timestamp default CURRENT_TIMESTAMP
        )",
    )
    .execute(db)
    .await
    .expect("Failed to create cloud_files table");

    // System stats
    sqlx::query(
        "create table if not exists system_stats (
            id integer primary key autoincrement,
            cpu_usage real,
            memory_usage real,
            gpu_usage real,
            net_recv_kbps real,
            net_sent_kbps real,
            disk_usage real,
            disk_read_kbps real,
            disk_write_kbps real,
            created_at timestamp default CURRENT_TIMESTAMP
        )"
    )
    .execute(db)
    .await
    .expect("Failed to create system_stats table");

    // Indexes
    sqlx::query("create index if not exists idx_cloud_files_user_dir on cloud_files(user_id, dir)")
        .execute(db)
        .await
        .expect("Failed to create idx_cloud_files_user_dir");
    sqlx::query("create index if not exists idx_cloud_files_user_dir_name on cloud_files(user_id, dir, name)")
        .execute(db)
        .await
        .expect("Failed to create idx_cloud_files_user_dir_name");
    sqlx::query("create index if not exists idx_cloud_files_checksum on cloud_files(checksum)")
        .execute(db)
        .await
        .expect("Failed to create idx_cloud_files_checksum");

    // GPU memory usage column
    let row_gpu_mem: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM pragma_table_info('system_stats') WHERE name = 'gpu_memory_usage'")
        .fetch_one(db)
        .await
        .expect("Failed to query pragma_table_info for system_stats table");

    if row_gpu_mem.0 == 0 {
        sqlx::query("ALTER TABLE system_stats ADD COLUMN gpu_memory_usage REAL")
            .execute(db)
            .await
            .expect("Failed to alter system_stats table to add gpu_memory_usage column");
    }

    // App permissions
    sqlx::query(
        "create table if not exists app_permissions (
            id integer primary key autoincrement,
            app_name text not null,
            username text not null,
            created_at timestamp default CURRENT_TIMESTAMP,
            unique(app_name, username)
        )",
    )
    .execute(db)
    .await
    .expect("Failed to create app_permissions table");

    // Seed default permissions
    let _ = sqlx::query("insert or ignore into app_permissions (app_name, username) values ('jellyfin', 'zac')")
        .execute(db)
        .await;
    
    println!("Database schema verified");
}
