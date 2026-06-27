use molt_backend::tir::ops::{AttrDict, AttrValue};

pub(super) fn extract_int_attr(attrs: &AttrDict, key: &str) -> Option<i64> {
    match attrs.get(key)? {
        AttrValue::Int(v) => Some(*v),
        _ => None,
    }
}

pub(super) fn extract_float_attr(attrs: &AttrDict, key: &str) -> Option<f64> {
    match attrs.get(key)? {
        AttrValue::Float(v) => Some(*v),
        _ => None,
    }
}

pub(super) fn extract_bool_attr(attrs: &AttrDict, key: &str) -> Option<bool> {
    match attrs.get(key)? {
        AttrValue::Bool(v) => Some(*v),
        _ => None,
    }
}

pub(super) fn extract_str_attr(attrs: &AttrDict, key: &str) -> Option<String> {
    match attrs.get(key)? {
        AttrValue::Str(v) => Some(v.clone()),
        _ => None,
    }
}
