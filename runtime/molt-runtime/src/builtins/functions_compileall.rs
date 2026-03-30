// Compileall stdlib implementation.
// Extracted from functions.rs for tree shaking.

use crate::*;
use molt_obj_model::MoltObject;
use super::functions::*;


pub(crate) fn compileall_compile_file_impl(fullname: &str) -> bool {
    let mut handle = match fs::File::open(fullname) {
        Ok(handle) => handle,
        Err(_) => return false,
    };
    let mut one = [0u8; 1];
    handle.read(&mut one).is_ok()
}


pub(crate) fn compileall_compile_dir_impl(dir: &str, maxlevels: i64) -> bool {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return false,
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let mut success = true;
    for entry in names {
        if entry == "__pycache__" {
            continue;
        }
        let full = pkgutil_join(dir, &entry);
        if entry.ends_with(".py") {
            if !compileall_compile_file_impl(&full) {
                success = false;
            }
            continue;
        }
        if maxlevels <= 0 {
            continue;
        }
        if fs::read_dir(&full).is_err() {
            continue;
        }
        if !compileall_compile_dir_impl(&full, maxlevels - 1) {
            success = false;
        }
    }
    success
}


#[unsafe(no_mangle)]
pub extern "C" fn molt_compileall_compile_file(fullname_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let allowed = crate::has_capability(_py, "fs.read");
        audit_capability_decision(
            "compileall.compile.file",
            "fs.read",
            AuditArgs::None,
            allowed,
        );
        if !allowed {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let Some(fullname) = string_obj_to_owned(obj_from_bits(fullname_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "fullname must be str");
        };
        MoltObject::from_bool(compileall_compile_file_impl(&fullname)).bits()
    })
}


#[unsafe(no_mangle)]
pub extern "C" fn molt_compileall_compile_dir(dir_bits: u64, maxlevels_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let allowed = crate::has_capability(_py, "fs.read");
        audit_capability_decision(
            "compileall.compile.dir",
            "fs.read",
            AuditArgs::None,
            allowed,
        );
        if !allowed {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let Some(dir) = string_obj_to_owned(obj_from_bits(dir_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "dir must be str");
        };
        let Some(maxlevels) = to_i64(obj_from_bits(maxlevels_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "maxlevels must be int");
        };
        MoltObject::from_bool(compileall_compile_dir_impl(&dir, maxlevels)).bits()
    })
}


#[unsafe(no_mangle)]
pub extern "C" fn molt_compileall_compile_path(
    paths_bits: u64,

