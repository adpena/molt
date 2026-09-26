//! One-image CPython-ABI extension host for `tests/cext_dlopen_smoke.rs`.
//!
//! This cdylib links molt-runtime and the molt-cpython-abi rlib into one image,
//! so the `Py*` exports a native extension binds, the runtime hook table, bridge
//! maps, static types and thread-local error state are a single instance. The
//! test executable links its fixture extension against this image, loads it and
//! calls one C entry point that exchanges only C strings and a POD status.
//! Runtime initialization, extension loading, dispatch, cleanup, GC and
//! shutdown all run here, through production runtime and ABI entry points.
//!
//! Build it in the test's exact Cargo target/profile first:
//! `cargo build -p molt-runtime --example cext_host --features cext_loader`.

#![cfg(all(feature = "cext_loader", not(target_arch = "wasm32")))]

use molt_cpython_abi::PyObject;
use molt_cpython_abi::abi_types::{METH_NOARGS, PyCFunctionObject, PyMethodDef, PyModuleDef};
use molt_cpython_abi::api::{errors, modules, object, refcount};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use molt_obj_model::MoltObject;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::panic::AssertUnwindSafe;
use std::path::Path;

// The host loads extensions but has no compiler-generated application image.
molt_runtime::declare_app_bootstrap!(molt_runtime::AppBootstrapProvider::Unavailable(
    "molt-runtime/cext_host"
));

unsafe extern "C" {
    fn molt_object_getattr_bytes(obj_bits: u64, name_ptr: *const u8, name_len: u64) -> u64;
    fn molt_call_func_dispatch(func_bits: u64, args_ptr_bits: u64, nargs: u64, code_id: u64)
    -> u64;
    fn molt_string_as_ptr(string_bits: u64, out_len: *mut u64) -> *const u8;
    fn molt_exception_pending() -> u64;
    fn molt_exception_last() -> u64;
    fn molt_exception_clear() -> u64;
    fn molt_exception_kind(exc_bits: u64) -> u64;
    fn molt_exception_message(exc_bits: u64) -> u64;
    fn molt_type_of(val_bits: u64) -> u64;
    fn molt_dec_ref_obj(bits: u64);
    fn molt_module_cache_get(name_bits: u64) -> u64;
    fn molt_module_cache_del(name_bits: u64) -> u64;
    fn molt_gc_collect(generation_bits: u64) -> u64;
    fn molt_weakref_reference_type() -> u64;
    fn molt_weakref_new(class_bits: u64, target_bits: u64, callback_bits: u64) -> u64;
    fn molt_weakref_call(self_bits: u64) -> u64;
}

const MODULE: &str = "hello";
const GREETING: &str = "hello from C";
const FAILURE: &str = "deliberate hello failure";
// The oldest generation collects every generation.
const FULL_GC_GENERATION: i64 = 2;

type Check<T> = Result<T, String>;

/// One owned runtime reference, released on every exit path.
struct Owned(u64);

impl Drop for Owned {
    fn drop(&mut self) {
        unsafe { molt_dec_ref_obj(self.0) };
    }
}

/// One owned C reference to a bridge view.
struct View(*mut PyObject);

impl View {
    fn of(bits: u64, what: &str) -> Check<Self> {
        let view = unsafe { GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(bits) };
        if view.is_null() {
            return Err(format!(
                "{what} has no CPython-ABI view{}",
                c_error_suffix()
            ));
        }
        Ok(Self(view))
    }
}

impl Drop for View {
    fn drop(&mut self) {
        unsafe { refcount::Py_DECREF(self.0) };
    }
}

/// Fixture-only extension exports. They carry addresses of the extension's
/// own static definitions and of its resolved import, never runtime state.
struct Probes {
    // Opening maps the extension without running PyInit. The loader later
    // takes and pins its own reference to this same mapping.
    _image: libloading::Library,
    bound_module_create2: *const c_void,
    methods: *const PyMethodDef,
    module_def: *const PyModuleDef,
}

impl Probes {
    fn open(path: &Path) -> Check<Self> {
        let image = unsafe { libloading::Library::new(path) }
            .map_err(|error| format!("open extension {path:?}: {error}"))?;
        let probe = |name: &str| -> Check<*const c_void> {
            let function =
                unsafe { image.get::<unsafe extern "C" fn() -> *const c_void>(name.as_bytes()) }
                    .map_err(|error| format!("extension probe {name} is not exported: {error}"))?;
            Ok(unsafe { function() })
        };
        let bound_module_create2 = probe("hello_probe_module_create2")?;
        let methods = probe("hello_probe_methods")?.cast::<PyMethodDef>();
        let module_def = probe("hello_probe_module_def")?.cast::<PyModuleDef>();
        Ok(Self {
            _image: image,
            bound_module_create2,
            methods,
            module_def,
        })
    }
}

fn runtime_text(bits: u64) -> Option<String> {
    let mut len = 0;
    let ptr = unsafe { molt_string_as_ptr(bits, &mut len) };
    if ptr.is_null() {
        unsafe { molt_exception_clear() };
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
    Some(String::from_utf8_lossy(bytes).into_owned())
}

fn owned_text(bits: u64) -> Option<String> {
    let owner = Owned(bits);
    runtime_text(owner.0)
}

/// Take and describe the pending runtime exception, if any.
fn take_pending_exception() -> Option<String> {
    if unsafe { molt_exception_pending() } == 0 {
        return None;
    }
    let exception = Owned(unsafe { molt_exception_last() });
    unsafe { molt_exception_clear() };
    let kind = owned_text(unsafe { molt_exception_kind(exception.0) });
    let message = owned_text(unsafe { molt_exception_message(exception.0) });
    unsafe { molt_exception_clear() };
    Some(format!(
        "{}: {}",
        kind.as_deref().unwrap_or("<unknown exception>"),
        message.as_deref().unwrap_or("<no message>")
    ))
}

fn pending_suffix() -> String {
    take_pending_exception()
        .map(|error| format!(" ({error})"))
        .unwrap_or_default()
}

fn c_error_suffix() -> String {
    let Some(error) = errors::take_current_error() else {
        return String::new();
    };
    let render = |value| {
        let text = View(unsafe { molt_cpython_abi::api::typeobj::PyObject_Str(value) });
        if text.0.is_null() {
            return "<unprintable>".to_owned();
        }
        let utf8 = unsafe { molt_cpython_abi::api::strings::PyUnicode_AsUTF8(text.0) };
        if utf8.is_null() {
            return "<unprintable>".to_owned();
        }
        unsafe { CStr::from_ptr(utf8) }
            .to_string_lossy()
            .into_owned()
    };
    let description = format!(" ({}: {})", render(error.exc_type), render(error.value));
    unsafe { errors::PyErr_Clear() };
    errors::restore_current_error_exact(error);
    description
}

fn no_pending(context: &str) -> Check<()> {
    match take_pending_exception() {
        Some(error) => Err(format!("{context}: {error}")),
        None => Ok(()),
    }
}

fn getattr(owner: u64, name: &str) -> Check<Owned> {
    let bits = unsafe { molt_object_getattr_bytes(owner, name.as_ptr(), name.len() as u64) };
    let value = Owned(bits);
    no_pending(&format!("runtime getattr {name}"))?;
    Ok(value)
}

fn call_noargs(function: u64) -> u64 {
    let args: [u64; 0] = [];
    unsafe { molt_call_func_dispatch(function, args.as_ptr() as u64, 0, 0) }
}

/// The runtime attribute and the C API attribute are one physical
/// `PyCFunctionObject` over the extension's own static `PyMethodDef`.
fn method(
    module: &Owned,
    module_view: &View,
    name: &str,
    definition: *const PyMethodDef,
) -> Check<Owned> {
    let function = getattr(module.0, name)?;
    let view = View::of(function.0, &format!("module.{name}"))?;
    let c_name = CString::new(name).expect("method names have no NUL");
    let abi = unsafe { object::PyObject_GetAttrString(module_view.0, c_name.as_ptr()) };
    if abi.is_null() {
        return Err(format!("C API getattr {name} failed{}", c_error_suffix()));
    }
    let abi = View(abi);
    if abi.0 != view.0 {
        return Err(format!(
            "module.{name}: runtime view {:p} and C API attribute {:p} are different objects",
            view.0, abi.0
        ));
    }
    if unsafe { object::PyCFunction_Check(view.0) } == 0 {
        return Err(format!("module.{name} is not a PyCFunctionObject"));
    }
    let physical_definition = unsafe { (*view.0.cast::<PyCFunctionObject>()).m_ml };
    if physical_definition.cast_const() != definition {
        return Err(format!(
            "module.{name} references PyMethodDef {physical_definition:p}, not the fixture definition {definition:p}"
        ));
    }
    let expected = unsafe { (*definition).ml_meth }.map(|function| function as usize);
    let actual =
        unsafe { object::PyCFunction_GetFunction(view.0) }.map(|function| function as usize);
    if expected.is_none() || actual != expected {
        return Err(format!(
            "module.{name} dispatches to {actual:x?}, not the fixture function {expected:x?}"
        ));
    }
    if unsafe { object::PyCFunction_GetSelf(view.0) } != module_view.0 {
        return Err(format!("module.{name} is not bound to its module view"));
    }
    if unsafe { object::PyCFunction_GetFlags(view.0) } != METH_NOARGS {
        return Err(format!("module.{name} lost its METH_NOARGS convention"));
    }
    Ok(function)
}

unsafe fn exercise(path: &Path) -> Check<()> {
    // Topology, before any extension code runs: the extension's resolved
    // import is this image's own export, not an ambient or standalone
    // CPython-ABI image.
    let probes = Probes::open(path)?;
    let own_module_create2 = modules::PyModule_Create2 as *const () as usize;
    if probes.bound_module_create2 as usize != own_module_create2 {
        return Err(format!(
            "extension bound PyModule_Create2 at {:p}, but this host exports it at {own_module_create2:#x}; the extension is linked to a different CPython-ABI image",
            probes.bound_module_create2
        ));
    }

    let module_bits = unsafe { molt_cpython_abi::loader::load_cpython_extension(path, MODULE) }
        .map_err(|error| {
            format!(
                "extension load failed: {error}{}{}",
                pending_suffix(),
                c_error_suffix()
            )
        })?;
    if module_bits == 0 || module_bits == MoltObject::none().bits() {
        return Err(format!("loader returned no module (bits={module_bits:#x})"));
    }
    let module = Owned(module_bits);

    // Single-phase init published the extension's own static definition.
    let module_view = View::of(module.0, "extension module")?;
    let def = unsafe { modules::PyModule_GetDef(module_view.0) };
    if def.cast_const() != probes.module_def {
        return Err(format!(
            "PyModule_GetDef returned {def:p}, not the fixture PyModuleDef {:p}",
            probes.module_def
        ));
    }
    if unsafe { modules::PyState_FindModule(def) } != module_view.0 {
        return Err(format!(
            "single-phase module is not registered for its definition{}",
            c_error_suffix()
        ));
    }

    let greet = method(&module, &module_view, "greet", probes.methods)?;
    let fail = method(&module, &module_view, "fail", unsafe {
        probes.methods.add(1)
    })?;

    let result = Owned(call_noargs(greet.0));
    no_pending("greet()")?;
    match runtime_text(result.0) {
        Some(text) if text == GREETING => {}
        other => return Err(format!("greet() returned {other:?}, not {GREETING:?}")),
    }
    drop(result);

    // The C error crosses the runtime call boundary as its exact class and
    // message, and the C error indicator does not remain set.
    let error_class = getattr(module.0, "HelloError")?;
    let returned = Owned(call_noargs(fail.0));
    if unsafe { molt_exception_pending() } == 0 {
        return Err("fail() returned without raising".into());
    }
    if !unsafe { errors::PyErr_Occurred() }.is_null() {
        return Err("fail() left the C error indicator set after runtime transfer".into());
    }
    let exception = Owned(unsafe { molt_exception_last() });
    unsafe { molt_exception_clear() };
    drop(returned);
    let exception_class = Owned(unsafe { molt_type_of(exception.0) });
    if exception_class.0 != error_class.0 {
        let kind = owned_text(unsafe { molt_exception_kind(exception.0) });
        return Err(format!(
            "fail() raised {kind:?}, not the module's HelloError class"
        ));
    }
    match owned_text(unsafe { molt_exception_message(exception.0) }) {
        Some(message) if message == FAILURE => {}
        other => return Err(format!("fail() message was {other:?}, not {FAILURE:?}")),
    }
    drop((exception_class, exception, error_class));

    // Release every owner of the module: callables, C view, PyState
    // registration and the module cache. The CFunction m_self edges still form
    // a cycle that only the collector may break.
    let weak_type = Owned(unsafe { molt_weakref_reference_type() });
    let weak = Owned(unsafe { molt_weakref_new(weak_type.0, module.0, MoltObject::none().bits()) });
    no_pending("weakref to extension module")?;
    drop((weak_type, greet, fail));

    if unsafe { modules::PyState_RemoveModule(def) } != 0 {
        return Err(format!("PyState_RemoveModule failed{}", c_error_suffix()));
    }
    if !unsafe { modules::PyState_FindModule(def) }.is_null() {
        return Err("PyState_FindModule still finds the removed module".into());
    }
    drop(module_view);

    let name = getattr(module.0, "__name__")?;
    if runtime_text(name.0).as_deref() != Some(MODULE) {
        return Err(format!("module __name__ is not {MODULE:?}"));
    }
    let cached = Owned(unsafe { molt_module_cache_get(name.0) });
    no_pending("module cache lookup")?;
    if cached.0 != module.0 {
        return Err("module cache does not hold the loaded extension module".into());
    }
    drop(cached);
    unsafe { molt_module_cache_del(name.0) };
    no_pending("module cache removal")?;
    let cached = Owned(unsafe { molt_module_cache_get(name.0) });
    no_pending("module cache lookup after removal")?;
    if cached.0 != MoltObject::none().bits() {
        return Err("module cache still holds the extension module after removal".into());
    }
    drop((cached, name, module));

    unsafe { molt_gc_collect(MoltObject::from_int(FULL_GC_GENERATION).bits()) };
    no_pending("full collection")?;
    let referent = Owned(unsafe { molt_weakref_call(weak.0) });
    no_pending("weakref dereference")?;
    if referent.0 != MoltObject::none().bits() {
        return Err(
            "extension module survived release of every owner and a full collection".into(),
        );
    }
    Ok(())
}

unsafe fn run_hello(extension_path: *const c_char) -> Check<()> {
    if extension_path.is_null() {
        return Err("extension path is NULL".into());
    }
    let path = unsafe { CStr::from_ptr(extension_path) }
        .to_str()
        .map_err(|_| "extension path is not UTF-8".to_owned())?;
    if molt_runtime::lifecycle::init() != 1 {
        return Err("runtime initialization failed".into());
    }
    if !molt_runtime::cpython_abi_hooks::register_cpython_hooks() {
        return Err(format!(
            "CPython-ABI hook registration failed{}",
            pending_suffix()
        ));
    }
    let outcome =
        molt_runtime::lifecycle::with_ready_execution(|| unsafe { exercise(Path::new(path)) })
            .unwrap_or_else(|| Err("runtime is not ready for execution".into()));
    // Shutdown runs outside the execution lease, inside this image; the test
    // keeps the image loaded afterwards.
    let shutdown = molt_runtime::lifecycle::shutdown();
    outcome?;
    if shutdown != 1 {
        return Err("runtime shutdown refused after extension cleanup".into());
    }
    Ok(())
}

/// Run the `hello` extension scenario in this image.
///
/// Returns 0 on success. Otherwise writes a NUL-terminated UTF-8 diagnostic,
/// truncated to `diagnostic_capacity`, and returns 1.
///
/// # Safety
/// `extension_path` must be a NUL-terminated string and `diagnostic` NULL or
/// writable for `diagnostic_capacity` bytes. Call at most once per process:
/// the runtime is shut down before this returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_cext_host_run_hello(
    extension_path: *const c_char,
    diagnostic: *mut c_char,
    diagnostic_capacity: usize,
) -> c_int {
    let outcome =
        std::panic::catch_unwind(AssertUnwindSafe(|| unsafe { run_hello(extension_path) }))
            .unwrap_or_else(|panic| {
                let detail = panic
                    .downcast_ref::<&str>()
                    .map(|text| (*text).to_owned())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "<non-string panic>".into());
                Err(format!("host panicked: {detail}"))
            });
    let (status, message) = match outcome {
        Ok(()) => (0, String::new()),
        Err(message) => (1, message),
    };
    if !diagnostic.is_null() && diagnostic_capacity != 0 {
        let len = message.len().min(diagnostic_capacity - 1);
        unsafe {
            std::ptr::copy_nonoverlapping(message.as_ptr().cast::<c_char>(), diagnostic, len);
            *diagnostic.add(len) = 0;
        }
    }
    status
}
