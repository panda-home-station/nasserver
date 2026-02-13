use sqlx::{sqlite::SqlitePool, postgres::PgPool, Row};
use std::path::Path;
use uuid::Uuid;
use chrono::{DateTime, Utc};

pub async fn migrate_from_sqlite_if_needed(pg_pool: &PgPool) {
    let sqlite_path = std::env::var("SQLITE_PATH")
        .unwrap_or_else(|_| "/var/panda/system/db/pnas.db".to_string());

    if !Path::new(&sqlite_path).exists() {
        return;
    }

    println!("Found legacy SQLite database at {}, starting data migration...", sqlite_path);

    let sqlite_url = format!("sqlite:{}", sqlite_path);
    let sl_pool = match SqlitePool::connect(&sqlite_url).await {
        Ok(pool) => pool,
        Err(e) => {
            eprintln!("Failed to connect to legacy SQLite database: {}", e);
            return;
        }
    };

    let table_map = [
        ("users", "sys.users"),
        ("system_config", "sys.system_config"),
        ("system_stats", "sys.system_stats"),
        ("app_permissions", "sys.app_permissions"),
        ("file_tasks", "storage.file_tasks"),
        ("downloads", "storage.downloads"),
        ("cloud_files", "storage.cloud_files"),
    ];

    for (sl_table, pg_table) in table_map {
        if let Err(e) = migrate_table(&sl_pool, pg_pool, sl_table, pg_table).await {
            eprintln!("Failed to migrate table {} to {}: {}", sl_table, pg_table, e);
        }
    }

    println!("Data migration completed.");
    
    // Backup the old sqlite db
    let backup_path = format!("{}.bak", sqlite_path);
    if let Err(e) = std::fs::rename(&sqlite_path, &backup_path) {
        eprintln!("Failed to rename legacy SQLite database to {}: {}", backup_path, e);
    } else {
        println!("Legacy SQLite database backed up to {}", backup_path);
    }
}

async fn migrate_table(sl_pool: &SqlitePool, pg_pool: &PgPool, sl_table: &str, pg_table: &str) -> Result<(), sqlx::Error> {
    println!("  Migrating {} -> {}...", sl_table, pg_table);

    // 1. Get column names and types from SQLite
    let _rows = sqlx::query(&format!("SELECT * FROM {} LIMIT 0", sl_table))
        .fetch_all(sl_pool)
        .await?;
    
    // If table is empty, we still need column names. PRAGMA is more reliable.
    let columns: Vec<String> = sqlx::query(&format!("PRAGMA table_info({})", sl_table))
        .fetch_all(sl_pool)
        .await?
        .into_iter()
        .map(|row| row.get::<String, _>(1))
        .collect();

    if columns.is_empty() {
        println!("    Table {} not found in SQLite, skipping.", sl_table);
        return Ok(());
    }

    let col_names = columns.join(", ");
    let placeholders = (1..=columns.len())
        .map(|i| format!("${}", i))
        .collect::<Vec<_>>()
        .join(", ");

    // 2. Fetch data from SQLite
    let sl_rows = sqlx::query(&format!("SELECT {} FROM {}", col_names, sl_table))
        .fetch_all(sl_pool)
        .await?;

    if sl_rows.is_empty() {
        println!("    Table {} is empty.", sl_table);
        return Ok(());
    }

    // 3. Insert into PostgreSQL
    let insert_sql = format!(
        "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT DO NOTHING",
        pg_table, col_names, placeholders
    );

    let mut count = 0;
    for sl_row in sl_rows {
        let mut query = sqlx::query(&insert_sql);
        
        for (i, col_name) in columns.iter().enumerate() {
            // Try to handle special types based on column name or content
            if col_name == "id" || col_name.ends_with("_id") {
                if let Ok(val_str) = sl_row.try_get::<String, _>(i) {
                    if let Ok(uuid) = Uuid::parse_str(&val_str) {
                        query = query.bind(uuid);
                        continue;
                    }
                    query = query.bind(val_str);
                } else {
                    query = query.bind(None::<String>);
                }
            } else if col_name.ends_with("_at") {
                if let Ok(val_str) = sl_row.try_get::<String, _>(i) {
                    // SQLite timestamps are often strings
                    if let Ok(dt) = DateTime::parse_from_rfc3339(&val_str) {
                        query = query.bind(dt.with_timezone(&Utc));
                        continue;
                    }
                    // Try common SQLite format
                    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(&val_str, "%Y-%m-%d %H:%M:%S") {
                        query = query.bind(DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc));
                        continue;
                    }
                    query = query.bind(val_str);
                } else {
                    query = query.bind(None::<String>);
                }
            } else {
                // Generic binding
                if let Ok(val) = sl_row.try_get::<String, _>(i) {
                    query = query.bind(val);
                } else if let Ok(val) = sl_row.try_get::<i64, _>(i) {
                    query = query.bind(val);
                } else if let Ok(val) = sl_row.try_get::<f64, _>(i) {
                    query = query.bind(val);
                } else if let Ok(val) = sl_row.try_get::<bool, _>(i) {
                    query = query.bind(val);
                } else {
                    query = query.bind(None::<String>);
                }
            }
        }
        
        if let Err(e) = query.execute(pg_pool).await {
            eprintln!("    Error inserting row into {}: {}", pg_table, e);
        } else {
            count += 1;
        }
    }

    println!("    Successfully migrated {} rows to {}.", count, pg_table);
    Ok(())
}
