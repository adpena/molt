use std::{io, path::Path};

use molt_backend::SimpleIR;

use super::super::admit_or_invalidate_shared_stdlib_cache;
use super::request::NativeStdlibCachePrepare;

pub(crate) fn try_reuse_existing_stdlib_cache(
    ir: &mut SimpleIR,
    stdlib_path: &Path,
    request: &NativeStdlibCachePrepare<'_>,
    current_partition_manifest: &str,
    user_remaining: &mut Vec<molt_backend::FunctionIR>,
    stdlib_funcs: &mut Vec<molt_backend::FunctionIR>,
) -> io::Result<bool> {
    if !request.can_reuse_stdlib_cache() {
        return Ok(false);
    }
    let admitted = admit_or_invalidate_shared_stdlib_cache(
        stdlib_path,
        request.expected_cache_key,
        request.expected_cache_manifest,
        current_partition_manifest,
        stdlib_funcs.len(),
        request.log_prefix,
    )?;
    if !admitted {
        return Ok(false);
    }
    let mut retained = std::mem::take(user_remaining);
    // The user document already holds an extern row for each stdlib function
    // it references; the rest are appended as rows nothing names yet.
    let present: std::collections::BTreeSet<String> =
        retained.iter().map(|func| func.name.clone()).collect();
    let user_count = retained.iter().filter(|func| !func.is_extern).count();
    let mut extern_count = retained.len() - user_count;
    for mut func in std::mem::take(stdlib_funcs) {
        if func.is_extern || present.contains(&func.name) {
            continue;
        }
        molt_backend::externalize_function_with_signature(&mut func);
        extern_count += 1;
        retained.push(func);
    }
    ir.functions = retained;
    eprintln!(
        "{}: incremental -- compiling {user_count} user functions \
         ({extern_count} stdlib extern from {})",
        request.log_prefix,
        stdlib_path.display()
    );
    Ok(true)
}
