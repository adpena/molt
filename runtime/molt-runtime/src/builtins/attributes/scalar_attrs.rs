use super::*;
use crate::builtins::methods::{float_method_bits, int_method_bits};

/// Which builtin numeric scalar class a receiver belongs to, for
/// [`resolve_scalar_method`].
///
/// `Bool` is distinct from `Int` only for class selection (`bool`'s own class vs
/// `int`'s); its methods come from the int method table because, per CPython,
/// `bool` inherits `int`.
#[derive(Clone, Copy)]
enum ScalarKind {
    Int,
    Bool,
    Float,
}

fn numeric_scalar_kind_from_bits(py: &PyToken<'_>, obj_bits: u64) -> Option<ScalarKind> {
    let obj = obj_from_bits(obj_bits);
    if obj.is_float() {
        return Some(ScalarKind::Float);
    }
    if obj.is_bool() {
        return Some(ScalarKind::Bool);
    }
    if obj.is_int() {
        return Some(ScalarKind::Int);
    }
    // Heap numeric storage also carries subclasses. Only canonical class
    // identity admits this builtin-only resolver; payload kind is not proof.
    let actual = type_of_bits(py, obj_bits);
    let builtins = builtin_classes(py);
    if actual == builtins.int {
        return Some(ScalarKind::Int);
    }
    if actual == builtins.float {
        return Some(ScalarKind::Float);
    }
    None
}

pub(crate) fn is_numeric_scalar_attr_receiver(py: &PyToken<'_>, obj_bits: u64) -> bool {
    numeric_scalar_kind_from_bits(py, obj_bits).is_some()
}

fn scalar_class_bits(_py: &PyToken<'_>, kind: ScalarKind) -> u64 {
    let builtins = builtin_classes(_py);
    match kind {
        ScalarKind::Int => builtins.int,
        ScalarKind::Bool => builtins.bool,
        ScalarKind::Float => builtins.float,
    }
}

/// Resolve `name` as a bound method on a numeric/bool scalar receiver.
///
/// This is the method half of the single numeric scalar attribute authority. The
/// receiver classifier in [`resolve_scalar_attr`] sends inline int/bool/float
/// and exact heap bigint/NaN-float through this same binder, so `getattr`,
/// `getattr(_, default)`, `hasattr`, and direct `object.__getattribute__` can
/// never disagree about which numeric methods a scalar exposes.
fn resolve_scalar_method(
    _py: &PyToken<'_>,
    self_bits: u64,
    kind: ScalarKind,
    name: &str,
) -> Option<u64> {
    let builtins = builtin_classes(_py);
    let class_bits = scalar_class_bits(_py, kind);
    let class_ptr = obj_from_bits(class_bits).as_ptr()?;
    let direct = match kind {
        ScalarKind::Int | ScalarKind::Bool => int_method_bits(_py, name),
        ScalarKind::Float => float_method_bits(_py, name),
    };
    if let Some(func_bits) = direct {
        return unsafe {
            descriptor_bind(
                _py,
                func_bits,
                Some(MoltObject::from_ptr(class_ptr).bits()),
                Some(self_bits),
            )
        };
    }
    if let Some(func_bits) = builtin_class_method_bits(_py, class_bits, name) {
        return unsafe {
            descriptor_bind(
                _py,
                func_bits,
                Some(MoltObject::from_ptr(class_ptr).bits()),
                Some(self_bits),
            )
        };
    }
    if let Some(func_bits) = builtin_class_method_bits(_py, builtins.object, name) {
        return unsafe {
            descriptor_bind(
                _py,
                func_bits,
                Some(MoltObject::from_ptr(class_ptr).bits()),
                Some(self_bits),
            )
        };
    }
    None
}

/// Resolve `attr_name` on a scalar receiver.
///
/// Numeric scalars include inline int/bool/float plus exact heap bigint and
/// NaN-float. Heap subclasses use logical-class lookup. This is shared by every
/// numeric-scalar attribute path:
/// `molt_get_attr_name`, `molt_get_attr_name_default`, `molt_has_attr_name`,
/// `molt_get_attr_object`, `attr_lookup_ptr`, and `molt_object_getattribute`.
pub(crate) fn resolve_scalar_attr(
    _py: &PyToken<'_>,
    obj_bits: u64,
    attr_name: &str,
) -> Option<u64> {
    let class_bits = type_of_bits(_py, obj_bits);
    let name = attr_name_bits_from_bytes(_py, attr_name.as_bytes())?;
    let value = unsafe {
        obj_from_bits(class_bits)
            .as_ptr()
            .and_then(|class| class_attr_lookup_raw_mro(_py, class, name))
            .and_then(|descriptor| {
                descriptor_bind(_py, descriptor, Some(class_bits), Some(obj_bits))
            })
    };
    dec_ref_bits(_py, name);
    if value.is_some() || exception_pending(_py) {
        return value;
    }
    if let Some(kind) = numeric_scalar_kind_from_bits(_py, obj_bits) {
        return resolve_scalar_method(_py, obj_bits, kind, attr_name);
    }
    None
}
