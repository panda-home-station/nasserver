use std::sync::Arc;
use boa_engine::{Context, JsValue, NativeFunction, JsString, Finalize, Trace, JsError};
use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use crate::service::TerminalService;
use domain::storage::{DocsListQuery, DocsMkdirReq, DocsDeleteQuery, DocsRenameReq};

#[derive(Clone, Finalize)]
struct TerminalServiceWrapper(Arc<TerminalService>);

unsafe impl Trace for TerminalServiceWrapper {
    unsafe fn trace(&self, _tracer: &mut boa_engine::gc::Tracer) {}
    unsafe fn trace_non_roots(&self) {}
    fn run_finalizer(&self) { Finalize::finalize(self) }
}

pub fn create_fs_api(context: &mut Context, service: Arc<TerminalService>) -> JsValue {
    let service_clone = service.clone();

    // fs.readDir(path)
    let read_dir = NativeFunction::from_copy_closure_with_captures(
        move |_this, args, captures, ctx| {
            let service = &captures.0;
            let path_arg = args.get(0).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or_else(|| "".to_string());
            
            let handle = tokio::runtime::Handle::current();
            let result: Result<Vec<(String, String)>, String> = handle.block_on(async {
                let resolved = service.resolve_path(&path_arg);
                
                match service.storage_service.list(
                    &service.current_user,
                    DocsListQuery {
                        path: Some(resolved),
                        limit: None,
                        offset: None,
                    }
                ).await {
                    Ok(resp) => Ok(resp.entries.into_iter().map(|f| (f.name, if f.is_dir { "dir".to_string() } else { "file".to_string() })).collect()),
                    Err(e) => Err(e.to_string())
                }
            });

            match result {
                Ok(files) => {
                    let mut js_files = Vec::new();
                    for (name, type_) in files {
                        let obj = ObjectInitializer::new(ctx)
                            .property(JsString::from("name"), JsString::from(name), Attribute::all())
                            .property(JsString::from("type"), JsString::from(type_), Attribute::all())
                            .build();
                        js_files.push(JsValue::from(obj));
                    }
                    Ok(JsValue::from(boa_engine::object::builtins::JsArray::from_iter(js_files, ctx)))
                },
                Err(e) => Err(JsError::from_opaque(JsValue::from(JsString::from(e))))
            }
        },
        TerminalServiceWrapper(service_clone.clone())
    );

    // fs.readFile(path)
    let read_file = NativeFunction::from_copy_closure_with_captures(
        move |_this, args, captures, _ctx| {
            let service = &captures.0;
            let path_arg = args.get(0).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or("".to_string());
            
            let handle = tokio::runtime::Handle::current();
            let result: Result<String, String> = handle.block_on(async {
                let resolved = service.resolve_path(&path_arg);
                match service.storage_service.get_file_path(&service.current_user, &resolved).await {
                    Ok(p) => match tokio::fs::read_to_string(&p).await {
                        Ok(c) => Ok(c),
                        Err(e) => Err(e.to_string())
                    },
                    Err(e) => Err(e.to_string())
                }
            });

            match result {
                Ok(content) => Ok(JsValue::from(JsString::from(content))),
                Err(e) => Err(JsError::from_opaque(JsValue::from(JsString::from(e))))
            }
        },
        TerminalServiceWrapper(service_clone.clone())
    );
    
    // fs.writeFile(path, content, append)
    let write_file = NativeFunction::from_copy_closure_with_captures(
        move |_this, args, captures, _ctx| {
            let service = &captures.0;
            let path_arg = args.get(0).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or("".to_string());
            let content_arg = args.get(1).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or("".to_string());
            let append = args.get(2).and_then(|v| v.as_boolean()).unwrap_or(false);
            
            let handle = tokio::runtime::Handle::current();
            let result: Result<(), String> = handle.block_on(async {
                let resolved = service.resolve_path(&path_arg);
                let p = std::path::Path::new(&resolved);
                let parent_path = p.parent().unwrap_or(std::path::Path::new("/")).to_string_lossy().to_string();
                let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
                
                // Create temp file
                let temp_dir = std::env::temp_dir();
                let temp_file_path = temp_dir.join(format!("kernel_write_{}_{}", service.current_user, uuid::Uuid::new_v4()));
                
                if append {
                     // Try to locate existing file
                     if let Ok(existing_path) = service.storage_service.get_file_path(&service.current_user, &resolved).await {
                         // Copy existing to temp (streaming copy, O(1) memory)
                         if let Err(e) = tokio::fs::copy(&existing_path, &temp_file_path).await {
                             // If copy fails, maybe file doesn't exist or other error.
                             // If append is true but file doesn't exist, we just create new.
                             // But if copy failed for other reasons (perm), we might want to error?
                             // For now, assume if copy fails, we treat as new file if it was not found.
                             // But tokio::fs::copy returns error if from doesn't exist.
                             // We should check error kind?
                             // get_file_path returning Ok means it *should* exist.
                             return Err(format!("Failed to copy existing file: {}", e));
                         }
                     }
                }
                
                // Append/Write content
                // Open for append if it exists (copied), or create if new
                let mut file = tokio::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .append(true) 
                    .open(&temp_file_path)
                    .await
                    .map_err(|e| e.to_string())?;
                    
                use tokio::io::AsyncWriteExt;
                if let Err(e) = file.write_all(content_arg.as_bytes()).await {
                    return Err(format!("Failed to write content: {}", e));
                }
                if let Err(e) = file.sync_all().await {
                     return Err(format!("Failed to sync content: {}", e));
                }
                drop(file); // Close file before saving

                match service.storage_service.save_file_from_path(&service.current_user, &parent_path, &name, &temp_file_path).await {
                    Ok(_) => Ok(()),
                    Err(e) => {
                        let _ = tokio::fs::remove_file(&temp_file_path).await;
                        Err(e.to_string())
                    }
                }
            });

            match result {
                Ok(_) => Ok(JsValue::Undefined),
                Err(e) => Err(JsError::from_opaque(JsValue::from(JsString::from(e))))
            }
        },
        TerminalServiceWrapper(service_clone.clone())
    );

    // fs.mkdir(path)
    let mkdir = NativeFunction::from_copy_closure_with_captures(
        move |_this, args, captures, _ctx| {
            let service = &captures.0;
            let path_arg = args.get(0).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or("".to_string());
            
            let handle = tokio::runtime::Handle::current();
            let result: Result<(), String> = handle.block_on(async {
                let resolved = service.resolve_path(&path_arg);
                match service.storage_service.mkdir(&service.current_user, DocsMkdirReq { path: resolved }).await {
                    Ok(_) => Ok(()),
                    Err(e) => Err(e.to_string())
                }
            });

            match result {
                Ok(_) => Ok(JsValue::Undefined),
                Err(e) => Err(JsError::from_opaque(JsValue::from(JsString::from(e))))
            }
        },
        TerminalServiceWrapper(service_clone.clone())
    );

    // fs.delete(path)
    let delete_file = NativeFunction::from_copy_closure_with_captures(
        move |_this, args, captures, _ctx| {
            let service = &captures.0;
            let path_arg = args.get(0).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or("".to_string());
            
            let handle = tokio::runtime::Handle::current();
            let result: Result<(), String> = handle.block_on(async {
                let resolved = service.resolve_path(&path_arg);
                match service.storage_service.delete(&service.current_user, DocsDeleteQuery { path: Some(resolved) }).await {
                    Ok(_) => Ok(()),
                    Err(e) => Err(e.to_string())
                }
            });

            match result {
                Ok(_) => Ok(JsValue::Undefined),
                Err(e) => Err(JsError::from_opaque(JsValue::from(JsString::from(e))))
            }
        },
        TerminalServiceWrapper(service_clone.clone())
    );

    // fs.rename(from, to)
    let rename_file = NativeFunction::from_copy_closure_with_captures(
        move |_this, args, captures, _ctx| {
            let service = &captures.0;
            let from_arg = args.get(0).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or("".to_string());
            let to_arg = args.get(1).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or("".to_string());
            
            let handle = tokio::runtime::Handle::current();
            let result: Result<(), String> = handle.block_on(async {
                let resolved_from = service.resolve_path(&from_arg);
                let resolved_to = service.resolve_path(&to_arg);
                
                match service.storage_service.rename(&service.current_user, DocsRenameReq { from: Some(resolved_from), to: Some(resolved_to) }).await {
                    Ok(_) => Ok(()),
                    Err(e) => Err(e.to_string())
                }
            });

            match result {
                Ok(_) => Ok(JsValue::Undefined),
                Err(e) => Err(JsError::from_opaque(JsValue::from(JsString::from(e))))
            }
        },
        TerminalServiceWrapper(service_clone.clone())
    );

    let obj = ObjectInitializer::new(context)
        .function(read_dir, JsString::from("readDir"), 1)
        .function(read_file, JsString::from("readFile"), 1)
        .function(write_file, JsString::from("writeFile"), 3)
        .function(mkdir, JsString::from("mkdir"), 1)
        .function(delete_file, JsString::from("delete"), 1)
        .function(rename_file, JsString::from("rename"), 2)
        .build();
        
    JsValue::from(obj)
}
