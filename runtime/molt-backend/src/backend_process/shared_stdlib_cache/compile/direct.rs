use std::io;
use std::path::Path;

use molt_backend::{SimpleBackend, SimpleIR};

pub(super) fn compile_direct_stdlib_cache_object(
    stdlib_path: &Path,
    functions: Vec<molt_backend::FunctionIR>,
    profile: Option<molt_backend::PgoProfileIR>,
    target_triple: Option<&str>,
    module_context: molt_backend::NativeBackendModuleContext,
) -> io::Result<()> {
    let stdlib_ir = SimpleIR { functions, profile };
    let mut stdlib_backend = SimpleBackend::new_with_target(target_triple);
    stdlib_backend.skip_ir_passes = true;
    stdlib_backend.skip_shared_stdlib_partition = true;
    // The stdlib cache object is not the main application object; the per-app
    // resolver is emitted once, into the main object.
    stdlib_backend.emit_app_callable_resolver = false;
    stdlib_backend.set_module_context(module_context);
    let stdlib_output = stdlib_backend.compile(stdlib_ir);
    std::fs::write(stdlib_path, &stdlib_output.bytes)
}
