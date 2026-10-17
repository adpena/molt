use super::*;

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_find_in_path_package_context(
    fullname_bits: u64,
    search_paths_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        importlib_find_in_path_payload(_py, fullname_bits, search_paths_bits, true)
    })
}

/// Apply native-extension metadata and capability admission without owning the
/// Python finder protocol. Ordinary in-memory specs require no filesystem work.
#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_admit_spec(fullname_bits: u64, spec_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let fullname = match string_arg_from_bits(py, fullname_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match importlib_enforce_extension_spec_object_boundary(py, &fullname, spec_bits) {
            Ok(()) => MoltObject::none().bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_filefinder_find_spec(
    fullname_bits: u64,
    path_bits: u64,
    machinery_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let fullname = match string_arg_from_bits(py, fullname_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let path = match string_arg_from_bits(py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if let Err(error) = crate::require_operation::<u64>(
            py,
            crate::OperationId::ImportlibFilefinderFindSpec,
            AuditArgs::None,
        ) {
            return error;
        }
        let resolution = match importlib_resolve_spec_in_paths(py, &fullname, &[path], true) {
            Ok(Some(resolution)) => resolution,
            Ok(None) => return MoltObject::none().bits(),
            Err(bits) => return bits,
        };
        match importlib_find_spec_object_bits(py, &fullname, &resolution, machinery_bits) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}
