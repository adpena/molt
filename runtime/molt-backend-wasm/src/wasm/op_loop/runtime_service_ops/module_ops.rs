use super::RuntimeServiceOpContext;
use super::call_emit::{RuntimeServiceArg::Local, RuntimeServiceCall, emit_runtime_service_call};
use crate::OpIR;
use wasm_encoder::Function;

pub(super) fn emit_module_runtime_op(
    context: &RuntimeServiceOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) -> bool {
    if let Some(call) = module_runtime_call(op.kind.as_str()) {
        emit_runtime_service_call(context, func, op, call);
        return true;
    }
    false
}

fn module_runtime_call(kind: &str) -> Option<RuntimeServiceCall<'static>> {
    Some(match kind {
        "module_new" => RuntimeServiceCall::result("module_new", &[Local(0)]),
        "module_cache_get" => RuntimeServiceCall::result("module_cache_get", &[Local(0)]),
        "module_import" => RuntimeServiceCall::result("module_import", &[Local(0)]),
        "module_cache_set" => {
            RuntimeServiceCall::non_none("module_cache_set", &[Local(0), Local(1)])
        }
        "module_cache_del" => RuntimeServiceCall::non_none("module_cache_del", &[Local(0)]),
        "module_get_attr" => RuntimeServiceCall::result("module_get_attr", &[Local(0), Local(1)]),
        // `from M import name` uses CPython IMPORT_FROM semantics
        // (ImportError on miss + sys.modules submodule fallback);
        // plain `M.name` raises AttributeError.
        "module_import_from" => {
            RuntimeServiceCall::result("module_import_from", &[Local(0), Local(1)])
        }
        "module_get_global" => {
            RuntimeServiceCall::result("module_get_global", &[Local(0), Local(1)])
        }
        "module_del_global" => {
            RuntimeServiceCall::non_none("module_del_global", &[Local(0), Local(1)])
        }
        "module_del_global_if_present" => {
            RuntimeServiceCall::non_none("module_del_global_if_present", &[Local(0), Local(1)])
        }
        "module_get_name" => RuntimeServiceCall::result("module_get_name", &[Local(0), Local(1)]),
        "module_set_attr" => {
            RuntimeServiceCall::non_none("module_set_attr", &[Local(0), Local(1), Local(2)])
        }
        "module_import_star" => {
            RuntimeServiceCall::result("module_import_star", &[Local(0), Local(1)])
        }
        _ => return None,
    })
}
