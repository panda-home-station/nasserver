// DEPRECATED: 本模块已弃用，不再在服务入口处初始化。保留代码仅用于参考和可能的将来复用。
// 目前系统已迁移至基于数据库与 blob 的文件管理，不再需要对 $storage/vol1/User 进行扫描与监听。

use crate::state::AppState;
// use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
// use std::path::{Path, PathBuf};
// use tokio::sync::mpsc;
// use notify::event::{ModifyKind, RenameMode, AccessKind};

pub async fn init(_state: AppState) {
    /*
    let watch_path = format!("{}/vol1/User", state.storage_path);
    let watch_path_buf = PathBuf::from(&watch_path);

    let state_clone = state.clone();
    let watch_path_clone = watch_path.clone();
    tokio::spawn(async move {
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

    let (tx, mut rx) = mpsc::unbounded_channel();

    let mut watcher: RecommendedWatcher = Watcher::new(
        move |res: notify::Result<Event>| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        },
        Config::default(),
    )
    .expect("Failed to create file watcher");

    if let Err(e) = watcher.watch(&watch_path_buf, RecursiveMode::Recursive) {
        eprintln!("Failed to watch directory {}: {}", watch_path, e);
        return;
    }

    println!("File system watcher started on {}", watch_path);

    tokio::spawn(async move {
        let _watcher_guard = watcher; 

        while let Some(event) = rx.recv().await {
            process_event(&state, event).await;
        }
    });
    */
}

/*
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
            
            if path.file_name().map_or(false, |n| n.to_string_lossy().starts_with('.')) {
                continue;
            }

            if path.is_dir() {
                queue.push_back(path.clone());
            }

            let _ = state.storage_service.sync_external_change(&path).await;
        }
    }
    Ok(())
}

async fn process_event(state: &AppState, event: Event) {
    if let EventKind::Access(AccessKind::Open(_)) = event.kind {
        return;
    }
    if let EventKind::Access(AccessKind::Read) = event.kind {
        return;
    }

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
                         let _ = state.storage_service.sync_external_change(path).await;
                    }
                }
                RenameMode::From => {
                     if let Some(path) = event.paths.first() {
                         let _ = state.storage_service.remove_external_change(path).await;
                    }
                }
                _ => {}
            }
        }
        EventKind::Create(_) | EventKind::Modify(ModifyKind::Data(_)) | EventKind::Access(AccessKind::Close(_)) => {
             for path in &event.paths {
                 let _ = state.storage_service.sync_external_change(path).await;
             }
        }
        EventKind::Remove(_) => {
             for path in &event.paths {
                 let _ = state.storage_service.remove_external_change(path).await;
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
        let _ = state.storage_service.sync_external_change(to).await;
    } else if !from_hidden && to_hidden {
        let _ = state.storage_service.remove_external_change(from).await;
    } else if !from_hidden && !to_hidden {
        let _ = state.storage_service.move_external_change(from, to).await;
    }
}
*/
