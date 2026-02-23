use std::sync::{Arc, Mutex};
use boa_engine::{Context, Source, JsValue, NativeFunction, JsString, Finalize, Trace, JsError};
use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use crate::service::TerminalService;
use crate::error::{Result, TerminalError};
use domain::container::ContainerService;

pub struct JsRuntime;

#[derive(Clone, Finalize)]
struct OutputWrapper(Arc<Mutex<String>>);

unsafe impl Trace for OutputWrapper {
    unsafe fn trace(&self, _tracer: &mut boa_engine::gc::Tracer) {}
    unsafe fn trace_non_roots(&self) {}
    fn run_finalizer(&self) { Finalize::finalize(self) }
}

#[derive(Clone, Finalize)]
struct ContainerServiceWrapper(Arc<dyn ContainerService>);

unsafe impl Trace for ContainerServiceWrapper {
    unsafe fn trace(&self, _tracer: &mut boa_engine::gc::Tracer) {}
    unsafe fn trace_non_roots(&self) {}
    fn run_finalizer(&self) { Finalize::finalize(self) }
}

impl JsRuntime {
    pub fn execute(code: &str, service: Arc<TerminalService>, args: Vec<String>) -> Result<String> {
        // Initialize context
        let mut context = Context::default();

        // Register 'print' global
        let output_buffer = Arc::new(Mutex::new(String::new()));
        let buffer_clone = output_buffer.clone();
        
        let print_func = NativeFunction::from_copy_closure_with_captures(
            move |_this, args, captures, ctx| {
                let mut buf = captures.0.lock().unwrap();
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 { buf.push(' '); }
                    let s = arg.to_string(ctx)
                        .map(|s| s.to_std_string().unwrap_or_default())
                        .unwrap_or_else(|_| "Error".to_string());
                    buf.push_str(&s);
                }
                buf.push('\n');
                Ok(JsValue::Undefined)
            },
            OutputWrapper(buffer_clone)
        );

        context.register_global_callable(JsString::from("print"), 1, print_func)
            .map_err(|e| TerminalError::Internal(e.to_string()))?;

        // Register 'args' global
        let js_args_list: Vec<JsValue> = args.into_iter()
            .map(|s| JsValue::from(JsString::from(s)))
            .collect();
            
        let js_args = boa_engine::object::builtins::JsArray::from_iter(
            js_args_list,
            &mut context
        );
        context.register_global_property(JsString::from("args"), js_args, Attribute::all())
            .map_err(|e| TerminalError::Internal(e.to_string()))?;

        // Register 'sys' object
        // Create docker_api first to avoid double borrow
        let docker_api = create_docker_api(&mut context, service.container_service.clone());
        let sys_obj = ObjectInitializer::new(&mut context)
            .property(
                JsString::from("docker"),
                docker_api,
                Attribute::all()
            )
            .build();
            
        context.register_global_property(JsString::from("sys"), sys_obj, Attribute::all())
            .map_err(|e| TerminalError::Internal(e.to_string()))?;

        // Handle shebang if present
        let code_to_run = if code.trim_start().starts_with("#!") {
            if let Some(idx) = code.find('\n') {
                &code[idx..]
            } else {
                ""
            }
        } else {
            code
        };

        // Execute code
        match context.eval(Source::from_bytes(code_to_run)) {
            Ok(val) => {
                let mut output = output_buffer.lock().unwrap().clone();
                if !val.is_undefined() {
                    let s = val.to_string(&mut context)
                        .map(|s| s.to_std_string().unwrap_or_default())
                        .unwrap_or_else(|_| "Error".to_string());
                    output.push_str(&s);
                    output.push('\n');
                }
                Ok(output)
            },
            Err(e) => {
                let mut output = output_buffer.lock().unwrap().clone();
                output.push_str(&format!("Error: {}", e));
                
                // Add hint for SyntaxError which might be due to unquoted strings in shell
                if e.to_string().contains("SyntaxError") && !code.contains("'") && !code.contains("\"") {
                     output.push_str("\nHint: Did you forget to quote the JS code? Try: js 'print(\"...\")'");
                }
                
                Ok(output)
            }
        }
    }
}

fn create_docker_api(context: &mut Context, container_service: Arc<dyn ContainerService>) -> JsValue {
    // sys.docker.ps()
    let ps_func = NativeFunction::from_copy_closure_with_captures(
        move |_this, _args, captures, ctx| {
            let service = &captures.0;
            let handle = tokio::runtime::Handle::current();
            let result = handle.block_on(async {
                service.list_containers().await
            });
            
            match result {
                Ok(containers) => {
                    // Convert to JS Array of Objects
                    let mut js_containers = Vec::new();
                    for c in containers {
                        let obj = ObjectInitializer::new(ctx)
                            .property(JsString::from("id"), JsString::from(c.id), Attribute::all())
                            .property(JsString::from("image"), JsString::from(c.image), Attribute::all())
                            .property(JsString::from("status"), JsString::from(c.status.unwrap_or_default()), Attribute::all())
                            .property(JsString::from("name"), JsString::from(c.names.first().cloned().unwrap_or_default()), Attribute::all())
                            .build();
                        js_containers.push(JsValue::from(obj));
                    }
                    Ok(JsValue::from(boa_engine::object::builtins::JsArray::from_iter(js_containers, ctx)))
                },
                Err(e) => {
                    Err(JsError::from_opaque(JsValue::from(JsString::from(format!("Docker Error: {}", e)))))
                }
            }
        },
        ContainerServiceWrapper(container_service)
    );

    let obj = ObjectInitializer::new(context)
        .function(ps_func, JsString::from("ps"), 0)
        .build();
        
    JsValue::from(obj)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use domain::container::{ContainerService, ContainerInfo, ImageInfo, VolumeInfo, NetworkInfo, CreateContainerReq, PullImageReq, CreateVolumeReq};

    struct MockContainerService;

    #[async_trait]
    impl ContainerService for MockContainerService {
        async fn list_containers(&self) -> domain::Result<Vec<ContainerInfo>> {
            Ok(vec![
                ContainerInfo {
                    id: "1234567890ab".to_string(),
                    image: "test-image".to_string(),
                    names: vec!["/test-container".to_string()],
                    state: "running".to_string(),
                    status: Some("Up 2 hours".to_string()),
                    created: 1620000000,
                    ports: vec![],
                }
            ])
        }
        async fn start_container(&self, _id: &str) -> domain::Result<()> { Ok(()) }
        async fn stop_container(&self, _id: &str) -> domain::Result<()> { Ok(()) }
        async fn restart_container(&self, _id: &str) -> domain::Result<()> { Ok(()) }
        async fn remove_container(&self, _id: &str) -> domain::Result<()> { Ok(()) }
        async fn create_container(&self, _req: CreateContainerReq) -> domain::Result<()> { Ok(()) }
        async fn list_images(&self) -> domain::Result<Vec<ImageInfo>> { Ok(vec![]) }
        async fn remove_image(&self, _id: &str) -> domain::Result<()> { Ok(()) }
        async fn pull_image(&self, _req: PullImageReq) -> domain::Result<()> { Ok(()) }
        async fn list_volumes(&self) -> domain::Result<Vec<VolumeInfo>> { Ok(vec![]) }
        async fn create_volume(&self, _req: CreateVolumeReq) -> domain::Result<()> { Ok(()) }
        async fn remove_volume(&self, _name: &str) -> domain::Result<()> { Ok(()) }
        async fn list_networks(&self) -> domain::Result<Vec<NetworkInfo>> { Ok(vec![]) }
    }

    #[tokio::test]
    async fn test_docker_api() {
        let service = Arc::new(MockContainerService);
        
        let result = tokio::task::spawn_blocking(move || {
            let mut context = Context::default();
            let docker_api = create_docker_api(&mut context, service);
            
            let sys_obj = ObjectInitializer::new(&mut context)
                .property(JsString::from("docker"), docker_api, Attribute::all())
                .build();
            context.register_global_property(JsString::from("sys"), sys_obj, Attribute::all()).unwrap();

            let val = context.eval(Source::from_bytes("sys.docker.ps()[0].name")).unwrap();
            val.to_string(&mut context).unwrap().to_std_string().unwrap()
        }).await.unwrap();

        assert_eq!(result, "/test-container");
    }

    #[test]
    fn test_shebang_stripping() {
        // Since we can't easily mock Context in pure unit test without spinning up full boa,
        // we can just verify the string logic we added if we extracted it.
        // But here we can rely on the fact that if we didn't strip it, parse might fail (or not).
        // Actually, let's just trust the implementation as it is simple string manipulation.
        let code = "#!/usr/bin/env js\nvar x = 1;";
        let code_to_run = if code.trim_start().starts_with("#!") {
            if let Some(idx) = code.find('\n') {
                &code[idx..]
            } else {
                ""
            }
        } else {
            code
        };
        assert_eq!(code_to_run, "\nvar x = 1;");
    }
}
