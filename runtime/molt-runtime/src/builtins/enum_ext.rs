// === FILE: runtime/molt-runtime/src/builtins/enum_ext.rs ===
//
// Intrinsics for Flag, IntFlag, StrEnum, and auto() enumeration machinery.
//
// Flag values are integer bitmasks.  All Flag operations work entirely in the
// NaN-boxed integer / float domain – no heap allocation for the bitmask value
// itself.  The Python wrapper classes hold the integer value; the intrinsics
// compute the combined/decomposed values and report membership.
//
// auto() values: Python maintains a per-class counter in _generate_next_value_;
// here we provide a simple monotonic counter that returns the next power-of-2
// for Flag-style enums when count_bits represents the *number of existing
// members* (0-based), and a plain 1-based index for non-Flag enums.
// The Python stdlib wrapper can call molt_enum_auto_value(len(existing_members))
// to get the next value.

use crate::*;

// ─────────────────────────────────────────────────────────────────────────────
// Flag arithmetic helpers
// ─────────────────────────────────────────────────────────────────────────────

#[inline]
fn flag_bits_from_obj(obj_bits: u64) -> Option<i64> {
    to_i64(obj_from_bits(obj_bits))
}

// ─── Public intrinsics: Flag / IntFlag ───────────────────────────────────────

/// Create a Flag member: returns the integer bitmask value as a NaN-boxed int.
/// `name_bits` is the str name (ignored at this layer; the Python wrapper uses
/// it for repr).  `value_bits` must be an int bitmask.
#[unsafe(no_mangle)]
pub extern "C" fn molt_enum_flag_new(name_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let _ = name_bits; // name used only by Python wrapper for member setup
        let Some(val) = flag_bits_from_obj(value_bits) else {
            return raise_exception::<_>(_py, "TypeError", "Flag value must be an integer");
        };
        MoltObject::from_int(val).bits()
    })
}

/// Flag.__or__: a | b → combined bitmask.
#[unsafe(no_mangle)]
pub extern "C" fn molt_enum_flag_or(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let (Some(a), Some(b)) = (flag_bits_from_obj(a_bits), flag_bits_from_obj(b_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "Flag operands must be integers");
        };
        MoltObject::from_int(a | b).bits()
    })
}

/// Flag.__and__: a & b → intersection bitmask.
#[unsafe(no_mangle)]
pub extern "C" fn molt_enum_flag_and(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let (Some(a), Some(b)) = (flag_bits_from_obj(a_bits), flag_bits_from_obj(b_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "Flag operands must be integers");
        };
        MoltObject::from_int(a & b).bits()
    })
}

/// Flag.__xor__: a ^ b → XOR bitmask.
#[unsafe(no_mangle)]
pub extern "C" fn molt_enum_flag_xor(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let (Some(a), Some(b)) = (flag_bits_from_obj(a_bits), flag_bits_from_obj(b_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "Flag operands must be integers");
        };
        MoltObject::from_int(a ^ b).bits()
    })
}

/// Flag.__invert__: ~a.  For a non-negative bitmask this produces the bitwise
/// complement.  CPython's Flag uses the boundary of the "pseudo-member" set to
/// invert, but for the low-level intrinsic we return ~a (all bits flipped as i64)
/// and let the Python wrapper mask to the valid member bits.
#[unsafe(no_mangle)]
pub extern "C" fn molt_enum_flag_invert(a_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(a) = flag_bits_from_obj(a_bits) else {
            return raise_exception::<_>(_py, "TypeError", "Flag operand must be an integer");
        };
        MoltObject::from_int(!a).bits()
    })
}

/// Flag.__contains__: check whether all bits of `b` are set in `a`.
/// Returns True if (a & b) == b (i.e. b is a submask of a).
#[unsafe(no_mangle)]
pub extern "C" fn molt_enum_flag_contains(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let (Some(a), Some(b)) = (flag_bits_from_obj(a_bits), flag_bits_from_obj(b_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "Flag operands must be integers");
        };
        MoltObject::from_bool((a & b) == b).bits()
    })
}

/// Decompose a composite Flag value into a list of single-bit integer values.
/// E.g. 0b1010 → [2, 8].  The returned list contains i64 values for each set
/// bit, from LSB to MSB, that the Python wrapper can look up in the enum class
/// members table to build the individual flag member objects.
///
/// Returns an empty list for value 0.
/// Negative values (sign bit set) are treated as their unsigned u64 bit pattern.
#[unsafe(no_mangle)]
pub extern "C" fn molt_enum_flag_decompose(value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(val) = flag_bits_from_obj(value_bits) else {
            return raise_exception::<_>(_py, "TypeError", "Flag value must be an integer");
        };
        let bits_u = val as u64;
        let mut single_bits: Vec<u64> = Vec::new();
        let mut remaining = bits_u;
        let mut bit: u64 = 1;
        while remaining != 0 {
            if remaining & bit != 0 {
                single_bits.push(MoltObject::from_int(bit as i64).bits());
                remaining &= !bit;
            }
            bit <<= 1;
        }
        let ptr = alloc_list(_py, &single_bits);
        if ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "failed to allocate list");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// auto() value generation
// ─────────────────────────────────────────────────────────────────────────────

/// Compute the next `auto()` value given the current member count.
///
/// For standard (non-Flag) enums: returns `count + 1` (1-based index).
/// The Python wrapper decides based on enum kind whether to use the returned
/// value directly (for Enum/IntEnum/StrEnum) or left-shift it (for Flag).
///
/// For Flag enums the wrapper calls `1 << molt_enum_auto_value(count)` to get
/// the next power-of-two — but for simplicity we expose the raw count-based
/// formula here.  The stdlib `_generate_next_value_` hook in Flag already does
/// the bit-shift math; the intrinsic just hands back an integer the Python
/// layer can use as the seed.
///
/// Returns: `count + 1` as a NaN-boxed int.
#[unsafe(no_mangle)]
pub extern "C" fn molt_enum_auto_value(count_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(count) = to_i64(obj_from_bits(count_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "auto() count must be an integer");
        };
        if count < 0 {
            return raise_exception::<_>(_py, "ValueError", "auto() count must be non-negative");
        }
        MoltObject::from_int(count + 1).bits()
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// @unique / duplicate-value checking
// ─────────────────────────────────────────────────────────────────────────────

/// Check that a list of (name_bits, value_bits) member pairs has no duplicate
/// values.  Returns True if all values are unique, False otherwise.
///
/// `members_bits` must be a list of 2-tuples [(name, value), ...].
#[unsafe(no_mangle)]
pub extern "C" fn molt_enum_unique_check(members_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(members_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "members must be a list");
        };
        let type_id = unsafe { object_type_id(ptr) };
        if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
            return raise_exception::<_>(_py, "TypeError", "members must be a list");
        }
        let elems = unsafe { seq_vec_ref(ptr) };
        let mut seen_values: std::collections::HashSet<u64> = std::collections::HashSet::new();
        for &elem_bits in elems.iter() {
            let elem_obj = obj_from_bits(elem_bits);
            let Some(eptr) = elem_obj.as_ptr() else {
                continue;
            };
            let etype = unsafe { object_type_id(eptr) };
            if etype != TYPE_ID_TUPLE && etype != TYPE_ID_LIST {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "each member must be a (name, value) tuple",
                );
            }
            let pair = unsafe { seq_vec_ref(eptr) };
            if pair.len() < 2 {
                return raise_exception::<_>(_py, "ValueError", "each member must have 2 elements");
            }
            let val_bits = pair[1];
            if !seen_values.insert(val_bits) {
                return MoltObject::from_bool(false).bits();
            }
        }
        MoltObject::from_bool(true).bits()
    })
}

/// Check whether `value_bits` is a valid member value in `members_bits`
/// (a list of (name, value) 2-tuples).  Returns True if found.
///
/// Used by `Enum._missing_` and `@verify` to validate membership.
#[unsafe(no_mangle)]
pub extern "C" fn molt_enum_verify_member(members_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(members_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "members must be a list");
        };
        let type_id = unsafe { object_type_id(ptr) };
        if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
            return raise_exception::<_>(_py, "TypeError", "members must be a list");
        }
        let elems = unsafe { seq_vec_ref(ptr) };
        for &elem_bits in elems.iter() {
            let elem_obj = obj_from_bits(elem_bits);
            let Some(eptr) = elem_obj.as_ptr() else {
                continue;
            };
            let etype = unsafe { object_type_id(eptr) };
            if etype != TYPE_ID_TUPLE && etype != TYPE_ID_LIST {
                continue;
            }
            let pair = unsafe { seq_vec_ref(eptr) };
            if pair.len() < 2 {
                continue;
            }
            if pair[1] == value_bits {
                return MoltObject::from_bool(true).bits();
            }
            // Also compare by value equality for int/float/str.
            if obj_eq(_py, obj_from_bits(pair[1]), obj_from_bits(value_bits)) {
                return MoltObject::from_bool(true).bits();
            }
        }
        MoltObject::from_bool(false).bits()
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// StrEnum helper
// ─────────────────────────────────────────────────────────────────────────────

/// Return the default StrEnum value for a member: the lowercased name.
/// `name_bits` must be a str object.  Returns str.
///
/// CPython's StrEnum._generate_next_value_ returns `name.lower()`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_enum_str_value(name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(name_bits);
        let Some(name) = string_obj_to_owned(obj) else {
            let tn = type_name(_py, obj);
            let msg = format!("StrEnum name must be str, not {tn}");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        let lower = name.to_lowercase();
        let ptr = alloc_string(_py, lower.as_bytes());
        if ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "failed to allocate string");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}
