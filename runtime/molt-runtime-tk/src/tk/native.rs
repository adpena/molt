#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::args::{clear_last_error, set_last_error};
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::callbacks::{
    invoke_filehandler_command, remove_after_events_for_tokens, tokens_for_after_command,
    unregister_after_command_token,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::commands::invoke_callback;
use super::state::TkAppState;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::state::{
    alloc_string_bits, app_mut_from_registry, app_tcl_error_locked, raise_tcl_error, tk_registry,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use super::tcl::{
    TCL_OK, TclApi, TclInterpreter, TclObj, TclObjHeader, TclObjKind, TclTypePtrs, TclWideInt,
    tcl_result_string,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use crate::bridge::{
    clear_exception, dec_ref_bits, decode_value_list, dict_order, exception_pending,
    format_obj_str, inc_ref_bits, int_from_obj, is_truthy, object_type_id, raise_exception_u64,
    string_obj_to_owned, to_f64, to_i64,
};
use molt_runtime_core::prelude::PyToken;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use molt_runtime_core::prelude::{
    GilReleaseGuard, MoltObject, obj_from_bits, rt_bytes_as_slice, rt_bytes_from, rt_int, rt_none,
    rt_str, rt_string_from, rt_string_from_bytes, rt_tuple,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use molt_runtime_core::type_ids::{TYPE_ID_BYTEARRAY, TYPE_ID_BYTES, TYPE_ID_DICT, TYPE_ID_STRING};
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use std::ffi::{CString, c_int, c_void};
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use std::ptr;
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
use std::sync::OnceLock;

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn option_use_tk(py: &PyToken, options_bits: u64) -> bool {
    let obj = obj_from_bits(options_bits);
    let Some(dict_ptr) = obj.as_ptr() else {
        return true;
    };
    if object_type_id(dict_ptr) != TYPE_ID_DICT {
        return true;
    }
    let entries = dict_order(dict_ptr);
    for pair in entries.chunks(2) {
        if pair.len() != 2 {
            continue;
        }
        let Some(key) = string_obj_to_owned(obj_from_bits(pair[0])) else {
            continue;
        };
        if key == "useTk" {
            return is_truthy(py, obj_from_bits(pair[1]));
        }
    }
    true
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn tcl_obj_from_bits(py: &PyToken, bits: u64) -> TclObj {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return TclObj::from("");
    }
    if let Some(i) = to_i64(obj) {
        return TclObj::from(i);
    }
    if let Some(f) = to_f64(obj) {
        return TclObj::from(f);
    }
    if let Some(s) = string_obj_to_owned(obj) {
        return TclObj::from(s);
    }
    // Handle tuples/lists as Tcl lists (e.g., font tuples like ("SF Pro Display", 18, "bold"))
    if let Some(elements) = decode_value_list(obj) {
        let tcl_elements: Vec<TclObj> = elements
            .iter()
            .map(|&elem_bits| tcl_obj_from_bits(py, elem_bits))
            .collect();
        return TclObj::new_list(tcl_elements);
    }
    // Use str() instead of repr() for widget objects and other types.
    // Widget.__str__ returns the Tcl widget path (e.g., ".!frame24"),
    // while repr() returns "<tkinter.Frame object .!frame24>" which Tcl rejects.
    // Clear pending exceptions first — they cause molt_str_from_obj to bail.
    if exception_pending(py) {
        clear_exception(py);
    }
    let str_bits = rt_str(bits);
    if !obj_from_bits(str_bits).is_none()
        && let Some(s) = string_obj_to_owned(obj_from_bits(str_bits))
    {
        dec_ref_bits(py, str_bits);
        return TclObj::from(s);
    }
    dec_ref_bits(py, str_bits);
    TclObj::from(format_obj_str(py, obj))
}

// ---------------------------------------------------------------------------
// Typed Tcl_Obj value bridge — wantobjects=1 parity (CPython _tkinter.c)
//
// AsObj (molt -> Tcl_Obj): build TYPED objects (wideInt/double/boolean/byteArray/
// list) instead of stringifying, so Tcl never re-parses "42" back into an int.
// FromObj (Tcl_Obj -> molt): dispatch on the result's typePtr and produce a
// native molt int/float/bool/bytes/tuple/str with no string round-trip.
//
// The returned Tcl_Obj from AsObj has refCount 0 (freshly allocated); the caller
// owns it via Tcl_IncrRefCount before handing it to Tcl_EvalObjv.
// ---------------------------------------------------------------------------

/// AsObj: allocate a typed `Tcl_Obj*` for a molt value. Runs with the GIL held
/// (touches the molt runtime). Returns a refCount-0 object, or `Err` on
/// allocation failure (caller cleans up already-allocated argv entries).
///
/// Observable arg semantics preserved from the prior string bridge: `None` maps
/// to an empty string object (Tk treats it as ""), and unknown objects fall back
/// to their `str()` (e.g. widget paths), never `repr()`.
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn tcl_obj_alloc_typed_from_bits(
    py: &PyToken,
    api: &'static TclApi,
    interp: *mut c_void,
    bits: u64,
) -> Result<*mut c_void, String> {
    let obj = obj_from_bits(bits);
    // None -> empty string (matches the prior bridge; CPython would reject None
    // in call args, but molt has long mapped it to "").
    if obj.is_none() {
        return alloc_string_obj(api, "");
    }
    // bool BEFORE int: bool is an int subclass, and CPython's AsObj checks it
    // first so True/False become boolean objects, not 1/0 ints.
    if obj.is_bool() {
        let b = obj.as_bool().unwrap_or(false);
        let o = unsafe { (api.new_boolean_obj)(b as c_int) };
        return non_null_obj(o, "Tcl_NewBooleanObj returned null");
    }
    if let Some(i) = obj.as_int() {
        let o = unsafe { (api.new_wide_int_obj)(i) };
        return non_null_obj(o, "Tcl_NewWideIntObj returned null");
    }
    if let Some(f) = obj.as_float() {
        let o = unsafe { (api.new_double_obj)(f) };
        return non_null_obj(o, "Tcl_NewDoubleObj returned null");
    }
    // Heap objects: dispatch on the concrete type id. We must NOT probe via
    // `string_obj_to_owned` here — its `molt_string_as_ptr` raises a TypeError on
    // non-strings, which would pollute the interpreter's exception state for
    // tuple/list/widget arguments.
    if let Some(ptr) = obj.as_ptr() {
        let type_id = object_type_id(ptr);
        // str -> Tcl string object.
        if type_id == TYPE_ID_STRING {
            if let Some(s) = string_obj_to_owned(obj) {
                return alloc_string_obj(api, &s);
            }
            return alloc_string_obj(api, "");
        }
        // bytes / bytearray -> Tcl byte array object.
        if (type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY)
            && let Some(slice) = rt_bytes_as_slice(bits)
        {
            let len = slice.len() as c_int;
            let o = unsafe { (api.new_byte_array_obj)(slice.as_ptr(), len) };
            return non_null_obj(o, "Tcl_NewByteArrayObj returned null");
        }
    }
    // tuple / list -> Tcl list of typed elements (font tuples, coordinate lists).
    if let Some(elements) = decode_value_list(obj) {
        let list_obj = unsafe { (api.new_list_obj)(0, ptr::null()) };
        if list_obj.is_null() {
            return Err("Tcl_NewListObj returned null".to_string());
        }
        for &elem_bits in &elements {
            let elem = match tcl_obj_alloc_typed_from_bits(py, api, interp, elem_bits) {
                Ok(e) => e,
                Err(err) => {
                    // Free the partially-built list (refCount 0 -> incr/decr).
                    unsafe {
                        api.incr_ref_count_obj(list_obj);
                        api.decr_ref_count_obj(list_obj);
                    }
                    return Err(err);
                }
            };
            let rc = unsafe { (api.list_obj_append_element)(interp, list_obj, elem) };
            if rc != TCL_OK {
                unsafe {
                    api.incr_ref_count_obj(elem);
                    api.decr_ref_count_obj(elem);
                    api.incr_ref_count_obj(list_obj);
                    api.decr_ref_count_obj(list_obj);
                }
                let err = tcl_result_string(api, interp);
                return Err(if err.is_empty() {
                    "Tcl_ListObjAppendElement failed".to_string()
                } else {
                    err
                });
            }
        }
        return Ok(list_obj);
    }
    // Fallback: str() of the object (widget paths, custom __str__). Clear any
    // pending exception first so rt_str does not bail.
    if exception_pending(py) {
        clear_exception(py);
    }
    let str_bits = rt_str(bits);
    let text = if !obj_from_bits(str_bits).is_none() {
        string_obj_to_owned(obj_from_bits(str_bits))
    } else {
        None
    };
    dec_ref_bits(py, str_bits);
    match text {
        Some(s) => alloc_string_obj(api, &s),
        None => alloc_string_obj(api, &format_obj_str(py, obj)),
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn alloc_string_obj(api: &'static TclApi, text: &str) -> Result<*mut c_void, String> {
    let bytes = CString::new(text.as_bytes())
        .map_err(|_| "Tcl string contained interior NUL byte".to_string())?;
    let o = unsafe { (api.new_string_obj)(bytes.as_ptr(), text.len() as c_int) };
    non_null_obj(o, "Tcl_NewStringObj returned null")
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn non_null_obj(o: *mut c_void, msg: &'static str) -> Result<*mut c_void, String> {
    if o.is_null() {
        Err(msg.to_string())
    } else {
        Ok(o)
    }
}

/// FromObj: convert a Tcl result `Tcl_Obj*` to a molt value, dispatching on its
/// `typePtr` exactly like CPython `_tkinter.c`. Runs with the GIL held (allocates
/// molt objects). `obj` is borrowed (the interpreter owns the result); we never
/// free it here.
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn tcl_obj_result_to_bits(
    py: &PyToken,
    api: &'static TclApi,
    types: &TclTypePtrs,
    interp: *mut c_void,
    obj: *mut c_void,
) -> u64 {
    if obj.is_null() {
        return rt_none();
    }
    let tp = unsafe { (*obj.cast::<TclObjHeader>()).type_ptr };

    // typePtr == NULL  -> pure string (no internal rep).
    if tp.is_null() {
        return tcl_obj_string_to_bits(api, obj);
    }
    // boolean (checked before int: a boolean obj must not be read as wideInt).
    if tp == types.boolean_t && !types.boolean_t.is_null() {
        let mut b: c_int = 0;
        if unsafe { (api.get_boolean_from_obj)(interp, obj, &mut b) } == TCL_OK {
            return MoltObject::from_bool(b != 0).bits();
        }
        return tcl_obj_string_to_bits(api, obj);
    }
    // bytearray -> bytes.
    if tp == types.bytearray_t && !types.bytearray_t.is_null() {
        let mut len: c_int = 0;
        let data = unsafe { (api.get_byte_array_from_obj)(obj, &mut len) };
        if !data.is_null() && len >= 0 {
            let slice = unsafe { std::slice::from_raw_parts(data, len as usize) };
            return rt_bytes_from(slice);
        }
        return tcl_obj_string_to_bits(api, obj);
    }
    // double -> float.
    if tp == types.double_t && !types.double_t.is_null() {
        let mut d: f64 = 0.0;
        if unsafe { (api.get_double_from_obj)(interp, obj, &mut d) } == TCL_OK {
            return MoltObject::from_float(d).bits();
        }
        return tcl_obj_string_to_bits(api, obj);
    }
    // int / wideInt -> int. (bignum handled via the string fallback below, which
    // molt's int parser promotes to a bigint — value-identical.)
    if (tp == types.int_t && !types.int_t.is_null())
        || (tp == types.wide_int_t && !types.wide_int_t.is_null())
    {
        let mut w: TclWideInt = 0;
        if unsafe { (api.get_wide_int_from_obj)(interp, obj, &mut w) } == TCL_OK {
            return rt_int(w);
        }
        // Overflowed i64 (true bignum): fall back to the decimal string, which
        // molt parses into an arbitrary-precision int.
        return tcl_obj_int_string_to_bits(py, api, obj);
    }
    // bignum -> int (via decimal string -> molt bigint).
    if tp == types.bignum_t && !types.bignum_t.is_null() {
        return tcl_obj_int_string_to_bits(py, api, obj);
    }
    // list -> tuple (recurse on each element).
    if tp == types.list_t && !types.list_t.is_null() {
        let mut count: c_int = 0;
        let mut elems: *mut *mut c_void = ptr::null_mut();
        if unsafe { (api.list_obj_get_elements)(interp, obj, &mut count, &mut elems) } == TCL_OK
            && (count == 0 || !elems.is_null())
        {
            let n = count.max(0) as usize;
            let mut bits_vec: Vec<u64> = Vec::with_capacity(n);
            for i in 0..n {
                let elem = unsafe { *elems.add(i) };
                bits_vec.push(tcl_obj_result_to_bits(py, api, types, interp, elem));
            }
            // Build the tuple directly. We use `rt_tuple` rather than
            // `alloc_tuple_bits` because the latter also fails on any *already*
            // pending exception, and FromObj runs after a verified-OK Tcl eval —
            // a stale pending flag must not be misattributed to tuple allocation.
            let tuple_bits = rt_tuple(&bits_vec);
            if tuple_bits != 0 {
                return tuple_bits;
            }
            // Genuine allocation failure.
            return raise_exception_u64(
                py,
                "MemoryError",
                "failed to allocate tkinter result tuple",
            );
        }
        return tcl_obj_string_to_bits(api, obj);
    }
    // string / utf32string -> str.
    if (tp == types.string_t && !types.string_t.is_null())
        || (tp == types.utf32_string_t && !types.utf32_string_t.is_null())
    {
        return tcl_obj_string_to_bits(api, obj);
    }
    // Unknown internal type: match CPython, which wraps it in `_tkinter.Tcl_Obj`.
    // molt's `_tkinter.Tcl_Obj` is a `str` subclass, so the string value is the
    // faithful, comparison-correct representation.
    tcl_obj_string_to_bits(api, obj)
}

/// Read a `Tcl_Obj`'s UTF-8 string rep into a molt `str`.
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn tcl_obj_string_to_bits(api: &'static TclApi, obj: *mut c_void) -> u64 {
    let mut len: c_int = 0;
    let ptr = unsafe { (api.get_string_from_obj)(obj, &mut len) };
    if ptr.is_null() || len < 0 {
        return rt_string_from("");
    }
    let slice = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), len as usize) };
    // Tcl strings are Modified-UTF-8; for the common ASCII/UTF-8 case this is a
    // direct decode. Lossy only on genuinely invalid sequences (rare; CPython
    // uses surrogateescape — a follow-up can mirror that precisely).
    rt_string_from_bytes(slice)
}

/// Read a Tcl integer-typed `Tcl_Obj` whose value overflows i64 (a true bignum)
/// as its decimal string, then parse it into a molt arbitrary-precision int —
/// value-identical to CPython's `fromBignumObj`.
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn tcl_obj_int_string_to_bits(
    py: &PyToken,
    api: &'static TclApi,
    obj: *mut c_void,
) -> u64 {
    let mut len: c_int = 0;
    let ptr = unsafe { (api.get_string_from_obj)(obj, &mut len) };
    if ptr.is_null() || len < 0 {
        return rt_int(0);
    }
    let slice = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), len as usize) };
    let text = String::from_utf8_lossy(slice);
    // Parse the decimal string into a molt int via the runtime's int() path,
    // which promotes to arbitrary precision on overflow (CPython fromBignumObj
    // parity).
    let s_bits = rt_string_from(&text);
    let parsed = int_from_obj(s_bits, rt_int(10), MoltObject::from_bool(true).bits());
    dec_ref_bits(py, s_bits);
    parsed
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn register_tcl_callback_proc(app: &mut TkAppState, name: &str) -> Result<(), String> {
    let Some(interp) = app.interpreter.as_ref() else {
        return Ok(());
    };
    interp
        .eval((
            "proc",
            name.to_string(),
            "args",
            "lappend ::__molt_pending_callbacks [info level 0]; return {}",
        ))
        .map(|_| ())
        .map_err(|err| err.to_string())
}

#[cfg(all(not(target_arch = "wasm32"), not(feature = "native-tcl")))]
pub(super) fn register_tcl_callback_proc(_app: &mut TkAppState, _name: &str) -> Result<(), String> {
    Ok(())
}

#[cfg(target_arch = "wasm32")]
pub(super) fn register_tcl_callback_proc(_app: &mut TkAppState, _name: &str) -> Result<(), String> {
    Ok(())
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn unregister_tcl_callback_proc(app: &mut TkAppState, name: &str) {
    let Some(interp) = app.interpreter.as_ref() else {
        return;
    };
    let _ = interp.eval(("rename", name.to_string(), ""));
}

#[cfg(all(not(target_arch = "wasm32"), not(feature = "native-tcl")))]
pub(super) fn unregister_tcl_callback_proc(_app: &mut TkAppState, _name: &str) {}

#[cfg(target_arch = "wasm32")]
pub(super) fn unregister_tcl_callback_proc(_app: &mut TkAppState, _name: &str) {}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn unregister_all_tcl_callback_procs(app: &mut TkAppState) {
    let mut callback_names: Vec<String> = app.callbacks.keys().cloned().collect();
    callback_names.extend(app.filehandler_commands.keys().cloned());
    for callback_name in callback_names {
        unregister_tcl_callback_proc(app, &callback_name);
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn init_tcl_pending_callbacks(interp: &TclInterpreter) -> Result<(), String> {
    interp
        .eval((
            "set",
            "::__molt_pending_callbacks",
            TclObj::new_list(std::iter::empty::<TclObj>()),
        ))
        .map(|_| ())
        .map_err(|err| err.to_string())
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn build_native_tk_app(py: &PyToken, use_tk: bool) -> Result<TkAppState, u64> {
    let mut app = TkAppState::default();
    let interp = match std::panic::catch_unwind(TclInterpreter::new) {
        Ok(Ok(interp)) => interp,
        Ok(Err(err)) => {
            return Err(raise_tcl_error(
                py,
                &format!("failed to create Tcl interpreter: {err}"),
            ));
        }
        Err(_) => {
            return Err(raise_tcl_error(
                py,
                "failed to create Tcl interpreter: panic in tcl initialization",
            ));
        }
    };
    init_tcl_pending_callbacks(&interp).map_err(|err| {
        raise_tcl_error(
            py,
            &format!("failed to initialize tkinter callback queue: {err}"),
        )
    })?;
    if use_tk {
        interp
            .eval(("package", "require", "Tk"))
            .map_err(|err| raise_tcl_error(py, &format!("failed to load Tk package: {err}")))?;
        app.tk_loaded = true;
    }
    app.interpreter = Some(interp);
    Ok(app)
}

/// Allocate a Tcl_Obj from a `TclObj` part, using the given `TclApi`.
///
/// This is a free-standing version of `TclInterpreter::alloc_obj` that does
/// not require a `&TclInterpreter` reference — only the API function pointers
/// and the interpreter address (for list-append error messages).
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn alloc_tcl_obj_from_part(
    api: &'static TclApi,
    interp_addr: usize,
    part: &TclObj,
) -> Result<*mut c_void, String> {
    match &part.kind {
        TclObjKind::Scalar(text) => {
            let bytes = CString::new(text.as_bytes())
                .map_err(|_| "Tcl string contained interior NUL byte".to_string())?;
            let obj = unsafe { (api.new_string_obj)(bytes.as_ptr(), text.len() as c_int) };
            if obj.is_null() {
                return Err("Tcl_NewStringObj returned null".to_string());
            }
            Ok(obj)
        }
        TclObjKind::List(list) => {
            let list_obj = unsafe { (api.new_list_obj)(0, ptr::null()) };
            if list_obj.is_null() {
                return Err("Tcl_NewListObj returned null".to_string());
            }
            let interp = interp_addr as *mut c_void;
            for nested in list {
                let nested_obj = alloc_tcl_obj_from_part(api, interp_addr, nested)?;
                let rc = unsafe { (api.list_obj_append_element)(interp, list_obj, nested_obj) };
                if rc != TCL_OK {
                    // Safely free refcount-0 objects: incr then decr
                    unsafe {
                        api.incr_ref_count_obj(nested_obj);
                        api.decr_ref_count_obj(nested_obj);
                        api.incr_ref_count_obj(list_obj);
                        api.decr_ref_count_obj(list_obj);
                    }
                    let err = tcl_result_string(api, interp);
                    return Err(if err.is_empty() {
                        "Tcl_ListObjAppendElement failed".to_string()
                    } else {
                        err
                    });
                }
            }
            Ok(list_obj)
        }
    }
}

/// Evaluate a Tcl command (given as `Vec<TclObj>` parts) on the interpreter
/// at `interp_addr`, releasing the GIL for the duration of the Tcl call.
///
/// All argument conversion must happen *before* this function is called
/// (while the GIL is held).  This function only touches Tcl APIs — no
/// Molt runtime calls.
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn eval_tcl_without_gil(
    api: &'static TclApi,
    interp_addr: usize,
    parts: &[TclObj],
) -> Result<String, String> {
    let interp = interp_addr as *mut c_void;
    let mut objv = Vec::with_capacity(parts.len());
    for part in parts {
        let obj = alloc_tcl_obj_from_part(api, interp_addr, part)?;
        // Immediately incr so the object is owned (refcount 1).
        unsafe { api.incr_ref_count_obj(obj) };
        objv.push(obj);
    }

    let rc = {
        let _gil_release = GilReleaseGuard::new();
        unsafe { (api.eval_objv)(interp, objv.len() as c_int, objv.as_ptr(), 0) }
    };

    for &obj in &objv {
        unsafe { api.decr_ref_count_obj(obj) };
    }

    let result = tcl_result_string(api, interp);
    if rc != TCL_OK {
        return Err(if result.is_empty() {
            "Tcl_EvalObjv failed".to_string()
        } else {
            result
        });
    }
    Ok(result)
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn tcl_trace_enabled() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| std::env::var("MOLT_TRACE_TCL").is_ok())
}

/// Locking wrapper: resolve the interpreter context under one registry lock,
/// then run the command. Used by paths that have not already resolved the
/// context (event-loop draining, the headless fallthrough).
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn run_tcl_command(py: &PyToken, handle: i64, args: &[u64]) -> Result<u64, u64> {
    let (api, interp_addr, types) = {
        let mut registry = tk_registry().lock().unwrap();
        let app = app_mut_from_registry(py, &mut registry, handle)?;
        let Some(interp) = app.interpreter.as_ref() else {
            return Err(app_tcl_error_locked(
                py,
                app,
                "tk runtime interpreter is unavailable",
            ));
        };
        if let Err(err) = interp.ensure_owner_thread() {
            return Err(app_tcl_error_locked(py, app, err));
        }
        (interp.api, interp.interp_addr, interp.types)
    };
    run_tcl_command_with_ctx(py, handle, args, api, interp_addr, types)
}

/// Run a Tcl command with a pre-resolved interpreter context (no Phase-2 lock).
/// This is the hot path: `tk_call_dispatch` resolves the context in the same
/// single lock that checks callbacks/filehandlers, so a generic `tk.call`
/// touches the registry mutex only twice (resolve + final clear_last_error).
#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn run_tcl_command_with_ctx(
    py: &PyToken,
    handle: i64,
    args: &[u64],
    api: &'static TclApi,
    interp_addr: usize,
    types: TclTypePtrs,
) -> Result<u64, u64> {
    let interp = interp_addr as *mut c_void;

    // Optional Tcl tracing (string-renders the typed args for human reading).
    // The env var is read once and cached — a getenv syscall per tk.call would be
    // a measurable tax on the hot path.
    if tcl_trace_enabled() {
        let rendered: Vec<TclObj> = args.iter().map(|&b| tcl_obj_from_bits(py, b)).collect();
        eprintln!("[tcl] {}", TclObj::new_list(rendered));
    }

    // Phase 1 (GIL held): build TYPED Tcl_Obj argv directly from molt values.
    let mut objv: Vec<*mut c_void> = Vec::with_capacity(args.len());
    for &bits in args {
        match tcl_obj_alloc_typed_from_bits(py, api, interp, bits) {
            Ok(obj) => {
                unsafe { api.incr_ref_count_obj(obj) };
                objv.push(obj);
            }
            Err(err) => {
                for &allocated in &objv {
                    unsafe { api.decr_ref_count_obj(allocated) };
                }
                let message = format!("tk command failed: {err}");
                set_last_error(handle, message.clone());
                return Err(raise_tcl_error(py, &message));
            }
        }
    }

    // Phase 3 (GIL released): evaluate. No molt runtime calls in this window.
    let rc = {
        let _gil_release = GilReleaseGuard::new();
        unsafe { (api.eval_objv)(interp, objv.len() as c_int, objv.as_ptr(), 0) }
    };

    // Phase 4 (GIL held): convert the typed result, or raise the error string.
    if rc != TCL_OK {
        let err = tcl_result_string(api, interp);
        for &obj in &objv {
            unsafe { api.decr_ref_count_obj(obj) };
        }
        let message = format!(
            "tk command failed: {}",
            if err.is_empty() {
                "Tcl_EvalObjv failed".to_string()
            } else {
                err
            }
        );
        set_last_error(handle, message.clone());
        return Err(raise_tcl_error(py, &message));
    }

    // Read the result object BEFORE dropping the argv refs (the result may alias
    // an argument, e.g. `set x` returns the value object).
    let result_obj = unsafe { (api.get_obj_result)(interp) };
    // Own the result across the argv teardown so a shared object is not freed.
    if !result_obj.is_null() {
        unsafe { api.incr_ref_count_obj(result_obj) };
    }
    for &obj in &objv {
        unsafe { api.decr_ref_count_obj(obj) };
    }
    let bits = tcl_obj_result_to_bits(py, api, &types, interp, result_obj);
    if !result_obj.is_null() {
        unsafe { api.decr_ref_count_obj(result_obj) };
    }
    clear_last_error(handle);
    Ok(bits)
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn take_pending_tcl_callbacks(
    py: &PyToken,
    handle: i64,
) -> Result<Vec<Vec<String>>, u64> {
    // Extract interpreter context, then drop registry lock before
    // releasing the GIL for the Tcl variable operations.
    let (api, interp_addr) = {
        let mut registry = tk_registry().lock().unwrap();
        let app = app_mut_from_registry(py, &mut registry, handle)?;
        let Some(interp) = app.interpreter.as_ref() else {
            return Ok(Vec::new());
        };
        if interp.ensure_owner_thread().is_err() {
            return Ok(Vec::new());
        }
        (interp.api, interp.interp_addr)
        // registry lock dropped here
    };

    // Read and reset the pending callbacks variable with GIL released.
    let pending_text = {
        let get_parts = [
            TclObj::from("set"),
            TclObj::from("::__molt_pending_callbacks"),
        ];
        let reset_parts = [
            TclObj::from("set"),
            TclObj::from("::__molt_pending_callbacks"),
            TclObj::new_list(std::iter::empty::<TclObj>()),
        ];

        let interp = interp_addr as *mut c_void;
        let _gil_release = GilReleaseGuard::new();

        // Read current value
        let mut get_objv = Vec::with_capacity(get_parts.len());
        for part in &get_parts {
            let obj = match alloc_tcl_obj_from_part(api, interp_addr, part) {
                Ok(obj) => obj,
                Err(_) => return Ok(Vec::new()),
            };
            unsafe { api.incr_ref_count_obj(obj) };
            get_objv.push(obj);
        }
        let rc = unsafe { (api.eval_objv)(interp, get_objv.len() as c_int, get_objv.as_ptr(), 0) };
        for &obj in &get_objv {
            unsafe { api.decr_ref_count_obj(obj) };
        }
        if rc != TCL_OK {
            return Ok(Vec::new());
        }
        let pending_text = tcl_result_string(api, interp);

        // Reset to empty list
        let mut reset_objv = Vec::with_capacity(reset_parts.len());
        for part in &reset_parts {
            let obj = match alloc_tcl_obj_from_part(api, interp_addr, part) {
                Ok(obj) => obj,
                Err(_) => break,
            };
            unsafe { api.incr_ref_count_obj(obj) };
            reset_objv.push(obj);
        }
        if reset_objv.len() == reset_parts.len() {
            let _ = unsafe {
                (api.eval_objv)(interp, reset_objv.len() as c_int, reset_objv.as_ptr(), 0)
            };
        }
        for &obj in &reset_objv {
            unsafe { api.decr_ref_count_obj(obj) };
        }

        pending_text
    };

    if pending_text.is_empty() {
        return Ok(Vec::new());
    }

    let pending_obj = TclObj::scalar_from_interp(pending_text, interp_addr);
    let mut calls = Vec::new();
    let Ok(pending_iter) = pending_obj.get_elements() else {
        return Ok(calls);
    };
    for pending_call in pending_iter {
        if let Ok(parts) = pending_call.get_elements() {
            calls.push(parts.map(|obj| obj.to_string()).collect());
        } else {
            calls.push(vec![pending_call.to_string()]);
        }
    }
    Ok(calls)
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn dispatch_named_callback_from_strings(
    py: &PyToken,
    handle: i64,
    argv: Vec<String>,
) -> Result<bool, u64> {
    if argv.is_empty() {
        return Ok(false);
    }
    let command_name = argv[0].clone();
    if let Some(out_bits) = invoke_filehandler_command(py, handle, &command_name)? {
        if !obj_from_bits(out_bits).is_none() {
            dec_ref_bits(py, out_bits);
        }
        return Ok(true);
    }

    let (callback_bits, oneshot) = {
        let mut registry = tk_registry().lock().unwrap();
        let app = app_mut_from_registry(py, &mut registry, handle)?;
        let Some(bits) = app.callbacks.get(&command_name).copied() else {
            return Ok(false);
        };
        inc_ref_bits(py, bits);
        let oneshot = app.one_shot_callbacks.remove(&command_name);
        if oneshot {
            if let Some(old_bits) = app.callbacks.remove(&command_name) {
                debug_assert_eq!(old_bits, bits);
            }
            let oneshot_tokens = tokens_for_after_command(app, &command_name);
            for token in &oneshot_tokens {
                unregister_after_command_token(app, token);
            }
            remove_after_events_for_tokens(app, &oneshot_tokens);
            unregister_tcl_callback_proc(app, &command_name);
        }
        (bits, oneshot)
    };

    let mut arg_bits = Vec::new();
    for arg in argv.iter().skip(1) {
        match alloc_string_bits(py, arg) {
            Ok(bits) => arg_bits.push(bits),
            Err(bits) => {
                dec_ref_bits(py, callback_bits);
                for allocated in arg_bits {
                    dec_ref_bits(py, allocated);
                }
                return Err(bits);
            }
        }
    }

    let out_bits = invoke_callback(py, callback_bits, &arg_bits);
    dec_ref_bits(py, callback_bits);
    for allocated in arg_bits {
        dec_ref_bits(py, allocated);
    }
    if exception_pending(py) {
        if !obj_from_bits(out_bits).is_none() {
            dec_ref_bits(py, out_bits);
        }
        set_last_error(handle, "bound tkinter command raised an exception");
        return Err(MoltObject::none().bits());
    }
    if !obj_from_bits(out_bits).is_none() {
        dec_ref_bits(py, out_bits);
    }
    if oneshot {
        clear_last_error(handle);
        return Ok(true);
    }
    clear_last_error(handle);
    Ok(true)
}

#[cfg(all(not(target_arch = "wasm32"), feature = "native-tcl"))]
pub(super) fn pump_tcl_events(py: &PyToken, handle: i64, flags: i32) -> Result<bool, u64> {
    // Extract the do_one_event fn pointer and interp address, then drop
    // the registry lock before releasing the GIL for the Tcl call.
    let (do_one_event_fn, _interp_addr) = {
        let mut registry = tk_registry().lock().unwrap();
        let app = app_mut_from_registry(py, &mut registry, handle)?;
        let Some(interp) = app.interpreter.as_ref() else {
            return Ok(false);
        };
        if let Err(err) = interp.ensure_owner_thread() {
            return Err(app_tcl_error_locked(py, app, err));
        }
        (interp.api.do_one_event, interp.interp_addr)
        // registry lock dropped here
    };

    let event_handled = {
        let _gil_release = GilReleaseGuard::new();
        unsafe { do_one_event_fn(flags as c_int) != 0 }
    };

    let pending_callbacks = take_pending_tcl_callbacks(py, handle)?;
    let mut callback_handled = false;
    for callback_argv in pending_callbacks {
        if dispatch_named_callback_from_strings(py, handle, callback_argv)? {
            callback_handled = true;
        }
    }
    Ok(event_handled || callback_handled)
}

#[cfg(all(not(target_arch = "wasm32"), not(feature = "native-tcl")))]
pub(super) fn pump_tcl_events(_py: &PyToken, _handle: i64, _flags: i32) -> Result<bool, u64> {
    Ok(false)
}

#[cfg(target_arch = "wasm32")]
pub(super) fn pump_tcl_events(_py: &PyToken, _handle: i64, _flags: i32) -> Result<bool, u64> {
    Ok(false)
}
