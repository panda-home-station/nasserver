use std::sync::Arc;
use boa_engine::{Context, JsValue, NativeFunction, JsString, Finalize, Trace, JsError};
use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use domain::container::ContainerService;

#[derive(Clone, Finalize)]
struct ContainerServiceWrapper(Arc<dyn ContainerService>);

unsafe impl Trace for ContainerServiceWrapper {
    unsafe fn trace(&self, _tracer: &mut boa_engine::gc::Tracer) {}
    unsafe fn trace_non_roots(&self) {}
    fn run_finalizer(&self) { Finalize::finalize(self) }
}

pub fn create_docker_api(context: &mut Context, service: Arc<dyn ContainerService>) -> JsValue {
    let service_clone = service.clone();

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
        ContainerServiceWrapper(service_clone)
    );

    let obj = ObjectInitializer::new(context)
        .function(ps_func, JsString::from("ps"), 0)
        .build();
        
    JsValue::from(obj)
}
