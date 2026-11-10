//! The static class model's LOAD_BUILD_CLASS boundary (spec 0216).
use crate::*;

pub(crate) const CLASS_BUILDER_ERROR: &str = "Molt's static class model requires the canonical __build_class__; \
    replacing or directly calling the class builder would invalidate compiled class layouts and method resolution";

pub(crate) fn molt_build_class_builtin_attached_body(
    _py: &crate::PyToken<'_>,
    _args: u64,
    _kwargs: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        raise_exception::<u64>(py, BuiltinExc::TypeError, CLASS_BUILDER_ERROR)
    })
}

/// The guard's slow arm. LOAD_BUILD_CLASS never consults globals or class
/// locals. The namespace owner preserves exact-dict and custom __getitem__
/// behavior, including KeyError-only absence and arbitrary callback errors.
pub(crate) fn molt_class_builder_check_attached_body(_py: &crate::PyToken<'_>) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        let builtins = crate::builtins::frames::frame_stack_active_builtins_bits();
        let key = crate::NameId::DunderBuildClass.bits(py);
        if key == 0 {
            return MoltObject::none().bits();
        }
        inc_ref_bits(py, builtins);
        let selected = crate::builtins::modules::lookup_builtin_global(py, key, builtins);
        dec_ref_bits(py, builtins);
        let selected = match selected {
            Ok(Some(bits)) => bits,
            Ok(None) => {
                return raise_exception::<u64>(
                    py,
                    BuiltinExc::NameError,
                    "__build_class__ not found",
                );
            }
            Err(()) => return MoltObject::none().bits(),
        };
        let canonical = crate::intrinsics::registry::class_builder_bits(py);
        let same = canonical == Some(selected);
        dec_ref_bits(py, selected);
        if let Some(bits) = canonical {
            dec_ref_bits(py, bits);
        } else {
            return MoltObject::none().bits();
        }
        if same {
            MoltObject::none().bits()
        } else {
            raise_exception::<u64>(py, BuiltinExc::TypeError, CLASS_BUILDER_ERROR)
        }
    })
}

include!("../attached_exports_generated/builtins/class_builder.rs");

#[cfg(test)]
#[path = "class_builder_tests.rs"]
mod tests;
