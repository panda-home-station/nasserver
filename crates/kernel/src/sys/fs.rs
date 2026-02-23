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
                
                // Root virtual listing
                if resolved == "/" {
                    return Ok(vec![
                        ("AppData".to_string(), "dir".to_string()),
                        ("User".to_string(), "dir".to_string()),
                        ("bin".to_string(), "dir".to_string()),
                    ]);
                }

                let (storage_path, username) = if resolved.starts_with("/AppData") {
                    (resolved.clone(), service.current_user.clone())
                } else if resolved.starts_with("/bin") {
                    (resolved.clone(), service.current_user.clone())
                } else if resolved.starts_with(&format!("/User/{}", service.current_user)) {
                    let rel = resolved.trim_start_matches(&format!("/User/{}", service.current_user));
                    let sp = if rel.is_empty() { "/".to_string() } else { rel.to_string() };
                    (sp, service.current_user.clone())
                } else {
                    return Err("Permission denied".to_string());
                };

                match service.storage_service.list(
                    &username,
                    DocsListQuery {
                        path: Some(storage_path),
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
                let (storage_path, username) = if resolved.starts_with(&format!("/User/{}", service.current_user)) {
                    let rel = resolved.trim_start_matches(&format!("/User/{}", service.current_user));
                    let sp = if rel.is_empty() { "/".to_string() } else { rel.to_string() };
                    (sp, service.current_user.clone())
                } else if resolved.starts_with("/bin") {
                     // Allow reading bin
                    (resolved.clone(), service.current_user.clone())
                } else {
                     return Err("Permission denied".to_string());
                };

                match service.storage_service.get_file_path(&username, &storage_path).await {
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
    
    // fs.writeFile(path, content)
    let write_file = NativeFunction::from_copy_closure_with_captures(
        move |_this, args, captures, _ctx| {
            let service = &captures.0;
            let path_arg = args.get(0).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or("".to_string());
            let content_arg = args.get(1).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or("".to_string());
            let append = args.get(2).and_then(|v| v.as_boolean()).unwrap_or(false);
            
            let handle = tokio::runtime::Handle::current();
            let result: Result<(), String> = handle.block_on(async {
                let resolved = service.resolve_path(&path_arg);
                let (storage_path, username) = if resolved.starts_with(&format!("/User/{}", service.current_user)) {
                    let rel = resolved.trim_start_matches(&format!("/User/{}", service.current_user));
                    let sp = if rel.is_empty() { "/".to_string() } else { rel.to_string() };
                    (sp, service.current_user.clone())
                } else {
                     return Err("Permission denied".to_string());
                };
                
                let p = std::path::Path::new(&storage_path);
                let parent_path = p.parent().unwrap_or(std::path::Path::new("/")).to_string_lossy().to_string();
                let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
                
                let data = if append {
                    // Read existing content if append is true
                     if let Ok(existing_path) = service.storage_service.get_file_path(&username, &storage_path).await {
                        if let Ok(mut existing_content) = tokio::fs::read(&existing_path).await {
                             existing_content.extend_from_slice(content_arg.as_bytes());
                             bytes::Bytes::from(existing_content)
                        } else {
                             bytes::Bytes::from(content_arg)
                        }
                    } else {
                        bytes::Bytes::from(content_arg)
                    }
                } else {
                    bytes::Bytes::from(content_arg)
                };

                match service.storage_service.save_file(&username, &parent_path, &name, data).await {
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

    // fs.mkdir(path)
    let mkdir = NativeFunction::from_copy_closure_with_captures(
        move |_this, args, captures, _ctx| {
            let service = &captures.0;
            let path_arg = args.get(0).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or("".to_string());
            
            let handle = tokio::runtime::Handle::current();
            let result: Result<(), String> = handle.block_on(async {
                let resolved = service.resolve_path(&path_arg);
                let (storage_path, username) = if resolved.starts_with(&format!("/User/{}", service.current_user)) {
                    let rel = resolved.trim_start_matches(&format!("/User/{}", service.current_user));
                    let sp = if rel.is_empty() { "/".to_string() } else { rel.to_string() };
                    (sp, service.current_user.clone())
                } else {
                     return Err("Permission denied".to_string());
                };

                match service.storage_service.mkdir(&username, DocsMkdirReq { path: storage_path }).await {
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
                let (storage_path, username) = if resolved.starts_with(&format!("/User/{}", service.current_user)) {
                    let rel = resolved.trim_start_matches(&format!("/User/{}", service.current_user));
                    let sp = if rel.is_empty() { "/".to_string() } else { rel.to_string() };
                    (sp, service.current_user.clone())
                } else {
                     return Err("Permission denied".to_string());
                };

                match service.storage_service.delete(&username, DocsDeleteQuery { path: Some(storage_path) }).await {
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
                
                let (from_path, username) = if resolved_from.starts_with(&format!("/User/{}", service.current_user)) {
                    let rel = resolved_from.trim_start_matches(&format!("/User/{}", service.current_user));
                    let sp = if rel.is_empty() { "/".to_string() } else { rel.to_string() };
                    (sp, service.current_user.clone())
                } else {
                     return Err("Permission denied (source)".to_string());
                };

                 let (to_path, _) = if resolved_to.starts_with(&format!("/User/{}", service.current_user)) {
                    let rel = resolved_to.trim_start_matches(&format!("/User/{}", service.current_user));
                    let sp = if rel.is_empty() { "/".to_string() } else { rel.to_string() };
                    (sp, service.current_user.clone())
                } else {
                     return Err("Permission denied (dest)".to_string());
                };

                match service.storage_service.rename(&username, DocsRenameReq { from: Some(from_path), to: Some(to_path) }).await {
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
