//! Fresh execution transactions for compiler-emitted module bodies.
//!
//! `runpy` and importlib must execute the same compiled module body as normal
//! imports. A scoped execution context owns its private namespace independently
//! of global import initialization rows and public membership. Publication seeds
//! execution metadata and the optional run-name swap; ordinary imports performed
//! by the body still consult the public dictionary and its initialization owner. No source parser, second module store or
//! alternate import dispatcher lives here.

use super::*;
use std::cell::RefCell;

struct SysModulesSwap {
    modules_bits: u64,
    modules_ptr: *mut u8,
    key_bits: u64,
    previous_bits: Option<u64>,
}

struct ExecutionContext {
    runtime_id: usize,
    // The outermost execution that mutates process-visible sys state holds
    // this through restoration. Nested execution on the same thread reuses
    // that custody; loader execution never takes the global lock.
    sys_transition_guard: Option<std::sync::MutexGuard<'static, ()>>,
    import_name: String,
    run_name: String,
    init_globals_bits: Option<u64>,
    alter_sys: bool,
    metadata: ExecutionMetadata,
    module_bits: u64,
    sys_modules_swap: Option<SysModulesSwap>,
    metadata_override_pending: u8,
    spec_metadata_pending: bool,
}

#[derive(Clone)]
pub(crate) enum ExecutionMetadata {
    Module {
        argv0: Option<String>,
    },
    /// Preserve the loader-created module metadata while executing the
    /// compiler body through the same fresh-module transaction. `MODULE_NEW`
    /// is redirected to this exact caller-owned module object.
    LoaderNamespace {
        module_bits: u64,
    },
    ScriptFile(String),
    ImportContainer(String),
}

const EXECUTION_NAME_PENDING: u8 = 1 << 0;
const SCRIPT_FILE_PENDING: u8 = 1 << 1;
const SCRIPT_PACKAGE_PENDING: u8 = 1 << 2;
const SCRIPT_SPEC_PENDING: u8 = 1 << 3;
const SCRIPT_METADATA_PENDING: u8 =
    EXECUTION_NAME_PENDING | SCRIPT_FILE_PENDING | SCRIPT_PACKAGE_PENDING | SCRIPT_SPEC_PENDING;
/// `Loader.exec_module` sets no module metadata: CPython runs
/// `_init_module_attrs` only in `module_from_spec`, which bound `__path__` to
/// the spec's `submodule_search_locations` object. The compiled prologue keeps
/// each value the module already has and stores none it lacks.
const LOADER_PATH_PENDING: u8 = 1 << 4;
const LOADER_METADATA_PENDING: u8 = SCRIPT_METADATA_PENDING | LOADER_PATH_PENDING;

thread_local! {
    static EXECUTION_STACK: RefCell<Vec<ExecutionContext>> = const { RefCell::new(Vec::new()) };
}

struct Argv0Swap {
    previous_bits: u64,
}

struct SysPathSwap {
    path_bits: u64,
}

unsafe fn sys_list_attr_bits(_py: &PyToken<'_>, attr: &[u8]) -> Result<Option<u64>, u64> {
    unsafe {
        let Some(sys_bits) = interpreter_sys_module(_py).inspect(|bits| inc_ref_bits(_py, *bits))
        else {
            return Ok(None);
        };
        let Some(sys_ptr) = obj_from_bits(sys_bits).as_ptr() else {
            dec_ref_bits(_py, sys_bits);
            return Ok(None);
        };
        if object_type_id(sys_ptr) != TYPE_ID_MODULE {
            dec_ref_bits(_py, sys_bits);
            return Ok(None);
        }
        let attr_ptr = alloc_string(_py, attr);
        if attr_ptr.is_null() {
            dec_ref_bits(_py, sys_bits);
            return Err(crate::builtins::exceptions::ExceptionSentinel::exception_sentinel());
        }
        let attr_bits = MoltObject::from_ptr(attr_ptr).bits();
        let value_bits = crate::builtins::attributes::attr_lookup_ptr(_py, sys_ptr, attr_bits);
        dec_ref_bits(_py, attr_bits);
        dec_ref_bits(_py, sys_bits);
        let Some(value_bits) = value_bits.map_err(|()| MoltObject::none().bits())? else {
            return Ok(None);
        };
        let is_list = obj_from_bits(value_bits)
            .as_ptr()
            .is_some_and(|ptr| object_type_id(ptr) == TYPE_ID_LIST);
        if !is_list {
            dec_ref_bits(_py, value_bits);
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                &format!("sys.{} must be a list", String::from_utf8_lossy(attr)),
            ));
        }
        Ok(Some(value_bits))
    }
}

pub(super) unsafe fn execution_sys_path_entries(_py: &PyToken<'_>) -> Result<Vec<String>, u64> {
    unsafe {
        let Some(path_bits) = sys_list_attr_bits(_py, b"path")? else {
            return Ok(Vec::new());
        };
        let path_ptr = obj_from_bits(path_bits)
            .as_ptr()
            .expect("validated sys.path list");
        let entries = crate::object::seq_access::with_borrowed(path_ptr, |items| {
            items
                .iter()
                .filter_map(|&bits| string_obj_to_owned(obj_from_bits(bits)))
                .collect()
        });
        dec_ref_bits(_py, path_bits);
        Ok(entries)
    }
}

unsafe fn begin_argv0_swap(_py: &PyToken<'_>, argv0: &str) -> Result<Option<Argv0Swap>, u64> {
    unsafe {
        let Some(argv_bits) = sys_list_attr_bits(_py, b"argv")? else {
            return Ok(None);
        };
        let argv_ptr = obj_from_bits(argv_bits)
            .as_ptr()
            .expect("validated sys.argv list");
        if crate::object::seq_access::locked_len(argv_ptr) == 0 {
            dec_ref_bits(_py, argv_bits);
            return Ok(None);
        }
        let value_ptr = alloc_string(_py, argv0.as_bytes());
        if value_ptr.is_null() {
            dec_ref_bits(_py, argv_bits);
            return Err(crate::builtins::exceptions::ExceptionSentinel::exception_sentinel());
        }
        let value_bits = MoltObject::from_ptr(value_ptr).bits();
        let Some(previous) = crate::object::seq_access::pin_item(_py, argv_ptr, 0) else {
            dec_ref_bits(_py, value_bits);
            dec_ref_bits(_py, argv_bits);
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "failed to read sys.argv[0]",
            ));
        };
        let previous_bits = previous.bits();
        inc_ref_bits(_py, previous_bits);
        drop(previous);
        let replaced = crate::object::list_mutation::replace_indices(
            _py,
            argv_ptr,
            &[0],
            std::slice::from_ref(&value_bits),
        );
        dec_ref_bits(_py, value_bits);
        dec_ref_bits(_py, argv_bits);
        if !replaced {
            dec_ref_bits(_py, previous_bits);
            return Err(if exception_pending(_py) {
                MoltObject::none().bits()
            } else {
                raise_exception::<_>(_py, "RuntimeError", "failed to replace sys.argv[0]")
            });
        }
        Ok(Some(Argv0Swap { previous_bits }))
    }
}

unsafe fn restore_argv0_swap(_py: &PyToken<'_>, swap: Option<Argv0Swap>) {
    unsafe {
        let Some(swap) = swap else {
            return;
        };
        let saved = save_pending_exception(_py);
        if let Ok(Some(argv_bits)) = sys_list_attr_bits(_py, b"argv") {
            let argv_ptr = obj_from_bits(argv_bits)
                .as_ptr()
                .expect("validated sys.argv list");
            if crate::object::seq_access::locked_len(argv_ptr) != 0 {
                let _ = crate::object::list_mutation::replace_indices(
                    _py,
                    argv_ptr,
                    &[0],
                    std::slice::from_ref(&swap.previous_bits),
                );
            }
            dec_ref_bits(_py, argv_bits);
        }
        dec_ref_bits(_py, swap.previous_bits);
        restore_pending_exception(_py, saved);
    }
}

unsafe fn begin_sys_path_swap(_py: &PyToken<'_>, path: &str) -> Result<Option<SysPathSwap>, u64> {
    unsafe {
        let Some(path_list_bits) = sys_list_attr_bits(_py, b"path")? else {
            return Ok(None);
        };
        let path_list_ptr = obj_from_bits(path_list_bits)
            .as_ptr()
            .expect("validated sys.path list");
        let path_ptr = alloc_string(_py, path.as_bytes());
        if path_ptr.is_null() {
            dec_ref_bits(_py, path_list_bits);
            return Err(crate::builtins::exceptions::ExceptionSentinel::exception_sentinel());
        }
        let path_bits = MoltObject::from_ptr(path_ptr).bits();
        let inserted = crate::object::list_mutation::insert(_py, path_list_ptr, 0, path_bits);
        dec_ref_bits(_py, path_list_bits);
        if !inserted {
            dec_ref_bits(_py, path_bits);
            return Err(if exception_pending(_py) {
                MoltObject::none().bits()
            } else {
                raise_exception::<_>(_py, "RuntimeError", "failed to prepend sys.path")
            });
        }
        Ok(Some(SysPathSwap { path_bits }))
    }
}

unsafe fn restore_sys_path_swap(_py: &PyToken<'_>, swap: Option<SysPathSwap>) {
    unsafe {
        let Some(swap) = swap else {
            return;
        };
        let saved = save_pending_exception(_py);
        if let Ok(Some(path_list_bits)) = sys_list_attr_bits(_py, b"path") {
            let path_list_ptr = obj_from_bits(path_list_bits)
                .as_ptr()
                .expect("validated sys.path list");
            let index = crate::object::seq_access::with_borrowed(path_list_ptr, |items| {
                items.iter().position(|&bits| bits == swap.path_bits)
            });
            if let Some(index) = index {
                let _ = crate::object::list_mutation::remove_indices(
                    _py,
                    path_list_ptr,
                    std::slice::from_ref(&index),
                );
            }
            dec_ref_bits(_py, path_list_bits);
        }
        dec_ref_bits(_py, swap.path_bits);
        restore_pending_exception(_py, saved);
    }
}

pub(super) unsafe fn copy_dict_entries(_py: &PyToken<'_>, src_ptr: *mut u8, dst_ptr: *mut u8) {
    unsafe {
        let Some(source_order) = crate::object::ops_dict::dict_snapshot(
            _py,
            src_ptr,
            crate::object::ops_dict::DictSnapshotKind::Entries,
        ) else {
            return;
        };
        for idx in (0..source_order.len()).step_by(2) {
            let completed =
                dict_set_in_place(_py, dst_ptr, source_order[idx], source_order[idx + 1]);
            if !completed {
                return;
            }
        }
    }
}

pub(super) unsafe fn module_dict_ptr(_py: &PyToken<'_>, module_bits: u64) -> Result<*mut u8, u64> {
    unsafe {
        let Some(module_ptr) = obj_from_bits(module_bits).as_ptr() else {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "module execution expects module",
            ));
        };
        if object_type_id(module_ptr) != TYPE_ID_MODULE {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "module execution expects module",
            ));
        }
        let Some(dict_ptr) = obj_from_bits(module_dict_bits(module_ptr)).as_ptr() else {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "module dict missing",
            ));
        };
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "module dict missing",
            ));
        }
        Ok(dict_ptr)
    }
}

unsafe fn set_module_dict_name(
    _py: &PyToken<'_>,
    dict_ptr: *mut u8,
    name: &[u8],
    value_bits: u64,
) -> Result<(), u64> {
    unsafe {
        let key_ptr = alloc_string(_py, name);
        if key_ptr.is_null() {
            return Err(crate::builtins::exceptions::ExceptionSentinel::exception_sentinel());
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let completed = dict_set_in_place(_py, dict_ptr, key_bits, value_bits);
        dec_ref_bits(_py, key_bits);
        if !completed {
            Err(MoltObject::none().bits())
        } else {
            Ok(())
        }
    }
}

unsafe fn set_module_dict_text(
    _py: &PyToken<'_>,
    dict_ptr: *mut u8,
    name: &[u8],
    value: &str,
) -> Result<(), u64> {
    unsafe {
        let value_ptr = alloc_string(_py, value.as_bytes());
        if value_ptr.is_null() {
            return Err(crate::builtins::exceptions::ExceptionSentinel::exception_sentinel());
        }
        let value_bits = MoltObject::from_ptr(value_ptr).bits();
        let result = set_module_dict_name(_py, dict_ptr, name, value_bits);
        dec_ref_bits(_py, value_bits);
        result
    }
}

fn begin_sys_modules_swap(
    _py: &PyToken<'_>,
    run_name: &str,
    module_bits: u64,
) -> Result<Option<SysModulesSwap>, u64> {
    let Some(sys_bits) = interpreter_sys_module(_py).inspect(|bits| inc_ref_bits(_py, *bits))
    else {
        return Ok(None);
    };
    let modules_bits = sys_modules_dict_bits(_py, sys_bits);
    dec_ref_bits(_py, sys_bits);
    let Some(modules_bits) = modules_bits else {
        return if exception_pending(_py) {
            Err(MoltObject::none().bits())
        } else {
            Ok(None)
        };
    };
    let modules_ptr = obj_from_bits(modules_bits)
        .as_ptr()
        .expect("sys_modules_dict_bits validated dict");
    let key_ptr = alloc_string(_py, run_name.as_bytes());
    if key_ptr.is_null() {
        dec_ref_bits(_py, modules_bits);
        return Err(crate::builtins::exceptions::ExceptionSentinel::exception_sentinel());
    }
    let key_bits = MoltObject::from_ptr(key_ptr).bits();
    let previous_bits = unsafe { dict_get_in_place(_py, modules_ptr, key_bits) };
    let previous_bits = match previous_bits {
        Ok(value) => value,
        Err(()) => {
            dec_ref_bits(_py, key_bits);
            dec_ref_bits(_py, modules_bits);
            return Err(MoltObject::none().bits());
        }
    };
    if let Some(bits) = previous_bits {
        inc_ref_bits(_py, bits);
    }
    let completed = unsafe { dict_set_in_place(_py, modules_ptr, key_bits, module_bits) };
    if !completed {
        if let Some(bits) = previous_bits {
            dec_ref_bits(_py, bits);
        }
        dec_ref_bits(_py, key_bits);
        dec_ref_bits(_py, modules_bits);
        return Err(MoltObject::none().bits());
    }
    Ok(Some(SysModulesSwap {
        modules_bits,
        modules_ptr,
        key_bits,
        previous_bits,
    }))
}

fn restore_sys_modules_swap(_py: &PyToken<'_>, swap: Option<SysModulesSwap>) {
    let Some(swap) = swap else {
        return;
    };
    unsafe {
        if let Some(bits) = swap.previous_bits {
            // Restoration reports through the pending exception captured by
            // the caller after all swap references have been retired.
            let _ = dict_set_in_place(_py, swap.modules_ptr, swap.key_bits, bits);
            dec_ref_bits(_py, bits);
        } else {
            // The caller captures any cleanup failure after this restoration.
            let _ = dict_del_in_place(_py, swap.modules_ptr, swap.key_bits);
        }
    }
    dec_ref_bits(_py, swap.key_bits);
    dec_ref_bits(_py, swap.modules_bits);
}

fn save_pending_exception(_py: &PyToken<'_>) -> Option<u64> {
    if !exception_pending(_py) {
        return None;
    }
    let bits = molt_exception_last_pending();
    clear_exception(_py);
    Some(bits)
}

fn restore_pending_exception(_py: &PyToken<'_>, saved: Option<u64>) {
    let Some(bits) = saved else {
        return;
    };
    if exception_pending(_py) {
        clear_exception(_py);
    }
    if !obj_from_bits(bits).is_none() {
        let _ = crate::molt_exception_set_last(bits);
    }
    dec_ref_bits(_py, bits);
}

pub(super) fn on_module_publish(
    _py: &PyToken<'_>,
    name: &str,
    module_bits: u64,
) -> Result<(), u64> {
    if !crate::builtins::module_table::executing_private_body(_py, name) {
        return Ok(());
    }
    let runtime_id = runtime_state(_py) as *const _ as usize;
    let seed = EXECUTION_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        let ctx = stack.last_mut()?;
        if ctx.runtime_id != runtime_id || ctx.import_name != name || ctx.module_bits != 0 {
            return None;
        }
        inc_ref_bits(_py, module_bits);
        ctx.module_bits = module_bits;
        Some((
            ctx.init_globals_bits,
            ctx.alter_sys,
            ctx.run_name.clone(),
            ctx.metadata.clone(),
        ))
    });
    let Some((init_globals_bits, alter_sys, run_name, metadata)) = seed else {
        return Ok(());
    };

    let module_dict_ptr = unsafe { module_dict_ptr(_py, module_bits)? };
    if let Some(init_bits) = init_globals_bits {
        let Some(init_ptr) = obj_from_bits(init_bits)
            .as_ptr()
            .filter(|ptr| unsafe { object_type_id(*ptr) == TYPE_ID_DICT })
        else {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "init_globals must be dict",
            ));
        };
        unsafe {
            copy_dict_entries(_py, init_ptr, module_dict_ptr);
        }
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
    }

    // CPython overlays execution metadata after `init_globals`.  MODULE_NEW
    // has already initialized `__name__`, so this must happen at publication
    // rather than waiting for a later MODULE_SET_ATTR that may never exist.
    unsafe {
        set_module_dict_text(_py, module_dict_ptr, b"__name__", &run_name)?;
        if let ExecutionMetadata::ScriptFile(script_path) = &metadata {
            set_module_dict_text(_py, module_dict_ptr, b"__file__", script_path)?;
            let package = run_name
                .rsplit_once('.')
                .map(|(prefix, _)| prefix)
                .unwrap_or_default();
            set_module_dict_text(_py, module_dict_ptr, b"__package__", package)?;
            for name in [b"__spec__".as_slice(), b"__cached__", b"__loader__"] {
                if name == b"__cached__"
                    && crate::object::ops_sys::runtime_target_at_least(_py, 3, 15)
                {
                    continue;
                }
                set_module_dict_name(_py, module_dict_ptr, name, MoltObject::none().bits())?;
            }
        }
    }

    if alter_sys {
        let swap = begin_sys_modules_swap(_py, &run_name, module_bits)?;
        EXECUTION_STACK.with(|stack| {
            if let Some(ctx) = stack.borrow_mut().last_mut() {
                ctx.sys_modules_swap = swap;
            }
        });
    }
    Ok(())
}

/// Redirect the compiler initializer's single module allocation to the module
/// object supplied to `Loader.exec_module`. This preserves object identity and
/// makes body mutations (including partial state on exceptions) observable on
/// exactly the caller-owned module, as in CPython.
/// Defining globals of this exact active private body, never public membership.
pub(super) fn execution_owns_globals(py: &PyToken<'_>, name: &str, globals: u64) -> bool {
    if !crate::builtins::module_table::executing_private_body(py, name) {
        return false;
    }
    let runtime = runtime_state(py) as *const _ as usize;
    EXECUTION_STACK.with(|stack| {
        stack.borrow().last().is_some_and(|context| {
            context.runtime_id == runtime
                && context.import_name == name
                && obj_from_bits(context.module_bits)
                    .as_ptr()
                    .is_some_and(|module| unsafe {
                        object_type_id(module) == TYPE_ID_MODULE
                            && module_dict_bits(module) == globals
                    })
        })
    })
}

pub(super) fn module_new_target(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    if !crate::builtins::module_table::executing_private_body(_py, name) {
        return None;
    }
    let runtime_id = runtime_state(_py) as *const _ as usize;
    EXECUTION_STACK.with(|stack| {
        let stack = stack.borrow();
        let ctx = stack.last()?;
        if ctx.runtime_id != runtime_id || ctx.import_name != name || ctx.module_bits != 0 {
            return None;
        }
        let ExecutionMetadata::LoaderNamespace { module_bits } = &ctx.metadata else {
            return None;
        };
        inc_ref_bits(_py, *module_bits);
        Some(*module_bits)
    })
}

/// What one module metadata store writes while a compiled body executes.
pub(super) enum MetadataStore {
    /// The stored value itself: no execution context claims this store.
    Body,
    /// This value instead; `owned` when the store releases it afterwards.
    Value { bits: u64, owned: bool },
    /// Nothing: the loader's module lacks the attribute and keeps lacking it.
    Absent,
}

pub(super) fn module_metadata_store(
    _py: &PyToken<'_>,
    module_bits: u64,
    attr_bits: u64,
) -> Result<MetadataStore, u64> {
    let runtime_id = runtime_state(_py) as *const _ as usize;
    let Some(attr) = string_obj_to_owned(obj_from_bits(attr_bits)) else {
        return Ok(MetadataStore::Body);
    };
    enum MetadataOverride {
        Text(Option<String>),
        Existing,
    }
    let metadata_override = EXECUTION_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        let ctx = stack.last_mut()?;
        if ctx.runtime_id != runtime_id || ctx.module_bits != module_bits {
            return None;
        }
        let (bit, value) = match (&ctx.metadata, attr.as_str()) {
            (_, "__name__") => (
                EXECUTION_NAME_PENDING,
                MetadataOverride::Text(Some(ctx.run_name.clone())),
            ),
            (ExecutionMetadata::ScriptFile(path), "__file__") => (
                SCRIPT_FILE_PENDING,
                MetadataOverride::Text(Some(path.clone())),
            ),
            (ExecutionMetadata::ScriptFile(_), "__package__") => (
                SCRIPT_PACKAGE_PENDING,
                MetadataOverride::Text(Some(
                    ctx.run_name
                        .rsplit_once('.')
                        .map(|(package, _)| package.to_string())
                        .unwrap_or_default(),
                )),
            ),
            (ExecutionMetadata::ScriptFile(_), "__spec__") => {
                (SCRIPT_SPEC_PENDING, MetadataOverride::Text(None))
            }
            (ExecutionMetadata::ImportContainer(_), "__package__") => (
                SCRIPT_PACKAGE_PENDING,
                MetadataOverride::Text(Some(String::new())),
            ),
            (ExecutionMetadata::LoaderNamespace { .. }, "__file__") => {
                (SCRIPT_FILE_PENDING, MetadataOverride::Existing)
            }
            (ExecutionMetadata::LoaderNamespace { .. }, "__package__") => {
                (SCRIPT_PACKAGE_PENDING, MetadataOverride::Existing)
            }
            (ExecutionMetadata::LoaderNamespace { .. }, "__path__") => {
                (LOADER_PATH_PENDING, MetadataOverride::Existing)
            }
            (ExecutionMetadata::LoaderNamespace { .. }, "__spec__") => {
                (SCRIPT_SPEC_PENDING, MetadataOverride::Existing)
            }
            _ => return None,
        };
        if ctx.metadata_override_pending & bit == 0 {
            return None;
        }
        ctx.metadata_override_pending &= !bit;
        if bit == SCRIPT_SPEC_PENDING {
            // The compiler prologue stores `__file__`, `__package__` and a
            // located package's `__path__` before its spec admission.
            // Admission ends that window: a later store is the body's own.
            ctx.metadata_override_pending &=
                !(SCRIPT_FILE_PENDING | SCRIPT_PACKAGE_PENDING | LOADER_PATH_PENDING);
        }
        Some(value)
    });
    let Some(metadata_override) = metadata_override else {
        return Ok(MetadataStore::Body);
    };
    match metadata_override {
        MetadataOverride::Text(None) => Ok(MetadataStore::Value {
            bits: MoltObject::none().bits(),
            owned: false,
        }),
        MetadataOverride::Text(Some(text)) => {
            let ptr = alloc_string(_py, text.as_bytes());
            if ptr.is_null() {
                return Err(crate::builtins::exceptions::ExceptionSentinel::exception_sentinel());
            }
            Ok(MetadataStore::Value {
                bits: MoltObject::from_ptr(ptr).bits(),
                owned: true,
            })
        }
        MetadataOverride::Existing => unsafe {
            let dict_ptr = module_dict_ptr(_py, module_bits)?;
            let Some(bits) = dict_get_in_place(_py, dict_ptr, attr_bits)
                .map_err(|()| MoltObject::none().bits())?
            else {
                return Ok(MetadataStore::Absent);
            };
            inc_ref_bits(_py, bits);
            Ok(MetadataStore::Value { bits, owned: true })
        },
    }
}

/// Complete CPython's module metadata tuple once the compiler publishes the
/// generated `__spec__`.  The compiler owns spec construction; execution only
/// reads `spec.loader` and `spec.cached` once before user statements. Loader is
/// projected; cached is projected only before target 3.15, preserving that
/// version's observable getter without replacing an init_globals value.
pub(super) unsafe fn after_module_metadata_set(
    _py: &PyToken<'_>,
    module_bits: u64,
    attr_bits: u64,
    effective_val_bits: u64,
) -> Result<(), u64> {
    unsafe {
        enum SpecMetadataAction {
            Skip,
            Mirror,
            ImportContainer,
        }
        let runtime_id = runtime_state(_py) as *const _ as usize;
        let is_spec = crate::object::ops_format::string_obj_matches_bytes(
            obj_from_bits(attr_bits),
            b"__spec__",
        );
        if !is_spec {
            return Ok(());
        }
        let metadata = EXECUTION_STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            let ctx = stack.last_mut()?;
            if ctx.runtime_id != runtime_id
                || ctx.module_bits != module_bits
                || !ctx.spec_metadata_pending
            {
                return None;
            }
            ctx.spec_metadata_pending = false;
            Some(match &ctx.metadata {
                ExecutionMetadata::ScriptFile(_) => SpecMetadataAction::Skip,
                ExecutionMetadata::Module { .. } => SpecMetadataAction::Mirror,
                ExecutionMetadata::LoaderNamespace { .. } => SpecMetadataAction::Skip,
                ExecutionMetadata::ImportContainer(_) => SpecMetadataAction::ImportContainer,
            })
        });
        let Some(action) = metadata else {
            return Ok(());
        };
        if matches!(&action, SpecMetadataAction::Skip) {
            return Ok(());
        }
        if let SpecMetadataAction::ImportContainer = action {
            if obj_from_bits(effective_val_bits).is_none() {
                return Err(raise_exception::<_>(
                    _py,
                    "ImportError",
                    "run_path import container has no module spec",
                ));
            }
            let name_ptr = alloc_string(_py, b"name");
            let value_ptr = alloc_string(_py, b"__main__");
            if name_ptr.is_null() || value_ptr.is_null() {
                if !name_ptr.is_null() {
                    dec_ref_bits(_py, MoltObject::from_ptr(name_ptr).bits());
                }
                if !value_ptr.is_null() {
                    dec_ref_bits(_py, MoltObject::from_ptr(value_ptr).bits());
                }
                return Err(crate::builtins::exceptions::ExceptionSentinel::exception_sentinel());
            }
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let value_bits = MoltObject::from_ptr(value_ptr).bits();
            let result = crate::molt_set_attr_name(effective_val_bits, name_bits, value_bits);
            dec_ref_bits(_py, name_bits);
            dec_ref_bits(_py, value_bits);
            if !obj_from_bits(result).is_none() {
                dec_ref_bits(_py, result);
            }
            if exception_pending(_py) {
                return Err(MoltObject::none().bits());
            }
        }
        let dict_ptr = module_dict_ptr(_py, module_bits)?;
        for (spec_name, module_name) in [
            (b"loader".as_slice(), b"__loader__".as_slice()),
            (b"cached".as_slice(), b"__cached__".as_slice()),
        ] {
            let name_ptr = alloc_string(_py, spec_name);
            if name_ptr.is_null() {
                return Err(crate::builtins::exceptions::ExceptionSentinel::exception_sentinel());
            }
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let value_bits =
                molt_getattr_builtin(effective_val_bits, name_bits, MoltObject::none().bits());
            dec_ref_bits(_py, name_bits);
            if exception_pending(_py) {
                if !obj_from_bits(value_bits).is_none() {
                    dec_ref_bits(_py, value_bits);
                }
                return Err(MoltObject::none().bits());
            }
            // runpy still reads spec.cached in 3.15, but no longer overlays
            // __cached__; an existing init_globals value remains untouched.
            let set_result = if module_name == b"__cached__"
                && crate::object::ops_sys::runtime_target_at_least(_py, 3, 15)
            {
                Ok(())
            } else {
                set_module_dict_name(_py, dict_ptr, module_name, value_bits)
            };
            if !obj_from_bits(value_bits).is_none() {
                dec_ref_bits(_py, value_bits);
            }
            set_result?;
        }
        Ok(())
    }
}

fn capture_pending_exception(_py: &PyToken<'_>, first: &mut Option<u64>) {
    let Some(bits) = save_pending_exception(_py) else {
        return;
    };
    if first.is_none() {
        *first = Some(bits);
    } else {
        dec_ref_bits(_py, bits);
    }
}

fn pending_exception_is(_py: &PyToken<'_>, expected: &str) -> bool {
    if !exception_pending(_py) {
        return false;
    }
    let exc_bits = molt_exception_last_pending();
    let matches =
        crate::builtins::exceptions::exception_matches_builtin_name(_py, exc_bits, expected);
    if !obj_from_bits(exc_bits).is_none() {
        dec_ref_bits(_py, exc_bits);
    }
    matches
}

pub(crate) struct ModuleExecutionError {
    bits: u64,
    /// True only when dispatch could not find a compiler-emitted body.  An
    /// exception raised by an executing body is never classified as missing.
    missing_compiled_body: bool,
}

impl ModuleExecutionError {
    fn runtime(bits: u64) -> Self {
        Self {
            bits,
            missing_compiled_body: false,
        }
    }

    fn missing() -> Self {
        Self {
            bits: MoltObject::none().bits(),
            missing_compiled_body: true,
        }
    }

    /// Normalize the AOT admission boundary once for every public execution
    /// surface. Exceptions raised by an admitted body remain untouched;
    /// only dispatcher absence becomes the explicit compiled-closure error.
    pub(crate) fn into_import_error(self, _py: &PyToken<'_>, module_name: &str) -> u64 {
        if !self.missing_compiled_body {
            return self.bits;
        }
        if exception_pending(_py) {
            clear_exception(_py);
        }
        raise_exception::<_>(
            _py,
            "ImportError",
            &format!("module {module_name:?} has no compiler-emitted body in this binary"),
        )
    }

    pub(crate) fn is_missing_compiled_body(&self) -> bool {
        self.missing_compiled_body
    }

    /// Consume a dispatcher miss when the caller owns a distinct fallback
    /// lane (for example, a genuine dynamic C extension).  Body exceptions
    /// are never eligible for this path and must be propagated unchanged.
    pub(crate) fn discard_missing_compiled_body(self, _py: &PyToken<'_>) {
        debug_assert!(self.missing_compiled_body);
        if exception_pending(_py) {
            clear_exception(_py);
        }
        if !obj_from_bits(self.bits).is_none() {
            dec_ref_bits(_py, self.bits);
        }
    }

    pub(crate) fn into_bits(self) -> u64 {
        self.bits
    }
}

pub(crate) fn execute_compiled_module(
    _py: &PyToken<'_>,
    import_name: &str,
    run_name: &str,
    init_globals_bits: Option<u64>,
    alter_sys: bool,
    metadata: ExecutionMetadata,
) -> Result<u64, ModuleExecutionError> {
    if crate::builtins::module_table::module_execution_target_has_body(import_name) == Some(false) {
        return Err(ModuleExecutionError::missing());
    }
    let runtime_id = runtime_state(_py) as *const _ as usize;
    let needs_sys_transition_guard = alter_sys
        && EXECUTION_STACK.with(|stack| {
            !stack.borrow().iter().any(|context| {
                context.runtime_id == runtime_id && context.sys_transition_guard.is_some()
            })
        });
    let sys_transition_guard = if needs_sys_transition_guard {
        // Do not block another runtime thread while retaining the default GIL.
        // On a free-threaded runtime this guard is still the deterministic
        // authority for process-global sys.modules/sys.argv transitions.
        let gil_release = crate::concurrency::GilReleaseGuard::suspend();
        let guard = modules_state(_py)
            .sys_transition_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        drop(gil_release);
        Some(guard)
    } else {
        None
    };
    let mut path_swap = match &metadata {
        ExecutionMetadata::ImportContainer(path) => match unsafe { begin_sys_path_swap(_py, path) }
        {
            Ok(swap) => swap,
            Err(bits) => return Err(ModuleExecutionError::runtime(bits)),
        },
        _ => None,
    };
    let argv0 = match &metadata {
        ExecutionMetadata::Module { argv0 } => argv0.as_deref(),
        ExecutionMetadata::LoaderNamespace { .. } => None,
        ExecutionMetadata::ScriptFile(path) | ExecutionMetadata::ImportContainer(path) => {
            Some(path.as_str())
        }
    };
    let mut argv0_swap = match argv0 {
        Some(value) => match unsafe { begin_argv0_swap(_py, value) } {
            Ok(swap) => swap,
            Err(bits) => {
                unsafe {
                    restore_sys_path_swap(_py, path_swap.take());
                }
                return Err(ModuleExecutionError::runtime(bits));
            }
        },
        None => None,
    };
    let execution_name = crate::builtins::module_table::module_execution_target_name(import_name)
        .unwrap_or(import_name);
    if let Some(bits) = init_globals_bits {
        inc_ref_bits(_py, bits);
    }
    let loader_target_bits = match &metadata {
        ExecutionMetadata::LoaderNamespace { module_bits } => {
            inc_ref_bits(_py, *module_bits);
            Some(*module_bits)
        }
        _ => None,
    };
    let metadata_override_pending = match &metadata {
        ExecutionMetadata::ScriptFile(_) => SCRIPT_METADATA_PENDING,
        ExecutionMetadata::ImportContainer(_) => EXECUTION_NAME_PENDING | SCRIPT_PACKAGE_PENDING,
        ExecutionMetadata::Module { .. } => EXECUTION_NAME_PENDING,
        ExecutionMetadata::LoaderNamespace { .. } => LOADER_METADATA_PENDING,
    };
    EXECUTION_STACK.with(|stack| {
        stack.borrow_mut().push(ExecutionContext {
            runtime_id,
            sys_transition_guard,
            import_name: execution_name.to_string(),
            run_name: run_name.to_string(),
            init_globals_bits,
            alter_sys,
            metadata,
            module_bits: 0,
            sys_modules_swap: None,
            metadata_override_pending,
            spec_metadata_pending: true,
        });
    });

    let dispatched = crate::builtins::module_table::execute_private_body(_py, execution_name);
    let ctx = EXECUTION_STACK.with(|stack| stack.borrow_mut().pop().expect("execution context"));
    let result = if ctx.module_bits == 0 {
        MoltObject::none().bits()
    } else {
        ctx.module_bits
    };
    if dispatched.is_ok() && ctx.module_bits == 0 && !exception_pending(_py) {
        let _ = raise_exception::<u64>(
            _py,
            "ModuleNotFoundError",
            &format!("No module named {execution_name:?}"),
        );
    }
    let missing_compiled_body =
        ctx.module_bits == 0 && pending_exception_is(_py, "ModuleNotFoundError");
    if let Some(bits) = ctx.init_globals_bits {
        dec_ref_bits(_py, bits);
    }
    if let Some(bits) = loader_target_bits {
        dec_ref_bits(_py, bits);
    }
    let execution_exception = save_pending_exception(_py);
    let mut cleanup_exception = None;
    restore_sys_modules_swap(_py, ctx.sys_modules_swap);
    capture_pending_exception(_py, &mut cleanup_exception);
    let final_exception = if execution_exception.is_some() {
        if let Some(bits) = cleanup_exception {
            dec_ref_bits(_py, bits);
        }
        execution_exception
    } else {
        cleanup_exception
    };
    restore_pending_exception(_py, final_exception);
    unsafe {
        restore_argv0_swap(_py, argv0_swap.take());
        restore_sys_path_swap(_py, path_swap.take());
    }

    // Keep serialization custody alive through every public run-name/sys swap
    // restoration. Private execution never borrowed an ordinary import row.  The field is otherwise intentionally unread.
    let _sys_transition_guard = ctx.sys_transition_guard;

    if exception_pending(_py) || obj_from_bits(result).is_none() {
        if !obj_from_bits(result).is_none() {
            dec_ref_bits(_py, result);
        }
        Err(ModuleExecutionError {
            bits: MoltObject::none().bits(),
            missing_compiled_body,
        })
    } else {
        Ok(result)
    }
}
