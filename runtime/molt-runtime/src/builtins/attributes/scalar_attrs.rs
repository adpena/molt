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

fn numeric_scalar_kind_from_bits(obj_bits: u64) -> Option<ScalarKind> {
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
    if let Some(ptr) = maybe_ptr_from_bits(obj_bits) {
        match unsafe { object_type_id(ptr) } {
            TYPE_ID_BIGINT => return Some(ScalarKind::Int),
            TYPE_ID_FLOAT => return Some(ScalarKind::Float),
            _ => {}
        }
    }
    None
}

pub(crate) fn is_numeric_scalar_attr_receiver(obj_bits: u64) -> bool {
    numeric_scalar_kind_from_bits(obj_bits).is_some()
}

fn scalar_class_bits(_py: &PyToken<'_>, kind: ScalarKind) -> u64 {
    let builtins = builtin_classes(_py);
    match kind {
        ScalarKind::Int => builtins.int,
        ScalarKind::Bool => builtins.bool,
        ScalarKind::Float => builtins.float,
    }
}

/// Bind a curated builtin class attribute to a scalar receiver, mirroring the
/// descriptor protocol for the kinds those tables can yield.
fn bind_scalar_class_attr(
    _py: &PyToken<'_>,
    attr_bits: u64,
    self_bits: u64,
    class_bits: u64,
) -> u64 {
    if let Some(attr_ptr) = maybe_ptr_from_bits(attr_bits) {
        match unsafe { object_type_id(attr_ptr) } {
            TYPE_ID_CLASSMETHOD => {
                let func_bits = unsafe { classmethod_func_bits(attr_ptr) };
                return molt_bound_method_new(func_bits, class_bits);
            }
            TYPE_ID_STATICMETHOD => {
                let func_bits = unsafe { staticmethod_func_bits(attr_ptr) };
                inc_ref_bits(_py, func_bits);
                return func_bits;
            }
            _ => {}
        }
    }
    molt_bound_method_new(attr_bits, self_bits)
}

/// Resolve `name` as a bound method on a numeric/bool scalar receiver.
///
/// This is the method half of the single numeric scalar attribute authority. The
/// receiver classifier in [`resolve_scalar_attr`] sends inline int/bool/float
/// and heap bigint/NaN-float through this same binder, so `getattr`,
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
    let direct = match kind {
        ScalarKind::Int | ScalarKind::Bool => int_method_bits(_py, name),
        ScalarKind::Float => float_method_bits(_py, name),
    };
    if let Some(func_bits) = direct {
        return Some(bind_scalar_class_attr(
            _py, func_bits, self_bits, class_bits,
        ));
    }
    if let Some(func_bits) = builtin_class_method_bits(_py, class_bits, name) {
        return Some(bind_scalar_class_attr(
            _py, func_bits, self_bits, class_bits,
        ));
    }
    if let Some(func_bits) = builtin_class_method_bits(_py, builtins.object, name) {
        return Some(bind_scalar_class_attr(
            _py, func_bits, self_bits, class_bits,
        ));
    }
    None
}

/// Resolve `attr_name` on a scalar receiver.
///
/// Numeric scalars include inline int/bool/float plus heap bigint and heap
/// NaN-float. This is shared by every numeric-scalar attribute path:
/// `molt_get_attr_name`, `molt_get_attr_name_default`, `molt_has_attr_name`,
/// `molt_get_attr_object`, `attr_lookup_ptr`, and `molt_object_getattribute`.
pub(crate) fn resolve_scalar_attr(
    _py: &PyToken<'_>,
    obj_bits: u64,
    attr_name: &str,
) -> Option<u64> {
    if let Some(kind) = numeric_scalar_kind_from_bits(obj_bits) {
        let class_bits = scalar_class_bits(_py, kind);
        if attr_name == "__class__" {
            inc_ref_bits(_py, class_bits);
            return Some(class_bits);
        }
        return resolve_scalar_method(_py, obj_bits, kind, attr_name);
    }
    if maybe_ptr_from_bits(obj_bits).is_none() && attr_name == "__class__" {
        let class_bits = type_of_bits(_py, obj_bits);
        inc_ref_bits(_py, class_bits);
        return Some(class_bits);
    }
    None
}
