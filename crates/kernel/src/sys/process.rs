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

pub fn create_process_api(context: &mut Context, service: Arc<TerminalService>) -> JsValue {
    let service_clone = service.clone();

    // process.cwd()
    let cwd = NativeFunction::from_copy_closure_with_captures(
        move |_this, _args, captures, _ctx| {
            let service = &captures.0;
            let cwd = service.get_user_cwd();
            Ok(JsValue::from(JsString::from(cwd)))
        },
        TerminalServiceWrapper(service_clone.clone())
    );

    // process.chdir(path)
    let chdir = NativeFunction::from_copy_closure_with_captures(
        move |_this, args, captures, _ctx| {
            let service = &captures.0;
            let path_arg = args.get(0).and_then(|v| v.as_string()).map(|s| s.to_std_string_escaped()).unwrap_or("".to_string());
            
            let handle = tokio::runtime::Handle::current();
            let result: Result<String, String> = handle.block_on(async {
                let resolved = service.resolve_path(&path_arg);
                
                // TODO: Verify directory exists via storage_service
                
                let mut cwd_lock = service.user_cwd.lock().unwrap();
                *cwd_lock = resolved.clone();
                Ok(resolved)
            });

            match result {
                Ok(new_cwd) => Ok(JsValue::from(JsString::from(new_cwd))),
                Err(e) => Err(JsError::from_opaque(JsValue::from(JsString::from(e))))
            }
        },
        TerminalServiceWrapper(service_clone.clone())
    );

    let obj = ObjectInitializer::new(context)
        .function(cwd, JsString::from("cwd"), 0)
        .function(chdir, JsString::from("chdir"), 1)
        .build();
        
    JsValue::from(obj)
}
