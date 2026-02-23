use std::sync::Arc;
use boa_engine::{Context, JsValue, NativeFunction, JsString, Finalize, Trace, JsError};
use boa_engine::object::ObjectInitializer;
use crate::service::TerminalService;

#[derive(Clone, Finalize)]
struct TerminalServiceWrapper(Arc<TerminalService>);

unsafe impl Trace for TerminalServiceWrapper {
    unsafe fn trace(&self, _tracer: &mut boa_engine::gc::Tracer) {}
    unsafe fn trace_non_roots(&self) {}
    fn run_finalizer(&self) { Finalize::finalize(self) }
}

pub fn create_system_api(context: &mut Context, service: Arc<TerminalService>) -> JsValue {
    let service_clone = service.clone();

    // sys.system.stats()
    let stats = NativeFunction::from_copy_closure_with_captures(
        move |_this, _args, captures, _ctx| {
            let service = &captures.0;
            let handle = tokio::runtime::Handle::current();
            let result = handle.block_on(async {
                service.system_service.get_current_stats().await
            });

            match result {
                Ok(stats) => {
                    match serde_json::to_string(&stats) {
                        Ok(s) => Ok(JsValue::from(JsString::from(s))),
                        Err(e) => Err(JsError::from_opaque(JsValue::from(JsString::from(e.to_string()))))
                    }
                },
                Err(e) => Err(JsError::from_opaque(JsValue::from(JsString::from(e.to_string()))))
            }
        },
        TerminalServiceWrapper(service_clone.clone())
    );

    // sys.system.health()
    let health = NativeFunction::from_copy_closure_with_captures(
        move |_this, _args, captures, _ctx| {
            let service = &captures.0;
            let handle = tokio::runtime::Handle::current();
            let result = handle.block_on(async {
                service.system_service.health().await
            });

            match result {
                Ok(h) => {
                    match serde_json::to_string(&h) {
                        Ok(s) => Ok(JsValue::from(JsString::from(s))),
                        Err(e) => Err(JsError::from_opaque(JsValue::from(JsString::from(e.to_string()))))
                    }
                },
                Err(e) => Err(JsError::from_opaque(JsValue::from(JsString::from(e.to_string()))))
            }
        },
        TerminalServiceWrapper(service_clone.clone())
    );

    // sys.system.device()
    let device = NativeFunction::from_copy_closure_with_captures(
        move |_this, _args, captures, _ctx| {
            let service = &captures.0;
            let handle = tokio::runtime::Handle::current();
            let result = handle.block_on(async {
                service.system_service.get_device_info().await
            });

            match result {
                Ok(info) => {
                    match serde_json::to_string(&info) {
                        Ok(s) => Ok(JsValue::from(JsString::from(s))),
                        Err(e) => Err(JsError::from_opaque(JsValue::from(JsString::from(e.to_string()))))
                    }
                },
                Err(e) => Err(JsError::from_opaque(JsValue::from(JsString::from(e.to_string()))))
            }
        },
        TerminalServiceWrapper(service_clone.clone())
    );

    // sys.system.gpu()
    let gpu = NativeFunction::from_copy_closure_with_captures(
        move |_this, _args, captures, _ctx| {
            let service = &captures.0;
            let handle = tokio::runtime::Handle::current();
            let result = handle.block_on(async {
                service.system_service.get_gpus().await
            });

            match serde_json::to_string(&result) {
                Ok(s) => Ok(JsValue::from(JsString::from(s))),
                Err(e) => Err(JsError::from_opaque(JsValue::from(JsString::from(e.to_string()))))
            }
        },
        TerminalServiceWrapper(service_clone.clone())
    );

    // sys.system.checkPorts(ports)
    let check_ports = NativeFunction::from_copy_closure_with_captures(
        move |_this, args, captures, ctx| {
            let service = &captures.0;
            let ports_arg = args.get(0).ok_or_else(|| JsError::from_opaque(JsValue::from(JsString::from("Missing ports argument"))))?;
            
            // Convert JS array to Vec<u16>
            let mut ports = Vec::new();
            if let Some(obj) = ports_arg.as_object() {
                if obj.is_array() {
                    let len = obj.get(JsString::from("length"), ctx)
                        .ok()
                        .and_then(|v| v.as_number())
                        .map(|n| n as u32)
                        .unwrap_or(0);
                    for i in 0..len {
                        if let Ok(val) = obj.get(i, ctx) {
                            if let Some(p) = val.as_number() {
                                ports.push(p as u16);
                            }
                        }
                    }
                }
            } else if let Some(s) = ports_arg.as_string() {
                if let Ok(p) = serde_json::from_str::<Vec<u16>>(&s.to_std_string_escaped()) {
                    ports = p;
                }
            }

            let handle = tokio::runtime::Handle::current();
            let result = handle.block_on(async {
                service.system_service.check_ports(ports).await
            });

            match result {
                Ok(statuses) => {
                    match serde_json::to_string(&statuses) {
                        Ok(s) => Ok(JsValue::from(JsString::from(s))),
                        Err(e) => Err(JsError::from_opaque(JsValue::from(JsString::from(e.to_string()))))
                    }
                },
                Err(e) => Err(JsError::from_opaque(JsValue::from(JsString::from(e.to_string()))))
            }
        },
        TerminalServiceWrapper(service_clone.clone())
    );

    // sys.system.getDockerMirrors()
    let get_docker_mirrors = NativeFunction::from_copy_closure_with_captures(
        move |_this, _args, captures, _ctx| {
            let service = &captures.0;
            let handle = tokio::runtime::Handle::current();
            let result = handle.block_on(async {
                service.system_service.get_docker_mirrors().await
            });

            match result {
                Ok(mirrors) => {
                    match serde_json::to_string(&mirrors) {
                        Ok(s) => Ok(JsValue::from(JsString::from(s))),
                        Err(e) => Err(JsError::from_opaque(JsValue::from(JsString::from(e.to_string()))))
                    }
                },
                Err(e) => Err(JsError::from_opaque(JsValue::from(JsString::from(e.to_string()))))
            }
        },
        TerminalServiceWrapper(service_clone.clone())
    );

    // sys.system.setDockerMirrors(mirrors)
    let set_docker_mirrors = NativeFunction::from_copy_closure_with_captures(
        move |_this, args, captures, ctx| {
            let service = &captures.0;
            let mirrors_arg = args.get(0).ok_or_else(|| JsError::from_opaque(JsValue::from(JsString::from("Missing mirrors argument"))))?;
            
            // We expect a JS array of strings, or a JSON string?
            // Let's support JS array of strings.
            let mut mirrors = Vec::new();
            if let Some(obj) = mirrors_arg.as_object() {
                if obj.is_array() {
                    let len = obj.get(JsString::from("length"), ctx)
                        .ok()
                        .and_then(|v| v.as_number())
                        .map(|n| n as u32)
                        .unwrap_or(0);
                    for i in 0..len {
                        if let Ok(val) = obj.get(i, ctx) {
                            if let Some(s) = val.as_string() {
                                mirrors.push(serde_json::Value::String(s.to_std_string_escaped()));
                            }
                        }
                    }
                }
            } else if let Some(s) = mirrors_arg.as_string() {
                // Also support passing JSON string directly
                 match serde_json::from_str::<Vec<serde_json::Value>>(&s.to_std_string_escaped()) {
                    Ok(v) => mirrors = v,
                    Err(_) => {} // Ignore error, maybe empty
                 }
            }

            let handle = tokio::runtime::Handle::current();
            let result = handle.block_on(async {
                service.system_service.set_docker_mirrors(mirrors).await
            });

            match result {
                Ok(_) => Ok(JsValue::Undefined),
                Err(e) => Err(JsError::from_opaque(JsValue::from(JsString::from(e.to_string()))))
            }
        },
        TerminalServiceWrapper(service_clone.clone())
    );

    // sys.system.getDockerSettings()
    let get_docker_settings = NativeFunction::from_copy_closure_with_captures(
        move |_this, _args, captures, _ctx| {
            let service = &captures.0;
            let handle = tokio::runtime::Handle::current();
            let result = handle.block_on(async {
                service.system_service.get_docker_settings().await
            });

            match result {
                Ok(settings) => {
                    match serde_json::to_string(&settings) {
                        Ok(s) => Ok(JsValue::from(JsString::from(s))),
                        Err(e) => Err(JsError::from_opaque(JsValue::from(JsString::from(e.to_string()))))
                    }
                },
                Err(e) => Err(JsError::from_opaque(JsValue::from(JsString::from(e.to_string()))))
            }
        },
        TerminalServiceWrapper(service_clone.clone())
    );

    // sys.system.setDockerSettings(settings)
    let set_docker_settings = NativeFunction::from_copy_closure_with_captures(
        move |_this, args, captures, _ctx| {
            let service = &captures.0;
            let settings_arg = args.get(0).ok_or_else(|| JsError::from_opaque(JsValue::from(JsString::from("Missing settings argument"))))?;
            
            // Support JSON string
            let settings_json = if let Some(s) = settings_arg.as_string() {
                serde_json::from_str::<serde_json::Value>(&s.to_std_string_escaped())
                    .map_err(|e| JsError::from_opaque(JsValue::from(JsString::from(e.to_string()))))?
            } else {
                // Or maybe we can't easily convert JS Object to serde_json::Value with current boa binding utils in this context
                // So let's enforce JSON string for now
                return Err(JsError::from_opaque(JsValue::from(JsString::from("Settings must be a JSON string"))));
            };

            let handle = tokio::runtime::Handle::current();
            let result = handle.block_on(async {
                service.system_service.set_docker_settings(settings_json).await
            });

            match result {
                Ok(_) => Ok(JsValue::Undefined),
                Err(e) => Err(JsError::from_opaque(JsValue::from(JsString::from(e.to_string()))))
            }
        },
        TerminalServiceWrapper(service_clone.clone())
    );

    let obj = ObjectInitializer::new(context)
        .function(stats, JsString::from("stats"), 0)
        .function(health, JsString::from("health"), 0)
        .function(device, JsString::from("device"), 0)
        .function(gpu, JsString::from("gpu"), 0)
        .function(check_ports, JsString::from("checkPorts"), 1)
        .function(get_docker_mirrors, JsString::from("getDockerMirrors"), 0)
        .function(set_docker_mirrors, JsString::from("setDockerMirrors"), 1)
        .function(get_docker_settings, JsString::from("getDockerSettings"), 0)
        .function(set_docker_settings, JsString::from("setDockerSettings"), 1)
        .build();
        
    JsValue::from(obj)
}
