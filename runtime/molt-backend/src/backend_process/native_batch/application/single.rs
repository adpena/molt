use std::io;
use std::path::Path;

use molt_backend::{SimpleBackend, SimpleIR};

use super::super::{NativeApplicationObjectOptions, NativeApplicationObjectResult};
use crate::backend_process::io_limits::write_output_path;

pub(crate) fn compile_single_native_application_object_to_path(
    ir: SimpleIR,
    output_path: &Path,
    mut options: NativeApplicationObjectOptions<'_>,
    function_count: usize,
) -> io::Result<NativeApplicationObjectResult> {
    let mut backend = SimpleBackend::new_with_target(options.target_triple);
    if options.stdlib_split_enabled {
        backend.skip_shared_stdlib_partition = true;
    }
    backend.app_callable_manifest = options.app_callable_manifest.take();
    backend.module_registry = options.module_registry.take();
    if let Some(module_context) = options.module_context.take() {
        backend.set_module_context(module_context);
    }
    let obj_output = backend.compile(ir);
    write_output_path(output_path, &obj_output.bytes)?;
    eprintln!(
        "Successfully compiled to {} ({} functions)",
        output_path.display(),
        function_count
    );
    Ok(NativeApplicationObjectResult {
        function_count,
        batch_count: 1,
    })
}
