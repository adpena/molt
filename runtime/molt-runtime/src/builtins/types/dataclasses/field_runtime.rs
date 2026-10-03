use super::*;
use crate::object::builders::PtrDropGuard;
use crate::object::ops_compare::{CompareBoolOutcome, compare_object_eq_bool};
use crate::object::seq_access::PinnedSequenceSnapshot;

/// An attribute or comparison result transfers one owned reference. Keep that
/// ownership explicit across every later descriptor, truth or equality callback.
struct DcValue {
    bits: u64,
    guard: PtrDropGuard,
}

impl DcValue {
    fn owned(bits: u64) -> Self {
        Self {
            bits,
            guard: PtrDropGuard::new(obj_from_bits(bits).as_ptr().unwrap_or(std::ptr::null_mut())),
        }
    }

    fn into_bits(mut self) -> u64 {
        self.guard.release();
        self.bits
    }
}

macro_rules! dc_try {
    ($result:expr) => {
        match $result {
            Ok(value) => value,
            Err(()) => return MoltObject::none().bits(),
        }
    };
}

fn dc_result(py: &PyToken<'_>, bits: u64) -> Result<DcValue, ()> {
    let value = DcValue::owned(bits);
    if exception_pending(py) { Err(()) } else { Ok(value) }
}

fn dc_attr(py: &PyToken<'_>, object: u64, name: &[u8]) -> Result<DcValue, ()> {
    let name = DcValue::owned(attr_name_bits_from_bytes(py, name).ok_or(())?);
    dc_result(py, crate::builtins::attributes::molt_get_attr_name(object, name.bits))
}

pub(in crate::builtins::types::dataclasses) fn dc_getattr_default_bits(
    py: &PyToken<'_>, object: u64, name: &[u8], default: u64,
) -> Option<u64> {
    let name = DcValue::owned(attr_name_bits_from_bytes(py, name)?);
    Some(crate::builtins::attributes::molt_get_attr_name_default(object, name.bits, default))
}

fn dc_truth(py: &PyToken<'_>, bits: u64) -> Result<bool, ()> {
    let truth = is_truthy(py, obj_from_bits(bits));
    if exception_pending(py) { Err(()) } else { Ok(truth) }
}

fn dc_repr_str(py: &PyToken<'_>, bits: u64) -> Result<String, ()> {
    let repr = dc_result(py, molt_repr_from_obj(bits))?;
    string_obj_to_owned(obj_from_bits(repr.bits)).ok_or(())
}

/// Snapshot fields with one retained reference per entry. A field descriptor
/// may clear or replace the source dictionary while another field is inspected.
fn dc_dict_fields<'a, 'py>(
    py: &'a PyToken<'py>, bits: u64,
) -> Result<PinnedSequenceSnapshot<'a, 'py>, ()> {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        raise_exception::<()>(py, "TypeError", "dataclass fields must be a dict");
        return Err(());
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_DICT {
            raise_exception::<()>(py, "TypeError", "dataclass fields must be a dict");
            return Err(());
        }
        let order = dict_order(ptr);
        let Some(values) = crate::object::backing::tracked_vec_box_from_slice(order, order.len()) else {
            raise_exception::<()>(py, "MemoryError", "dataclass field snapshot allocation failed");
            return Err(());
        };
        let values = crate::object::backing::tracked_vec_box_from_raw(values);
        for &value in values.iter() {
            inc_ref_bits(py, value);
        }
        Ok(PinnedSequenceSnapshot::from_owned_values(py, values))
    }
}

fn dc_fields_ordered<'a, 'py>(
    py: &'a PyToken<'py>, object: u64,
) -> Result<PinnedSequenceSnapshot<'a, 'py>, ()> {
    let fields = dc_attr(py, type_of_bits(py, object), b"__dataclass_fields__")?;
    dc_dict_fields(py, fields.bits)
}

fn dc_field_has_tag(py: &PyToken<'_>, field: u64, tag: &[u8]) -> Result<bool, ()> {
    let field_type = dc_attr(py, field, b"_field_type")?;
    let name = dc_attr(py, field_type.bits, b"name")?;
    Ok(string_obj_to_owned(obj_from_bits(name.bits)).is_some_and(|name| name.as_bytes() == tag))
}

fn dc_is_field(py: &PyToken<'_>, field: u64) -> Result<bool, ()> {
    dc_field_has_tag(py, field, b"_FIELD")
}

fn dc_field_bool_attr(py: &PyToken<'_>, field: u64, name: &[u8], default: bool) -> Result<bool, ()> {
    let bits = dc_getattr_default_bits(py, field, name, MoltObject::from_bool(default).bits()).ok_or(())?;
    let value = dc_result(py, bits)?;
    dc_truth(py, value.bits)
}

fn dc_field_name_str(py: &PyToken<'_>, field: u64) -> Result<String, ()> {
    let name = dc_attr(py, field, b"name")?;
    string_obj_to_owned(obj_from_bits(name.bits)).ok_or_else(|| {
        raise_exception::<()>(py, "TypeError", "dataclass field name must be a string");
    })
}

fn dc_field_hash_flag(py: &PyToken<'_>, field: u64, compare: bool) -> Result<bool, ()> {
    let bits = dc_getattr_default_bits(py, field, b"hash", MoltObject::none().bits()).ok_or(())?;
    let value = dc_result(py, bits)?;
    if obj_from_bits(value.bits).is_none() { Ok(compare) } else { dc_truth(py, value.bits) }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclasses_repr(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let fields = dc_try!(dc_fields_ordered(py, self_bits));
        let class = dc_try!(dc_attr(py, self_bits, b"__class__"));
        let name = dc_try!(dc_attr(py, class.bits, b"__qualname__"));
        let name = string_obj_to_owned(obj_from_bits(name.bits)).unwrap_or_else(|| "?".into());
        let mut parts = Vec::new();
        for pair in fields.chunks_exact(2) {
            let field = pair[1];
            if !dc_try!(dc_is_field(py, field)) || !dc_try!(dc_field_bool_attr(py, field, b"repr", true)) {
                continue;
            }
            let field_name = dc_try!(dc_field_name_str(py, field));
            let value = dc_try!(dc_attr(py, self_bits, field_name.as_bytes()));
            let repr = dc_try!(dc_repr_str(py, value.bits));
            parts.push(format!("{field_name}={repr}"));
        }
        let result = format!("{name}({})", parts.join(", "));
        let ptr = alloc_string(py, result.as_bytes());
        if ptr.is_null() { MoltObject::none().bits() } else { MoltObject::from_ptr(ptr).bits() }
    })
}

/// Match the target's generated dataclass expression: 3.12 compares two fully
/// evaluated tuples, while 3.13+ returns a short-circuit chain of rich results.
fn dc_value_tuple(py: &PyToken<'_>, object: u64, names: &[u64]) -> Result<DcValue, ()> {
    let mut values = Vec::with_capacity(names.len());
    for &name in names {
        match dc_result(py, crate::builtins::attributes::molt_get_attr_name(object, name)) {
            Ok(value) => values.push(value),
            Err(()) => {
                // A failed tuple expression unwinds its operand stack from
                // the most recently evaluated field back to the first.
                while let Some(value) = values.pop() { drop(value); }
                return Err(());
            }
        }
    }
    let bits: Vec<u64> = values.iter().map(|value| value.bits).collect();
    let ptr = alloc_tuple(py, &bits);
    if ptr.is_null() {
        while let Some(value) = values.pop() { drop(value); }
        return Err(());
    }
    Ok(DcValue::owned(MoltObject::from_ptr(ptr).bits()))
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclasses_eq(self_bits: u64, other_bits: u64, compare_names: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let direct = crate::object::ops_sys::runtime_target_at_least(py, 3, 13);
        if direct && self_bits == other_bits {
            return MoltObject::from_bool(true).bits();
        }
        let other_class = dc_try!(dc_attr(py, other_bits, b"__class__"));
        let self_class = dc_try!(dc_attr(py, self_bits, b"__class__"));
        let same_class = self_class.bits == other_class.bits;
        drop(other_class);
        drop(self_class);
        if !same_class {
            return crate::builtins::methods::not_implemented_bits(py);
        }
        let Some(names_ptr) = obj_from_bits(compare_names).as_ptr().filter(|&ptr| unsafe {
            object_type_id(ptr) == TYPE_ID_TUPLE
        }) else {
            return raise_exception::<_>(py, "TypeError", "dataclass comparison fields must be a tuple");
        };
        // Decoration captures this immutable tuple in the generated closure.
        // Later metadata edits cannot change the method's selected fields.
        let Some(names) = (unsafe { crate::object::seq_access::snapshot(py, names_ptr, "dataclass comparison field snapshot allocation failed") }) else {
            return MoltObject::none().bits();
        };
        if direct {
            for (index, &name) in names.iter().enumerate() {
                let left = dc_try!(dc_result(py, crate::builtins::attributes::molt_get_attr_name(self_bits, name)));
                let right = dc_try!(dc_result(py, crate::builtins::attributes::molt_get_attr_name(other_bits, name)));
                let result = dc_result(py, molt_eq(left.bits, right.bits));
                drop(left);
                drop(right);
                let result = dc_try!(result);
                if index + 1 == names.len() || !dc_try!(dc_truth(py, result.bits)) {
                    return result.into_bits();
                }
            }
        } else {
            let left = dc_try!(dc_value_tuple(py, self_bits, &names));
            let right = dc_try!(dc_value_tuple(py, other_bits, &names));
            let outcome = compare_object_eq_bool(py, obj_from_bits(left.bits), obj_from_bits(right.bits));
            drop(left);
            drop(right);
            match outcome {
                CompareBoolOutcome::True => {}
                CompareBoolOutcome::False | CompareBoolOutcome::NotComparable => {
                    return MoltObject::from_bool(false).bits();
                }
                CompareBoolOutcome::Error => return MoltObject::none().bits(),
            }
        }
        MoltObject::from_bool(true).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclasses_hash_fn(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let fields = dc_try!(dc_fields_ordered(py, self_bits));
        let mut values = Vec::new();
        for pair in fields.chunks_exact(2) {
            let field = pair[1];
            if !dc_try!(dc_is_field(py, field)) { continue; }
            let compare = dc_try!(dc_field_bool_attr(py, field, b"compare", true));
            if !dc_try!(dc_field_hash_flag(py, field, compare)) { continue; }
            let name = dc_try!(dc_field_name_str(py, field));
            values.push(dc_try!(dc_attr(py, self_bits, name.as_bytes())));
        }
        let bits: Vec<u64> = values.iter().map(|value| value.bits).collect();
        let ptr = alloc_tuple(py, &bits);
        if ptr.is_null() { return MoltObject::none().bits(); }
        let tuple = DcValue::owned(MoltObject::from_ptr(ptr).bits());
        molt_hash_builtin(tuple.bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclasses_check_default_order(fields_dict_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let fields = dc_try!(dc_dict_fields(py, fields_dict_bits));
        let mut previous: Option<String> = None;
        for pair in fields.chunks_exact(2) {
            let field = pair[1];
            if !dc_try!(dc_field_has_tag(py, field, b"_FIELD"))
                && !dc_try!(dc_field_has_tag(py, field, b"_FIELD_INITVAR")) {
                continue;
            }
            if !dc_try!(dc_field_bool_attr(py, field, b"init", true))
                || dc_try!(dc_field_bool_attr(py, field, b"kw_only", false)) {
                continue;
            }
            let mut has_default = false;
            for attr in [b"default".as_slice(), b"default_factory".as_slice()] {
                let value = dc_try!(dc_attr(py, field, attr));
                if dc_try!(dc_repr_str(py, value.bits)).as_bytes() != b"MISSING" {
                    has_default = true;
                    break;
                }
            }
            let name = dc_try!(dc_field_name_str(py, field));
            if has_default {
                previous = Some(name);
            } else if let Some(previous) = &previous {
                return raise_exception::<_>(py, "TypeError", &format!(
                    "non-default argument {name:?} follows default argument {previous:?}"
                ));
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclasses_field_flags(fields_dict_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let fields = dc_try!(dc_dict_fields(py, fields_dict_bits));
        let mut flags = Vec::new();
        for pair in fields.chunks_exact(2) {
            let field = pair[1];
            if !dc_try!(dc_is_field(py, field)) { continue; }
            let repr = dc_try!(dc_field_bool_attr(py, field, b"repr", true));
            let compare = dc_try!(dc_field_bool_attr(py, field, b"compare", true));
            let hash = dc_try!(dc_field_hash_flag(py, field, compare));
            flags.push(MoltObject::from_int(i64::from(repr) | (i64::from(compare) << 1) | (i64::from(hash) << 2)).bits());
        }
        let ptr = alloc_tuple(py, &flags);
        if ptr.is_null() { MoltObject::none().bits() } else { MoltObject::from_ptr(ptr).bits() }
    })
}
