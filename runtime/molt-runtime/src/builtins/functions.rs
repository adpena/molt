use molt_obj_model::MoltObject;
use std::collections::HashSet;

use crate::{
    alloc_bound_method_obj, alloc_code_obj, alloc_function_obj, alloc_string, builtin_classes,
    dec_ref_bits, function_set_closure_bits, function_set_trampoline_ptr, inc_ref_bits,
    obj_from_bits, object_class_bits, object_set_class_bits, object_type_id, raise_exception,
    string_obj_to_owned, to_i64, TYPE_ID_FUNCTION, TYPE_ID_STRING, TYPE_ID_TUPLE,
};

struct CompileScope {
    indent: usize,
    assigned: HashSet<String>,
    globals: HashSet<String>,
    nonlocals: HashSet<String>,
    params: HashSet<String>,
}

impl CompileScope {
    fn new(indent: usize) -> Self {
        Self {
            indent,
            assigned: HashSet::new(),
            globals: HashSet::new(),
            nonlocals: HashSet::new(),
            params: HashSet::new(),
        }
    }
}

fn is_ident(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if first != '_' && !first.is_ascii_alphabetic() {
        return false;
    }
    chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn parse_name_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .filter_map(|part| {
            let name = part.trim();
            if is_ident(name) {
                Some(name.to_string())
            } else {
                None
            }
        })
        .collect()
}

fn parse_param_names(raw: &str) -> Vec<String> {
    raw.split(',')
        .filter_map(|part| {
            let mut name = part.trim();
            if name.is_empty() {
                return None;
            }
            name = name.trim_start_matches('*').trim();
            if name.is_empty() {
                return None;
            }
            if let Some((before, _)) = name.split_once('=') {
                name = before.trim();
            }
            if let Some((before, _)) = name.split_once(':') {
                name = before.trim();
            }
            if is_ident(name) {
                Some(name.to_string())
            } else {
                None
            }
        })
        .collect()
}

fn binding_in_outer(scopes: &[CompileScope], name: &str) -> bool {
    scopes
        .iter()
        .rev()
        .any(|scope| scope.assigned.contains(name) || scope.params.contains(name))
}

fn compile_check_nonlocal(source: &str) -> Option<String> {
    // TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P1, status:partial): replace with full parser-backed compile that returns executable code objects.
    let mut scopes = vec![CompileScope::new(0)];
    let mut pending_def_indent: Option<usize> = None;
    let mut pending_params: Vec<String> = Vec::new();
    for raw in source.lines() {
        let stripped = raw.trim_start();
        if stripped.is_empty() || stripped.starts_with('#') {
            continue;
        }
        let indent = raw.len().saturating_sub(stripped.len());
        if let Some(pending) = pending_def_indent {
            if indent > pending {
                let mut scope = CompileScope::new(indent);
                for name in pending_params.drain(..) {
                    scope.params.insert(name.clone());
                    scope.assigned.insert(name);
                }
                scopes.push(scope);
                pending_def_indent = None;
            }
        }
        while scopes.len() > 1 && indent < scopes.last().map(|s| s.indent).unwrap_or(0) {
            let scope = scopes.pop().unwrap();
            for name in scope.nonlocals {
                if !binding_in_outer(&scopes[1..], &name) {
                    return Some(format!("no binding for nonlocal '{name}' found"));
                }
            }
        }
        if stripped.starts_with("def ") || stripped.starts_with("async def ") {
            let def_line = stripped
                .strip_prefix("async def ")
                .or_else(|| stripped.strip_prefix("def "))
                .unwrap_or(stripped);
            let name = def_line.split('(').next().unwrap_or("").trim();
            if is_ident(name) {
                if let Some(scope) = scopes.last_mut() {
                    scope.assigned.insert(name.to_string());
                }
            }
            if let (Some(start), Some(end)) = (def_line.find('('), def_line.rfind(')')) {
                if end > start {
                    pending_params = parse_param_names(&def_line[start + 1..end]);
                }
            }
            pending_def_indent = Some(indent);
            continue;
        }
        if let Some(rest) = stripped.strip_prefix("global ") {
            for name in parse_name_list(rest) {
                let scope = scopes.last_mut().unwrap();
                if scope.nonlocals.contains(&name) {
                    return Some(format!("name '{name}' is nonlocal and global"));
                }
                scope.globals.insert(name);
            }
            continue;
        }
        if let Some(rest) = stripped.strip_prefix("nonlocal ") {
            for name in parse_name_list(rest) {
                let scope = scopes.last_mut().unwrap();
                if scope.globals.contains(&name) {
                    return Some(format!("name '{name}' is nonlocal and global"));
                }
                scope.nonlocals.insert(name);
            }
            continue;
        }
        if stripped.contains('=')
            && !stripped.contains("==")
            && !stripped.contains("!=")
            && !stripped.starts_with("return ")
            && !stripped.starts_with("yield ")
            && !stripped.starts_with("raise ")
            && !stripped.starts_with("assert ")
        {
            let lhs = stripped
                .split_once('=')
                .map(|(lhs, _)| lhs)
                .unwrap_or("")
                .trim();
            for name in parse_name_list(lhs) {
                if let Some(scope) = scopes.last_mut() {
                    scope.assigned.insert(name);
                }
            }
        }
    }
    while scopes.len() > 1 {
        let scope = scopes.pop().unwrap();
        for name in scope.nonlocals {
            if !binding_in_outer(&scopes[1..], &name) {
                return Some(format!("no binding for nonlocal '{name}' found"));
            }
        }
    }
    None
}

#[no_mangle]
pub extern "C" fn molt_compile_builtin(
    source_bits: u64,
    filename_bits: u64,
    mode_bits: u64,
    flags_bits: u64,
    dont_inherit_bits: u64,
    optimize_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let source = match string_obj_to_owned(obj_from_bits(source_bits)) {
            Some(val) => val,
            None => {
                return raise_exception::<_>(_py, "TypeError", "compile() arg 1 must be a string");
            }
        };
        if string_obj_to_owned(obj_from_bits(filename_bits)).is_none() {
            return raise_exception::<_>(_py, "TypeError", "compile() arg 2 must be a string");
        }
        let mode = match string_obj_to_owned(obj_from_bits(mode_bits)) {
            Some(val) => val,
            None => {
                return raise_exception::<_>(_py, "TypeError", "compile() arg 3 must be a string");
            }
        };
        if mode != "exec" && mode != "eval" && mode != "single" {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "compile() mode must be 'exec', 'eval' or 'single'",
            );
        }
        if to_i64(obj_from_bits(flags_bits)).is_none() {
            return raise_exception::<_>(_py, "TypeError", "compile() arg 4 must be int");
        }
        if to_i64(obj_from_bits(dont_inherit_bits)).is_none() {
            return raise_exception::<_>(_py, "TypeError", "compile() arg 5 must be int");
        }
        if to_i64(obj_from_bits(optimize_bits)).is_none() {
            return raise_exception::<_>(_py, "TypeError", "compile() arg 6 must be int");
        }
        if let Some(message) = compile_check_nonlocal(&source) {
            return raise_exception::<_>(_py, "SyntaxError", &message);
        }
        let name_ptr = alloc_string(_py, b"<module>");
        if name_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let code_ptr = alloc_code_obj(_py, filename_bits, name_bits, 1, MoltObject::none().bits());
        if code_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(code_ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_func_new(fn_ptr: u64, trampoline_ptr: u64, arity: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = alloc_function_obj(_py, fn_ptr, arity);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            unsafe {
                function_set_trampoline_ptr(ptr, trampoline_ptr);
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_func_new_builtin(fn_ptr: u64, trampoline_ptr: u64, arity: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = alloc_function_obj(_py, fn_ptr, arity);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            function_set_trampoline_ptr(ptr, trampoline_ptr);
            let builtin_bits = builtin_classes(_py).builtin_function_or_method;
            object_set_class_bits(_py, ptr, builtin_bits);
            inc_ref_bits(_py, builtin_bits);
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_func_new_closure(
    fn_ptr: u64,
    trampoline_ptr: u64,
    arity: u64,
    closure_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = alloc_function_obj(_py, fn_ptr, arity);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            function_set_closure_bits(_py, ptr, closure_bits);
            function_set_trampoline_ptr(ptr, trampoline_ptr);
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_function_set_builtin(func_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(func_ptr) = obj_from_bits(func_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected function");
        };
        unsafe {
            if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
                return raise_exception::<_>(_py, "TypeError", "expected function");
            }
            let builtin_bits = builtin_classes(_py).builtin_function_or_method;
            let old_bits = object_class_bits(func_ptr);
            if old_bits != builtin_bits {
                if old_bits != 0 {
                    dec_ref_bits(_py, old_bits);
                }
                object_set_class_bits(_py, func_ptr, builtin_bits);
                inc_ref_bits(_py, builtin_bits);
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_code_new(
    filename_bits: u64,
    name_bits: u64,
    firstlineno_bits: u64,
    linetable_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let filename_obj = obj_from_bits(filename_bits);
        let Some(filename_ptr) = filename_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "code filename must be str");
        };
        unsafe {
            if object_type_id(filename_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "code filename must be str");
            }
        }
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "code name must be str");
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "code name must be str");
            }
        }
        if !obj_from_bits(linetable_bits).is_none() {
            let Some(table_ptr) = obj_from_bits(linetable_bits).as_ptr() else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "code linetable must be tuple or None",
                );
            };
            unsafe {
                if object_type_id(table_ptr) != TYPE_ID_TUPLE {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "code linetable must be tuple or None",
                    );
                }
            }
        }
        let firstlineno = to_i64(obj_from_bits(firstlineno_bits)).unwrap_or(0);
        let ptr = alloc_code_obj(_py, filename_bits, name_bits, firstlineno, linetable_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_bound_method_new(func_bits: u64, self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let func_obj = obj_from_bits(func_bits);
        let Some(func_ptr) = func_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "bound method expects function object");
        };
        unsafe {
            if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "bound method expects function object",
                );
            }
        }
        let ptr = alloc_bound_method_obj(_py, func_bits, self_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

/// # Safety
/// `self_ptr` must point to a valid closure storage region and `offset` must be
/// within the allocated payload.
#[no_mangle]
pub unsafe extern "C" fn molt_closure_load(self_ptr: *mut u8, offset: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if self_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let slot = self_ptr.add(offset as usize) as *mut u64;
        let bits = *slot;
        inc_ref_bits(_py, bits);
        bits
    })
}

/// # Safety
/// `self_ptr` must point to a valid closure storage region and `offset` must be
/// within the allocated payload.
#[no_mangle]
pub unsafe extern "C" fn molt_closure_store(self_ptr: *mut u8, offset: u64, bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if self_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let slot = self_ptr.add(offset as usize) as *mut u64;
        let old_bits = *slot;
        dec_ref_bits(_py, old_bits);
        inc_ref_bits(_py, bits);
        *slot = bits;
        MoltObject::none().bits()
    })
}
