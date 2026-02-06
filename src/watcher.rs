use crate::state::AppState;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;
use notify::event::{ModifyKind, RenameMode, AccessKind};
use uuid::Uuid;

pub async fn init(state: AppState) {
    // Determine the path to watch: {storage_path}/vol1/User
    let watch_path = format!("{}/vol1/User", state.storage_path);
    let watch_path_buf = PathBuf::from(&watch_path);

    // Initial Scan: Sync existing files to DB
    let state_clone = state.clone();
    let watch_path_clone = watch_path.clone();
    tokio::spawn(async move {
        // Step 0: Cleanup orphan .part files
        println!("Starting cleanup of orphan .part files on {}", watch_path_clone);
        if let Err(e) = cleanup_temp_files(&watch_path_clone).await {
            eprintln!("Cleanup failed: {}", e);
        }

        println!("Starting initial filesystem scan on {}", watch_path_clone);
        if let Err(e) = initial_scan(&state_clone, &watch_path_clone).await {
            eprintln!("Initial scan failed: {}", e);
        } else {
            println!("Initial filesystem scan completed.");
        }
    });

    // Create a channel to receive events
    let (tx, mut rx) = mpsc::unbounded_channel();

    // Create the watcher
    let mut watcher: RecommendedWatcher = Watcher::new(
        move |res: notify::Result<Event>| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        },
        Config::default(),
    )
    .expect("Failed to create file watcher");

    // Start watching
    if let Err(e) = watcher.watch(&watch_path_buf, RecursiveMode::Recursive) {
        eprintln!("Failed to watch directory {}: {}", watch_path, e);
        return;
    }

    println!("File system watcher started on {}", watch_path);

    // Spawn the event processing loop
    tokio::spawn(async move {
        let _watcher_guard = watcher; 

        while let Some(event) = rx.recv().await {
            process_event(&state, event).await;
        }
    });
}

use std::collections::VecDeque;

async fn cleanup_temp_files(root_path: &str) -> std::io::Result<()> {
    let mut queue = VecDeque::new();
    queue.push_back(PathBuf::from(root_path));

    while let Some(current_dir) = queue.pop_front() {
        let mut entries = match tokio::fs::read_dir(&current_dir).await {
            Ok(e) => e,
            Err(_) => continue,
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            
            if path.is_dir() {
                queue.push_back(path.clone());
                continue;
            }

            // Check if it's a .part file
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') && name.ends_with(".part") {
                    println!("Cleaning up orphan file: {:?}", path);
                    let _ = tokio::fs::remove_file(path).await;
                }
            }
        }
    }
    Ok(())
}

async fn initial_scan(state: &AppState, root_path: &str) -> std::io::Result<()> {
    let mut queue = VecDeque::new();
    queue.push_back(PathBuf::from(root_path));

    while let Some(current_dir) = queue.pop_front() {
        let mut entries = match tokio::fs::read_dir(&current_dir).await {
            Ok(e) => e,
            Err(_) => continue,
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            
            // Ignore dotfiles
            if path.file_name().map_or(false, |n| n.to_string_lossy().starts_with('.')) {
                continue;
            }

            if path.is_dir() {
                queue.push_back(path.clone());
            }

            // Sync to DB
            handle_upsert(state, &path).await;
        }
    }
    Ok(())
}

async fn process_event(state: &AppState, event: Event) {
    // Filter out noisy events
    if let EventKind::Access(AccessKind::Open(_)) = event.kind {
        return;
    }
    if let EventKind::Access(AccessKind::Read) = event.kind {
        return;
    }

    println!("Watcher received event: {:?}", event); 

    match event.kind {
        EventKind::Modify(ModifyKind::Name(mode)) => {
            match mode {
                RenameMode::Both => {
                    if event.paths.len() == 2 {
                        handle_rename(state, &event.paths[0], &event.paths[1]).await;
                    }
                }
                RenameMode::To => {
                     if let Some(path) = event.paths.first() {
                         handle_upsert(state, path).await;
                    }
                }
                RenameMode::From => {
                     if let Some(path) = event.paths.first() {
                         handle_delete(state, path).await;
                    }
                }
                _ => {}
            }
        }
        EventKind::Create(_) | EventKind::Modify(ModifyKind::Data(_)) | EventKind::Access(AccessKind::Close(_)) => {
             for path in &event.paths {
                 handle_upsert(state, path).await;
             }
        }
        EventKind::Remove(_) => {
             for path in &event.paths {
                 handle_delete(state, path).await;
             }
        }
        _ => {}
    }
}

fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .map_or(false, |n| n.to_string_lossy().starts_with('.'))
}

async fn handle_rename(state: &AppState, from: &Path, to: &Path) {
    let from_hidden = is_hidden(from);
    let to_hidden = is_hidden(to);

    if from_hidden && !to_hidden {
        // .part -> file : Created
        println!("Watcher: Atomic Upload Detected {:?} -> {:?}", from, to);
        handle_upsert(state, to).await;
    } else if !from_hidden && to_hidden {
        // file -> .hidden : Deleted
        handle_delete(state, from).await;
    } else if !from_hidden && !to_hidden {
        // file -> file : Moved/Renamed
        handle_move(state, from, to).await;
    }
}

struct FileInfo {
    user_id: Uuid,
    parent_dir: String,
    name: String,
    is_dir: bool,
    size: i64,
    mime: String,
    storage_type: String,
}

async fn parse_path(state: &AppState, path: &Path) -> Option<FileInfo> {
    let storage_base = PathBuf::from(&state.storage_path).join("vol1/User");
    
    println!("ParsePath: checking {:?} against base {:?}", path, storage_base);

    let rel_path = match path.strip_prefix(&storage_base) {
        Ok(p) => p,
        Err(_) => {
            println!("ParsePath: Mismatch! Path does not start with base.");
            return None;
        }
    };

    let rel_path_str = rel_path.to_string_lossy();
    let parts: Vec<&str> = rel_path_str.splitn(2, '/').collect();
    if parts.is_empty() || parts[0].is_empty() {
        return None;
    }
    let username = parts[0];
    let virtual_rel_path = if parts.len() > 1 { parts[1] } else { "" };
    
    if is_hidden(path) {
        println!("ParsePath: File is hidden, skipping: {:?}", path);
        return None;
    }

    // Get User ID
    let user_id_result = sqlx::query_scalar::<_, Uuid>("select id from users where username = $1")
        .bind(username)
        .fetch_optional(&state.db)
        .await;

    let user_id = match user_id_result {
        Ok(Some(id)) => Some(id),
        Ok(None) => {
            println!("ParsePath: User '{}' not found in DB (query returned None)", username);
            None
        },
        Err(e) => {
            println!("ParsePath: DB Error looking up user '{}': {}", username, e);
            None
        }
    };

    if user_id.is_none() {
        return None;
    }

    let user_id = user_id?;

    // Construct Virtual Path info
    let virtual_path = format!("/{}", virtual_rel_path);
    let parent_dir = Path::new(&virtual_path)
        .parent()
        .unwrap_or(Path::new("/"))
        .to_string_lossy()
        .to_string();
    let parent_dir = if parent_dir == "/" { "/".to_string() } else if parent_dir.starts_with('/') { parent_dir } else { format!("/{}", parent_dir) };
    
    let name = Path::new(&virtual_path)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    if name.is_empty() { return None; }

    let is_dir = path.is_dir();
    let size = if is_dir { 0 } else { path.metadata().map(|m| m.len()).unwrap_or(0) as i64 };
    let storage_type = if is_dir { "dir" } else { "file" };
    let mime = if is_dir { "directory".to_string() } else { mime_guess::from_path(&path).first_or_octet_stream().to_string() };

    Some(FileInfo {
        user_id,
        parent_dir,
        name,
        is_dir,
        size,
        mime,
        storage_type: storage_type.to_string(),
    })
}

async fn handle_upsert(state: &AppState, path: &Path) {
    println!("HandleUpsert: Processing {:?}", path);
    if let Some(info) = parse_path(state, path).await {
         // Check if exists first to handle non-unique index
         println!("HandleUpsert: Parsed info for {}", info.name);
         let exists: bool = sqlx::query_scalar::<_, i64>("SELECT 1 FROM cloud_files WHERE user_id = $1 AND dir = $2 AND name = $3")
            .bind(&info.user_id)
            .bind(&info.parent_dir)
            .bind(&info.name)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None)
            .is_some();

        if exists {
             let _ = sqlx::query("UPDATE cloud_files SET size = $1, mime = $2, updated_at = datetime('now') WHERE user_id = $3 AND dir = $4 AND name = $5")
                .bind(info.size)
                .bind(&info.mime)
                .bind(&info.user_id)
                .bind(&info.parent_dir)
                .bind(&info.name)
                .execute(&state.db)
                .await;
        } else {
            let _ = sqlx::query("INSERT INTO cloud_files (id, user_id, name, dir, size, mime, storage, created_at, updated_at) 
                VALUES ($1, $2, $3, $4, $5, $6, $7, datetime('now'), datetime('now'))")
                .bind(uuid::Uuid::new_v4().to_string())
                .bind(&info.user_id)
                .bind(&info.name)
                .bind(&info.parent_dir)
                .bind(info.size)
                .bind(&info.mime)
                .bind(&info.storage_type)
                .execute(&state.db)
                .await;
        }
    }
}

async fn handle_delete(state: &AppState, path: &Path) {
    // Note: for delete, we can't get metadata like size/is_dir easily if it's gone.
    // But parse_path tries to read metadata.
    // So we need a simplified parse logic for delete that doesn't check metadata or existence.
    // However, parse_path mostly just needs path string and DB lookup for user.
    // We can modify parse_path or just do manual parsing here.
    
    let path_str = path.to_string_lossy();
    let storage_base = format!("{}/vol1/User/", state.storage_path);
    
    if !path_str.starts_with(&storage_base) { return; }
    let rel_path = &path_str[storage_base.len()..];
    let parts: Vec<&str> = rel_path.splitn(2, '/').collect();
    if parts.is_empty() { return; }
    let username = parts[0];
    let virtual_rel_path = if parts.len() > 1 { parts[1] } else { "" };
    
    if is_hidden(path) { return; }

    let user_id: Option<Uuid> = sqlx::query_scalar("select id from users where username = $1")
        .bind(username)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);
        
    if let Some(uid) = user_id {
        let virtual_path = format!("/{}", virtual_rel_path);
        let parent_dir = Path::new(&virtual_path).parent().unwrap_or(Path::new("/")).to_string_lossy().to_string();
        let parent_dir = if parent_dir == "/" { "/".to_string() } else if parent_dir.starts_with('/') { parent_dir } else { format!("/{}", parent_dir) };
        let name = Path::new(&virtual_path).file_name().unwrap_or_default().to_string_lossy().to_string();
        
        if !name.is_empty() {
            let _ = sqlx::query("DELETE FROM cloud_files WHERE user_id = $1 AND dir = $2 AND name = $3")
                .bind(&uid)
                .bind(&parent_dir)
                .bind(&name)
                .execute(&state.db)
                .await;
        }
    }
}

async fn handle_move(state: &AppState, from: &Path, to: &Path) {
    // For move, we want to update the entry.
    // We need old coords and new coords.
    // Re-use logic from handle_delete (old) and handle_upsert (new).
    
    // Parse From
    let path_str_from = from.to_string_lossy();
    let storage_base = format!("{}/vol1/User/", state.storage_path);
    if !path_str_from.starts_with(&storage_base) { return; }
    let rel_path_from = &path_str_from[storage_base.len()..];
    let parts_from: Vec<&str> = rel_path_from.splitn(2, '/').collect();
    let username_from = parts_from[0];
    let virtual_rel_path_from = if parts_from.len() > 1 { parts_from[1] } else { "" };
    
    // Parse To
    let path_str_to = to.to_string_lossy();
    if !path_str_to.starts_with(&storage_base) { return; }
    let rel_path_to = &path_str_to[storage_base.len()..];
    let parts_to: Vec<&str> = rel_path_to.splitn(2, '/').collect();
    let username_to = parts_to[0]; // Should be same usually
    let virtual_rel_path_to = if parts_to.len() > 1 { parts_to[1] } else { "" };
    
    if username_from != username_to {
        // Cross-user move? Treat as delete + create
        handle_delete(state, from).await;
        handle_upsert(state, to).await;
        return;
    }
    
    let user_id: Option<Uuid> = sqlx::query_scalar("select id from users where username = $1")
        .bind(username_from)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);
        
    if let Some(uid) = user_id {
        // Old Coords
        let v_from = format!("/{}", virtual_rel_path_from);
        let p_from = Path::new(&v_from).parent().unwrap_or(Path::new("/")).to_string_lossy().to_string();
        let p_from = if p_from == "/" { "/".to_string() } else if p_from.starts_with('/') { p_from } else { format!("/{}", p_from) };
        let n_from = Path::new(&v_from).file_name().unwrap_or_default().to_string_lossy().to_string();
        
        // New Coords
        let v_to = format!("/{}", virtual_rel_path_to);
        let p_to = Path::new(&v_to).parent().unwrap_or(Path::new("/")).to_string_lossy().to_string();
        let p_to = if p_to == "/" { "/".to_string() } else if p_to.starts_with('/') { p_to } else { format!("/{}", p_to) };
        let n_to = Path::new(&v_to).file_name().unwrap_or_default().to_string_lossy().to_string();
        
        // Update
        let res = sqlx::query("UPDATE cloud_files SET dir = $1, name = $2, updated_at = datetime('now') WHERE user_id = $3 AND dir = $4 AND name = $5")
            .bind(&p_to)
            .bind(&n_to)
            .bind(&uid)
            .bind(&p_from)
            .bind(&n_from)
            .execute(&state.db)
            .await;
            
        // If update failed (e.g. row not found), try upsert
        if let Ok(r) = res {
            if r.rows_affected() == 0 {
                handle_upsert(state, to).await;
            }
        } else {
             handle_upsert(state, to).await;
        }
    }
}

