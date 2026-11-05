//! Runtime-owned default path finders (HF-737).
//!
//! `PathFinder`, `FileFinder` and the two default path hooks exist from
//! bootstrap, so `sys.meta_path` and `sys.path_hooks` hold CPython's defaults
//! before user code runs. These bodies port the former Python bodies of
//! `importlib/machinery.py`; the design and its invariants are in
//! `docs/design/foundation/import_native_default_finders.md`.

use super::*;
use crate::BuiltinExc;
use crate::builtins::exceptions::ExceptionValue;

/// Position of each class that the `importlib.machinery` body declares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in super::super) enum FileLoaderKind {
    Extension = 0,
    Source = 1,
    Sourceless = 2,
    ZipSource = 3,
    NamespacePath = 4,
}

const DECLARED_FILE_LOADER_NAMES: [crate::NameId; 5] = [
    crate::NameId::ExtensionFileLoader,
    crate::NameId::SourceFileLoader,
    crate::NameId::SourcelessFileLoader,
    crate::NameId::ZipSourceLoader,
    crate::NameId::PrivateNamespacePath,
];

fn none_bits() -> u64 {
    MoltObject::none().bits()
}

fn pending_error(py: &PyToken<'_>) -> Result<(), u64> {
    if exception_pending(py) {
        Err(none_bits())
    } else {
        Ok(())
    }
}

/// Adopt one call result, failing when the call left an exception pending.
fn checked<'a, 'py>(py: &'a PyToken<'py>, bits: u64) -> Result<ExceptionValue<'a, 'py>, u64> {
    let value = ExceptionValue::adopt(py, bits);
    pending_error(py)?;
    Ok(value)
}

fn attribute<'a, 'py>(
    py: &'a PyToken<'py>,
    object: u64,
    name: crate::NameId,
) -> Result<ExceptionValue<'a, 'py>, u64> {
    let key = name.bits(py);
    if key == 0 {
        return Err(none_bits());
    }
    let bits =
        crate::builtins::attributes::get_attr_name(py, object, key).map_err(|()| none_bits())?;
    Ok(ExceptionValue::adopt(py, bits))
}

/// Read a member of the interpreter `sys` module at the moment of use, as the
/// Python bodies read `_sys.<name>` at each use.
fn sys_member<'a, 'py>(
    py: &'a PyToken<'py>,
    name: crate::NameId,
) -> Result<ExceptionValue<'a, 'py>, u64> {
    let sys = importlib_system_module(py)?;
    attribute(py, sys.bits(), name)
}

fn set_attribute(
    py: &PyToken<'_>,
    object: u64,
    name: crate::NameId,
    value: u64,
) -> Result<(), u64> {
    importlib_set_attr(py, object, name, value)
}

fn is_none(bits: u64) -> bool {
    obj_from_bits(bits).is_none()
}

fn truth(py: &PyToken<'_>, bits: u64) -> Result<bool, u64> {
    crate::object::ops::truth_value(py, obj_from_bits(bits)).map_err(|()| none_bits())
}

fn is_str_instance(py: &PyToken<'_>, bits: u64) -> Result<bool, u64> {
    let class = crate::builtins::classes::builtin_classes(py).str;
    let result = crate::molt_isinstance(bits, class);
    pending_error(py)?;
    Ok(obj_from_bits(result).as_bool().unwrap_or(false))
}

/// Clear a pending exception of `kind` (or a subclass); leave any other one.
fn take_pending(py: &PyToken<'_>, kind: BuiltinExc) -> bool {
    let exception = crate::builtins::exceptions::molt_exception_last_pending();
    let matched = crate::builtins::exceptions::exception_matches_kind(py, exception, kind);
    dec_ref_bits(py, exception);
    if matched {
        clear_exception(py);
    }
    matched
}

// --- The declared `importlib.machinery` classes ---------------------------

fn machinery_provider() -> Option<&'static str> {
    crate::builtins::module_table::module_registry()?
        .compiled_spec(crate::NameId::ImportlibMachinery.as_str())
        .filter(|spec| spec.is_stdlib_provider)
        .map(|spec| spec.provider_name)
}

fn machinery_unavailable(py: &PyToken<'_>) -> u64 {
    raise_exception::<u64>(
        py,
        BuiltinExc::RuntimeError,
        "runtime support module unavailable: importlib.machinery",
    )
}

fn declared_file_loaders<'a, 'py>(py: &'a PyToken<'py>) -> Option<ExceptionValue<'a, 'py>> {
    let bits = platform_state(py).file_loader_types.load(Ordering::Acquire);
    (bits != 0).then(|| ExceptionValue::pin(py, bits))
}

/// The loader tuple, running the admitted machinery body once when no body has
/// declared yet. The public cache entry is never read for the classes: a
/// replaced or `None` entry leads to a private execution of the same body.
pub(in super::super) fn importlib_file_loader_types<'a, 'py>(
    py: &'a PyToken<'py>,
) -> Result<ExceptionValue<'a, 'py>, u64> {
    if let Some(loaders) = declared_file_loaders(py) {
        return Ok(loaders);
    }
    let Some(provider) = machinery_provider() else {
        return Err(machinery_unavailable(py));
    };
    let _execution = crate::builtins::module_table::ModuleExecutionGuard::acquire(py, provider)
        .map_err(|failure| failure.into_exception(py, provider))?;
    if let Some(loaders) = declared_file_loaders(py) {
        return Ok(loaders);
    }
    match crate::builtins::modules::public_module_cache_lookup(py, provider)? {
        crate::builtins::modules::PublicModuleCache::Present(bits) => {
            drop(ExceptionValue::adopt(py, bits));
        }
        crate::builtins::modules::PublicModuleCache::Missing => {
            let key = ExceptionValue::adopt(py, alloc_str_bits(py, provider)?);
            match crate::builtins::modules::catalog_import_attempt(key.bits())? {
                crate::builtins::modules::ModuleImportOutcome::Imported(bits) => {
                    drop(ExceptionValue::adopt(py, bits));
                }
                crate::builtins::modules::ModuleImportOutcome::Missing { .. } => {
                    return Err(machinery_unavailable(py));
                }
            }
        }
    }
    if let Some(loaders) = declared_file_loaders(py) {
        return Ok(loaders);
    }
    // The public entry is `None` or a stand-in: run the admitted body in its
    // private namespace without changing any public entry.
    let module = execute_compiled_module(
        py,
        provider,
        provider,
        None,
        false,
        ExecutionMetadata::Module { argv0: None },
    )
    .map_err(|error| error.into_import_error(py, provider))?;
    drop(ExceptionValue::adopt(py, module));
    declared_file_loaders(py).ok_or_else(|| machinery_unavailable(py))
}

/// Borrowed class of `kind` from an owned loader tuple.
pub(in super::super) fn file_loader_class(loaders: u64, kind: FileLoaderKind) -> u64 {
    let pointer = obj_from_bits(loaders)
        .as_ptr()
        .expect("declared file loaders are a tuple");
    unsafe {
        crate::object::seq_access::with_immutable_tuple_slice(pointer, |items| items[kind as usize])
    }
    .expect("declared file loaders are a tuple")
}

fn declare_file_loader_types(py: &PyToken<'_>, namespace: u64) -> Result<(), u64> {
    let Some(provider) = machinery_provider() else {
        return Err(machinery_unavailable(py));
    };
    if !crate::builtins::modules::module_body_owns_globals(py, provider, namespace) {
        return Err(raise_exception::<u64>(
            py,
            BuiltinExc::RuntimeError,
            "file loader declaration does not own the importlib.machinery globals",
        ));
    }
    let dictionary = obj_from_bits(namespace)
        .as_ptr()
        .filter(|pointer| unsafe { object_type_id(*pointer) } == TYPE_ID_DICT)
        .ok_or_else(|| {
            raise_exception::<u64>(
                py,
                BuiltinExc::TypeError,
                "file loader declaration requires a namespace dictionary",
            )
        })?;
    let mut classes = [0u64; DECLARED_FILE_LOADER_NAMES.len()];
    for (slot, name) in classes.iter_mut().zip(DECLARED_FILE_LOADER_NAMES) {
        let key = name.bits(py);
        if key == 0 {
            return Err(none_bits());
        }
        let value = unsafe { dict_get_in_place(py, dictionary, key) }.map_err(|()| none_bits())?;
        let Some(value) = value.filter(|value| {
            obj_from_bits(*value)
                .as_ptr()
                .is_some_and(|pointer| unsafe { object_type_id(pointer) } == TYPE_ID_TYPE)
        }) else {
            return Err(raise_exception::<u64>(
                py,
                BuiltinExc::TypeError,
                &format!("importlib.machinery.{} must be a class", name.as_str()),
            ));
        };
        *slot = value;
    }
    // Record the extension-admission role on the actual declared class.
    // Names, facade rebinding and sys.modules removal cannot erase it.
    // Declaring this role only adds admission checks; it never authorizes loading.
    unsafe {
        crate::object::class_storage::class_declare(
            obj_from_bits(classes[FileLoaderKind::Extension as usize])
                .as_ptr()
                .unwrap(),
            crate::object::class_storage::ClassDeclaration::ExtensionLoader,
        );
    }
    let tuple = alloc_tuple(py, &classes);
    if tuple.is_null() {
        return Err(none_bits());
    }
    let previous = platform_state(py)
        .file_loader_types
        .swap(MoltObject::from_ptr(tuple).bits(), Ordering::AcqRel);
    if previous != 0 {
        dec_ref_bits(py, previous);
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_file_loader_types_declare(namespace_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        declare_file_loader_types(py, namespace_bits).map_or_else(|error| error, |()| none_bits())
    })
}

/// CPython's private `_NamespacePath._epoch` (bpo-45703), owned natively so
/// `PathFinder.invalidate_caches` needs no facade.
#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_namespace_path_epoch() -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let epoch = platform_state(py)
            .namespace_path_epoch
            .load(Ordering::Relaxed);
        MoltObject::from_int(epoch as i64).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_path_finder_types() -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let path_finder = crate::builtins::types::path_finder_class(py);
        let file_finder = crate::builtins::types::file_finder_class(py);
        if path_finder == 0 || file_finder == 0 || exception_pending(py) {
            return none_bits();
        }
        let tuple = alloc_tuple(py, &[path_finder, file_finder]);
        if tuple.is_null() {
            return none_bits();
        }
        MoltObject::from_ptr(tuple).bits()
    })
}

// --- PathFinder ------------------------------------------------------------

/// `os.getcwd()` for a `""` path entry; `None` when the directory is gone.
fn current_directory<'a, 'py>(
    py: &'a PyToken<'py>,
) -> Result<Option<ExceptionValue<'a, 'py>>, u64> {
    let bits = crate::molt_os_getcwd();
    if exception_pending(py) {
        if !is_none(bits) {
            dec_ref_bits(py, bits);
        }
        if take_pending(py, BuiltinExc::FileNotFoundError) {
            return Ok(None);
        }
        return Err(none_bits());
    }
    Ok(Some(ExceptionValue::adopt(py, bits)))
}

/// `sys.path_importer_cache[entry]`, filled from `sys.path_hooks` on a miss.
fn path_entry_finder<'a, 'py>(
    py: &'a PyToken<'py>,
    entry: u64,
) -> Result<ExceptionValue<'a, 'py>, u64> {
    let cache = sys_member(py, crate::NameId::PathImporterCache)?;
    if let Some(bits) = crate::builtins::modules::lookup_namespace_item(py, cache.bits(), entry)
        .map_err(|()| none_bits())?
    {
        return Ok(ExceptionValue::adopt(py, bits));
    }
    drop(cache);
    let mut finder = ExceptionValue::adopt(py, none_bits());
    let hooks = sys_member(py, crate::NameId::PathHooks)?;
    let mut iterator =
        crate::object::iterable::OwnedIterator::new(py, hooks.bits()).ok_or_else(none_bits)?;
    while let Some(hook) = iterator.next().map_err(|_| none_bits())? {
        let hook = ExceptionValue::adopt(py, hook);
        let result = ExceptionValue::adopt(py, unsafe { call_callable1(py, hook.bits(), entry) });
        if exception_pending(py) {
            if take_pending(py, BuiltinExc::ImportError) {
                continue;
            }
            return Err(none_bits());
        }
        finder = result;
        break;
    }
    drop(iterator);
    drop(hooks);
    let cache = sys_member(py, crate::NameId::PathImporterCache)?;
    // The subscription protocol returns its borrowed receiver, not a new owner.
    let _ = crate::molt_store_index(cache.bits(), entry, finder.bits());
    pending_error(py)?;
    Ok(finder)
}

fn path_finder_get_spec(
    py: &PyToken<'_>,
    fullname: u64,
    path: u64,
    target: u64,
) -> Result<u64, u64> {
    let mut portions: Vec<ExceptionValue<'_, '_>> = Vec::new();
    let empty = ExceptionValue::adopt(py, alloc_str_bits(py, "")?);
    let mut entries =
        crate::object::iterable::OwnedIterator::new(py, path).ok_or_else(none_bits)?;
    while let Some(entry) = entries.next().map_err(|_| none_bits())? {
        let mut entry = ExceptionValue::adopt(py, entry);
        if !is_str_instance(py, entry.bits())? {
            continue;
        }
        let equal = checked(py, crate::molt_eq(entry.bits(), empty.bits()))?;
        if truth(py, equal.bits())? {
            match current_directory(py)? {
                Some(directory) => entry = directory,
                None => continue,
            }
        }
        let finder = path_entry_finder(py, entry.bits())?;
        if is_none(finder.bits()) {
            continue;
        }
        let find_spec = attribute(py, finder.bits(), crate::NameId::FindSpec)?;
        let spec = checked(py, unsafe {
            call_callable2(py, find_spec.bits(), fullname, target)
        })?;
        if is_none(spec.bits()) {
            continue;
        }
        let name = string_arg_from_bits(py, fullname, "module name")?;
        importlib_enforce_extension_spec_object_boundary(py, &name, spec.bits())?;
        let loader = attribute(py, spec.bits(), crate::NameId::Loader)?;
        if !is_none(loader.bits()) {
            return Ok(spec.into_bits());
        }
        let locations = attribute(py, spec.bits(), crate::NameId::SubmoduleSearchLocations)?;
        if is_none(locations.bits()) {
            return Err(raise_exception::<u64>(
                py,
                BuiltinExc::ImportError,
                "spec missing loader",
            ));
        }
        let mut items = crate::object::iterable::OwnedIterator::new(py, locations.bits())
            .ok_or_else(none_bits)?;
        while let Some(item) = items.next().map_err(|_| none_bits())? {
            portions.push(ExceptionValue::adopt(py, item));
        }
    }
    let portion_bits: Vec<u64> = portions.iter().map(ExceptionValue::bits).collect();
    let list = alloc_list(py, &portion_bits);
    if list.is_null() {
        return Err(none_bits());
    }
    let list = ExceptionValue::adopt(py, MoltObject::from_ptr(list).bits());
    let spec = ExceptionValue::adopt(
        py,
        crate::builtins::types::module_spec::alloc_module_spec(
            py,
            fullname,
            none_bits(),
            none_bits(),
            none_bits(),
        )
        .ok_or_else(none_bits)?,
    );
    set_attribute(
        py,
        spec.bits(),
        crate::NameId::SubmoduleSearchLocations,
        list.bits(),
    )?;
    Ok(spec.into_bits())
}

fn path_finder_find_spec(
    py: &PyToken<'_>,
    class: u64,
    fullname: u64,
    path: u64,
    target: u64,
) -> Result<u64, u64> {
    let path = if is_none(path) {
        sys_member(py, crate::NameId::Path)?
    } else {
        ExceptionValue::pin(py, path)
    };
    let get_spec = attribute(py, class, crate::NameId::PrivateGetSpec)?;
    let spec = checked(py, unsafe {
        call_callable3(py, get_spec.bits(), fullname, path.bits(), target)
    })?;
    drop(get_spec);
    if is_none(spec.bits()) {
        return Ok(none_bits());
    }
    let loader = attribute(py, spec.bits(), crate::NameId::Loader)?;
    if !is_none(loader.bits()) {
        return Ok(spec.into_bits());
    }
    let portions = attribute(py, spec.bits(), crate::NameId::SubmoduleSearchLocations)?;
    if !truth(py, portions.bits())? {
        return Ok(none_bits());
    }
    set_attribute(py, spec.bits(), crate::NameId::Origin, none_bits())?;
    let loaders = importlib_file_loader_types(py)?;
    let namespace_path = file_loader_class(loaders.bits(), FileLoaderKind::NamespacePath);
    let get_spec = attribute(py, class, crate::NameId::PrivateGetSpec)?;
    let wrapped = checked(py, unsafe {
        call_callable3(
            py,
            namespace_path,
            fullname,
            portions.bits(),
            get_spec.bits(),
        )
    })?;
    set_attribute(
        py,
        spec.bits(),
        crate::NameId::SubmoduleSearchLocations,
        wrapped.bits(),
    )?;
    Ok(spec.into_bits())
}

/// How `PathFinder.invalidate_caches` classifies a cache key as an import
/// root. The flavor is a parameter so one host can test both rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in super::super) enum ImportRootFlavor {
    Posix,
    Windows,
}

impl ImportRootFlavor {
    fn for_target() -> Self {
        if cfg!(all(target_os = "windows", not(target_arch = "wasm32"))) {
            Self::Windows
        } else {
            Self::Posix
        }
    }
}

/// CPython's import-root predicate (not `os.path.isabs`). POSIX calls the
/// key's own `startswith("/")`, so non-text keys raise as in the facade.
/// Windows: drive plus root from the native parser, `/` read as `\`,
/// absolute when longer than one character and UNC or separator-terminated.
pub(in super::super) fn import_root_is_absolute(
    py: &PyToken<'_>,
    key: u64,
    flavor: ImportRootFlavor,
) -> Result<bool, u64> {
    match flavor {
        ImportRootFlavor::Posix => {
            let method = attribute(py, key, crate::NameId::Startswith)?;
            let separator = ExceptionValue::adopt(py, alloc_str_bits(py, "/")?);
            let result = checked(py, unsafe {
                call_callable1(py, method.bits(), separator.bits())
            })?;
            truth(py, result.bits())
        }
        ImportRootFlavor::Windows => {
            if !truth(py, key)? {
                return Ok(false);
            }
            let parts = checked(
                py,
                crate::molt_path_splitroot(
                    key,
                    MoltObject::from_bool(false).bits(),
                    MoltObject::from_bool(true).bits(),
                    MoltObject::from_bool(false).bits(),
                ),
            )?;
            let pointer = obj_from_bits(parts.bits())
                .as_ptr()
                .filter(|pointer| unsafe { object_type_id(*pointer) } == TYPE_ID_TUPLE)
                .ok_or_else(|| {
                    raise_exception::<u64>(
                        py,
                        BuiltinExc::RuntimeError,
                        "path root split did not return a tuple",
                    )
                })?;
            let (drive, root) = unsafe {
                crate::object::seq_access::with_immutable_tuple_slice(pointer, |items| {
                    (items[0], items[1])
                })
            }
            .ok_or_else(none_bits)?;
            let text = |bits: u64| crate::object::ops_format::string_obj_bytes(obj_from_bits(bits));
            let bytes = |bits: u64| {
                obj_from_bits(bits)
                    .as_ptr()
                    .filter(|pointer| unsafe { object_type_id(*pointer) } == TYPE_ID_BYTES)
                    .map(|pointer| {
                        unsafe { crate::bytes_like_slice(pointer) }
                            .unwrap_or(&[])
                            .to_vec()
                    })
            };
            let (mut joined, units) = if let (Some(drive), Some(root)) = (text(drive), text(root)) {
                let joined = [drive, root].concat();
                let units = String::from_utf8_lossy(&joined).chars().count();
                (joined, units)
            } else if let (Some(drive), Some(root)) = (bytes(drive), bytes(root)) {
                let joined = [drive, root].concat();
                let units = joined.len();
                (joined, units)
            } else {
                return Err(raise_exception::<u64>(
                    py,
                    BuiltinExc::RuntimeError,
                    "path root split returned mixed text and bytes",
                ));
            };
            for byte in &mut joined {
                if *byte == b'/' {
                    *byte = b'\\';
                }
            }
            Ok(units > 1 && (joined.starts_with(b"\\\\") || joined.ends_with(b"\\")))
        }
    }
}

fn unpack_pair<'a, 'py>(
    py: &'a PyToken<'py>,
    item: u64,
) -> Result<(ExceptionValue<'a, 'py>, ExceptionValue<'a, 'py>), u64> {
    let mut iterator =
        crate::object::iterable::OwnedIterator::new(py, item).ok_or_else(none_bits)?;
    let mut values = Vec::with_capacity(2);
    while let Some(value) = iterator.next().map_err(|_| none_bits())? {
        values.push(ExceptionValue::adopt(py, value));
        if values.len() > 2 {
            return Err(raise_exception::<u64>(
                py,
                BuiltinExc::ValueError,
                "too many values to unpack (expected 2)",
            ));
        }
    }
    if values.len() != 2 {
        return Err(raise_exception::<u64>(
            py,
            BuiltinExc::ValueError,
            &format!(
                "not enough values to unpack (expected 2, got {})",
                values.len()
            ),
        ));
    }
    let second = values.pop().unwrap();
    let first = values.pop().unwrap();
    Ok((first, second))
}

fn path_finder_invalidate_caches_with(
    py: &PyToken<'_>,
    flavor: ImportRootFlavor,
) -> Result<(), u64> {
    let snapshot = {
        let cache = sys_member(py, crate::NameId::PathImporterCache)?;
        let items_method = attribute(py, cache.bits(), crate::NameId::Items)?;
        let items = checked(py, unsafe { call_callable0(py, items_method.bits()) })?;
        let list = unsafe { crate::object::ops::list_from_iter_bits(py, items.bits()) }
            .ok_or_else(none_bits)?;
        ExceptionValue::adopt(py, list)
    };
    let mut entries =
        crate::object::iterable::OwnedIterator::new(py, snapshot.bits()).ok_or_else(none_bits)?;
    while let Some(item) = entries.next().map_err(|_| none_bits())? {
        let item = ExceptionValue::adopt(py, item);
        let (key, finder) = unpack_pair(py, item.bits())?;
        if is_none(finder.bits()) || !import_root_is_absolute(py, key.bits(), flavor)? {
            let cache = sys_member(py, crate::NameId::PathImporterCache)?;
            // The deletion protocol returns its borrowed receiver, not a new owner.
            let _ = crate::molt_del_index(cache.bits(), key.bits());
            pending_error(py)?;
            continue;
        }
        let key_name = crate::NameId::InvalidateCaches.bits(py);
        if key_name == 0 {
            return Err(none_bits());
        }
        let Some(present) = getattr_optional_bits(py, finder.bits(), key_name)? else {
            continue;
        };
        drop(ExceptionValue::adopt(py, present));
        let method = attribute(py, finder.bits(), crate::NameId::InvalidateCaches)?;
        checked(py, unsafe { call_callable0(py, method.bits()) })?;
    }
    // Namespace paths recompute after this (bpo-45703).
    platform_state(py)
        .namespace_path_epoch
        .fetch_add(1, Ordering::Relaxed);
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_path_finder_find_spec(
    class_bits: u64,
    fullname_bits: u64,
    path_bits: u64,
    target_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        path_finder_find_spec(py, class_bits, fullname_bits, path_bits, target_bits)
            .unwrap_or_else(|error| error)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_path_finder_get_spec(
    _class_bits: u64,
    fullname_bits: u64,
    path_bits: u64,
    target_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        path_finder_get_spec(py, fullname_bits, path_bits, target_bits)
            .unwrap_or_else(|error| error)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_path_finder_invalidate_caches() -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        path_finder_invalidate_caches_with(py, ImportRootFlavor::for_target())
            .map_or_else(|error| error, |()| none_bits())
    })
}

// --- FileFinder and the default hooks --------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_file_finder_init(
    self_bits: u64,
    path_bits: u64,
    details_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let result = (|| -> Result<(), u64> {
            let text = checked(py, crate::molt_str_from_obj(path_bits))?;
            set_attribute(py, self_bits, crate::NameId::Path, text.bits())?;
            let details = ExceptionValue::adopt(
                py,
                unsafe { crate::object::ops::tuple_from_iter_bits(py, details_bits) }
                    .ok_or_else(none_bits)?,
            );
            set_attribute(
                py,
                self_bits,
                crate::NameId::PrivateLoaderDetails,
                details.bits(),
            )
        })();
        result.map_or_else(|error| error, |()| none_bits())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_file_finder_repr(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let result = (|| -> Result<u64, u64> {
            let path = attribute(py, self_bits, crate::NameId::Path)?;
            let rendered = checked(py, crate::molt_repr_from_obj(path.bits()))?;
            let rendered =
                crate::object::ops_format::string_obj_bytes(obj_from_bits(rendered.bits()))
                    .ok_or_else(none_bits)?;
            let text = [b"<MoltFileFinder path=".as_slice(), &rendered, b">"].concat();
            let pointer = alloc_string(py, &text);
            if pointer.is_null() {
                return Err(none_bits());
            }
            Ok(MoltObject::from_ptr(pointer).bits())
        })();
        result.unwrap_or_else(|error| error)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_file_finder_find_spec(
    self_bits: u64,
    fullname_bits: u64,
    _target_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let result = (|| -> Result<u64, u64> {
            let path = attribute(py, self_bits, crate::NameId::Path)?;
            let fullname = string_arg_from_bits(py, fullname_bits, "module name")?;
            let path = string_arg_from_bits(py, path.bits(), "path")?;
            crate::require_operation::<u64>(
                py,
                crate::OperationId::ImportlibFilefinderFindSpec,
                AuditArgs::None,
            )?;
            match importlib_resolve_spec_in_paths(py, &fullname, &[path], true)? {
                Some(resolution) => importlib_find_spec_object_bits(py, &fullname, &resolution),
                None => Ok(none_bits()),
            }
        })();
        result.unwrap_or_else(|error| error)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_file_finder_invalidate_caches(_self_bits: u64) -> u64 {
    // The native leaf has no directory-result cache. PathFinder owns the
    // separate path-to-finder cache and never re-enters it from this leaf.
    none_bits()
}

/// `FileFinder.path_hook(*loader_details)`: a hook that closes over the class
/// it was called on and the details.
pub(crate) fn file_finder_path_hook(
    py: &PyToken<'_>,
    class: u64,
    details: u64,
) -> Result<u64, u64> {
    use crate::builtins::functions::native_callable::{
        NativeCallableKind, NativeCallableName, NativeCallableSpec,
    };
    let context = alloc_tuple(py, &[class, details]);
    if context.is_null() {
        return Err(none_bits());
    }
    let context = ExceptionValue::adopt(py, MoltObject::from_ptr(context).bits());
    let spec = NativeCallableSpec {
        kind: NativeCallableKind::Function,
        name: Some(NativeCallableName::Known(
            crate::NameId::PathHookForFileFinder,
        )),
        ..NativeCallableSpec::uncached_function()
    };
    let function = crate::builtins::methods::alloc_builtin_function_with_signature(
        py,
        spec,
        crate::builtins::functions::runtime_fn_addr(
            "molt_importlib_file_finder_path_hook_call",
            molt_importlib_file_finder_path_hook_call as *const (),
        ),
        crate::intrinsics::public_objects_generated::PublicSignatureSpec {
            arity: 1,
            positional: &[b"path"],
            posonly: 0,
            vararg: false,
            varkw: false,
            defaults: &[],
            kwonly: &[],
            kwdefaults: &[],
        },
        &[],
    );
    if function == 0 || exception_pending(py) {
        if function != 0 {
            dec_ref_bits(py, function);
        }
        return Err(none_bits());
    }
    unsafe {
        crate::function_set_closure_bits(
            py,
            obj_from_bits(function).as_ptr().unwrap(),
            context.bits(),
            crate::FunctionCallAbi::OpaqueContextFirst,
        );
    }
    Ok(function)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_file_finder_path_hook(class_bits: u64, details_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let result = (|| -> Result<u64, u64> {
            let details = ExceptionValue::adopt(
                py,
                unsafe { crate::object::ops::tuple_from_iter_bits(py, details_bits) }
                    .ok_or_else(none_bits)?,
            );
            file_finder_path_hook(py, class_bits, details.bits())
        })();
        result.unwrap_or_else(|error| error)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_file_finder_path_hook_call(
    context_bits: u64,
    path_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let result = (|| -> Result<u64, u64> {
            let context = obj_from_bits(context_bits)
                .as_ptr()
                .filter(|pointer| unsafe { object_type_id(*pointer) } == TYPE_ID_TUPLE)
                .ok_or_else(none_bits)?;
            let (class, details) = unsafe {
                crate::object::seq_access::with_immutable_tuple_slice(context, |items| {
                    (items[0], items[1])
                })
            }
            .ok_or_else(none_bits)?;
            let directory = is_str_instance(py, path_bits)? && {
                let result = checked(py, crate::molt_path_isdir(path_bits))?;
                truth(py, result.bits())?
            };
            if !directory {
                return Err(crate::builtins::exceptions::raise_import_error(
                    py,
                    b"only directories are supported",
                    none_bits(),
                    path_bits,
                    none_bits(),
                ));
            }
            let mut arguments = vec![path_bits];
            let details = obj_from_bits(details).as_ptr().ok_or_else(none_bits)?;
            unsafe {
                crate::object::seq_access::with_immutable_tuple_slice(details, |items| {
                    arguments.extend_from_slice(items)
                });
            }
            let finder = call_callable_positional(py, class, &arguments)?;
            Ok(checked(py, finder)?.into_bits())
        })();
        result.unwrap_or_else(|error| error)
    })
}

/// The archive hook: rejects a path that names no `.zip`, `.whl` or `.egg`
/// archive, and otherwise imports `zipimport` only now.
fn zip_path_hook(py: &PyToken<'_>, path: u64) -> Result<u64, u64> {
    let lower_method = attribute(py, path, crate::NameId::Lower)?;
    let lower = checked(py, unsafe { call_callable0(py, lower_method.bits()) })?;
    let replace_method = attribute(py, lower.bits(), crate::NameId::Replace)?;
    let backslash = ExceptionValue::adopt(py, alloc_str_bits(py, "\\")?);
    let slash = ExceptionValue::adopt(py, alloc_str_bits(py, "/")?);
    let normalized = checked(py, unsafe {
        call_callable2(py, replace_method.bits(), backslash.bits(), slash.bits())
    })?;
    let archive = crate::object::ops_format::string_obj_bytes(obj_from_bits(normalized.bits()))
        .is_some_and(|text| {
            [b".zip".as_slice(), b".whl", b".egg"].iter().any(|suffix| {
                text.ends_with(suffix)
                    || text
                        .windows(suffix.len() + 1)
                        .any(|window| window.starts_with(suffix) && window[suffix.len()] == b'/')
            })
        });
    if !archive {
        return Err(raise_exception::<u64>(
            py,
            BuiltinExc::ImportError,
            "not an archive path",
        ));
    }
    let name = ExceptionValue::adopt(
        py,
        crate::NameId::Zipimport
            .owned_bits(py)
            .ok_or_else(none_bits)?,
    );
    let module = ExceptionValue::adopt(
        py,
        importlib_import_module_impl(py, name.bits(), none_bits())?,
    );
    let importer = attribute(py, module.bits(), crate::NameId::Zipimporter)?;
    let result = checked(py, unsafe { call_callable1(py, importer.bits(), path) })?;
    Ok(result.into_bits())
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_zip_path_hook(path_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        zip_path_hook(py, path_bits).unwrap_or_else(|error| error)
    })
}

/// The bootstrap hook list, in CPython order: the archive hook, then
/// `FileFinder.path_hook()`.
pub(crate) fn default_path_hooks(py: &PyToken<'_>) -> Result<[u64; 2], u64> {
    use crate::builtins::functions::native_callable::{
        NativeCallableKind, NativeCallableName, NativeCallableSpec,
    };
    let spec = NativeCallableSpec {
        kind: NativeCallableKind::Function,
        name: Some(NativeCallableName::Known(crate::NameId::PrivateZipPathHook)),
        ..NativeCallableSpec::uncached_function()
    };
    let zip_hook = crate::builtins::methods::alloc_builtin_function_with_signature(
        py,
        spec,
        crate::builtins::functions::runtime_fn_addr(
            "molt_importlib_zip_path_hook",
            molt_importlib_zip_path_hook as *const (),
        ),
        crate::intrinsics::public_objects_generated::PublicSignatureSpec {
            arity: 1,
            positional: &[b"path"],
            posonly: 0,
            vararg: false,
            varkw: false,
            defaults: &[],
            kwonly: &[],
            kwdefaults: &[],
        },
        &[],
    );
    if zip_hook == 0 || exception_pending(py) {
        if zip_hook != 0 {
            dec_ref_bits(py, zip_hook);
        }
        return Err(none_bits());
    }
    let zip_hook = ExceptionValue::adopt(py, zip_hook);
    let class = crate::builtins::types::file_finder_class(py);
    if class == 0 {
        return Err(none_bits());
    }
    let details = alloc_tuple(py, &[]);
    if details.is_null() {
        return Err(none_bits());
    }
    let details = ExceptionValue::adopt(py, MoltObject::from_ptr(details).bits());
    let file_hook = file_finder_path_hook(py, class, details.bits())?;
    Ok([zip_hook.into_bits(), file_hook])
}
