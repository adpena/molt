use super::*;
use crate::BuiltinExc;

pub(crate) fn molt_importlib_find_in_path_package_context_attached_body(
    _py: &crate::PyToken<'_>,
    fullname_bits: u64,
    search_paths_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        importlib_find_in_path_payload(_py, fullname_bits, search_paths_bits, true)
    })
}

/// The single ordered meta-path protocol, available before Python facades run.
/// CPython v3.12.15 walks the live iterable; v3.13.16+ copies it first
/// (gh-130094). Each finder lookup/call holds the interpreter import lock;
/// cache/spec reconciliation deliberately runs after releasing that lock.
pub(in super::super) fn importlib_find_spec_impl(
    py: &PyToken<'_>,
    name: u64,
    path: u64,
    target: u64,
) -> Result<u64, u64> {
    use crate::builtins::exceptions::ExceptionValue;
    let _name_owner = ExceptionValue::pin(py, name);
    let _path_owner = ExceptionValue::pin(py, path);
    let _target_owner = ExceptionValue::pin(py, target);
    let sys = importlib_system_module(py)?;
    let read = |object, key: crate::NameId| -> Result<ExceptionValue<'_, '_>, u64> {
        let key = key.bits(py);
        if key == 0 {
            return Err(MoltObject::none().bits());
        }
        let bits = crate::builtins::attributes::get_attr_name(py, object, key)
            .map_err(|()| MoltObject::none().bits())?;
        Ok(ExceptionValue::adopt(py, bits))
    };
    let observed = read(sys.bits(), crate::NameId::MetaPath)?;
    if obj_from_bits(observed.bits()).is_none() {
        return Err(raise_exception::<_>(
            py,
            BuiltinExc::ImportError,
            "sys.meta_path is None, Python is likely shutting down",
        ));
    }
    let meta_path = if crate::object::ops_sys::runtime_target_at_least(py, 3, 13) {
        let bits = unsafe { crate::object::ops::list_from_iter_bits(py, observed.bits()) }
            .ok_or_else(|| MoltObject::none().bits())?;
        let copied = ExceptionValue::adopt(py, bits);
        // `meta_path = list(meta_path)` retires the original expression owner
        // before truth conversion or any later finder/cache callbacks.
        drop(observed);
        copied
    } else {
        observed
    };
    let nonempty = is_truthy(py, obj_from_bits(meta_path.bits()));
    if exception_pending(py) {
        return Err(MoltObject::none().bits());
    }
    if !nonempty {
        // Use the actual public warning callable, including rebinding and
        // noncallable/error behavior; the fast warning engine is its callee.
        let warnings = {
            let dictionary = crate::builtins::module_table::module_dictionary(py)
                .ok_or_else(|| MoltObject::none().bits())?;
            let dictionary = ExceptionValue::pin(py, dictionary);
            ExceptionValue::adopt(
                py,
                importlib_module_support_bits(
                    py,
                    crate::ptr_from_bits(dictionary.bits()),
                    "_warnings",
                )?,
            )
        };
        let key = ExceptionValue::adopt(
            py,
            crate::NameId::Warn
                .owned_bits(py)
                .ok_or_else(|| MoltObject::none().bits())?,
        );
        let warn = ExceptionValue::adopt(
            py,
            crate::builtins::attributes::get_attr_name(py, warnings.bits(), key.bits())
                .map_err(|()| MoltObject::none().bits())?,
        );
        drop(warnings);
        let text = ExceptionValue::adopt(py, alloc_str_bits(py, "sys.meta_path is empty")?);
        let category =
            crate::builtins::exceptions::exception_class_bits(py, BuiltinExc::ImportWarning);
        if category == 0 {
            return Err(MoltObject::none().bits());
        }
        let category = ExceptionValue::pin(py, category);
        let result = ExceptionValue::adopt(py, unsafe {
            call_callable2(py, warn.bits(), text.bits(), category.bits())
        });
        drop(result);
        if exception_pending(py) {
            return Err(MoltObject::none().bits());
        }
    }
    let contains = || -> Result<bool, u64> {
        let modules = read(sys.bits(), crate::NameId::Modules)?;
        let result = crate::object::ops::molt_contains_attached_body(py, modules.bits(), name);
        if exception_pending(py) {
            return Err(MoltObject::none().bits());
        }
        let truth = is_truthy(py, obj_from_bits(result));
        dec_ref_bits(py, result);
        if exception_pending(py) {
            return Err(MoltObject::none().bits());
        }
        Ok(truth)
    };
    let is_reload = contains()?;
    let mut finders = crate::object::iterable::OwnedIterator::new(py, meta_path.bits())
        .ok_or_else(|| MoltObject::none().bits())?;
    let mut finder_owner = None;
    let mut callable_owner = None;
    while let Some(finder) = finders.next().map_err(|_| MoltObject::none().bits())? {
        drop(finder_owner.replace(ExceptionValue::adopt(py, finder)));
        let finder = finder_owner.as_ref().unwrap();
        let lock = ImportLockGuard::acquire(py)?;
        let result = (|| -> Result<Option<ExceptionValue<'_, '_>>, u64> {
            let key = crate::NameId::FindSpec.bits(py);
            if key == 0 {
                return Err(MoltObject::none().bits());
            }
            let Some(bits) = getattr_optional_bits(py, finder.bits(), key)? else {
                return Ok(None);
            };
            drop(callable_owner.replace(ExceptionValue::adopt(py, bits)));
            let callable = callable_owner.as_ref().unwrap();
            let bits = unsafe { call_callable3(py, callable.bits(), name, path, target) };
            let spec = ExceptionValue::adopt(py, bits);
            if exception_pending(py) {
                return Err(MoltObject::none().bits());
            }
            Ok(Some(spec))
        })();
        lock.finish(py)?;
        let Some(spec) = result? else {
            continue;
        };
        if obj_from_bits(spec.bits()).is_none() {
            continue;
        }
        let module = if !is_reload && contains()? {
            let module = {
                let modules = read(sys.bits(), crate::NameId::Modules)?;
                // sys.modules is a public subscription receiver: custom
                // mappings must reach __getitem__, with the normal dict fast
                // path retained by the shared subscription authority.
                let bits = crate::object::ops::molt_index_attached_body(py, modules.bits(), name);
                let module = ExceptionValue::adopt(py, bits);
                if exception_pending(py) {
                    return Err(MoltObject::none().bits());
                }
                module
            }; // The subscription's temporary mapping receiver retires here.
            Some(module)
        } else {
            None
        };
        let selected = if let Some(module) = module.as_ref() {
            let key = crate::NameId::DunderSpec.bits(py);
            if key == 0 {
                return Err(MoltObject::none().bits());
            }
            getattr_optional_bits(py, module.bits(), key)?
                .map(|bits| ExceptionValue::adopt(py, bits))
        } else {
            None
        }; // Keep the selected module alive through extension admission/return.
        let result = selected
            .as_ref()
            .filter(|value| !obj_from_bits(value.bits()).is_none())
            .map_or(spec.bits(), ExceptionValue::bits);
        let fullname = string_arg_from_bits(py, name, "module name")?;
        importlib_enforce_extension_spec_object_boundary(py, &fullname, result)?;
        inc_ref_bits(py, result);
        return Ok(result);
    }
    Ok(MoltObject::none().bits())
}

pub(crate) fn molt_importlib_find_spec_attached_body(
    _py: &crate::PyToken<'_>,
    name: u64,
    path: u64,
    target: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        importlib_find_spec_impl(py, name, path, target).unwrap_or_else(|error| error)
    })
}

include!("../../attached_exports_generated/builtins/platform_importlib_ffi/find_spec.rs");
