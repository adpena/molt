// Fnmatch pattern matching implementation.
// Extracted from functions.rs for compilation-unit size reduction and tree shaking.

use super::functions::*;
use crate::*;
use molt_obj_model::MoltObject;
use molt_stdlib_text::fnmatch::{
    fnmatch_match_bytes_impl, fnmatch_match_impl, fnmatch_normcase_bytes, fnmatch_normcase_text,
    fnmatch_translate_impl,
};

pub(crate) fn fnmatch_bytes_from_bits(bits: u64) -> Option<Vec<u8>> {
    let obj = obj_from_bits(bits);
    let ptr = obj.as_ptr()?;
    unsafe {
        if object_type_id(ptr) != TYPE_ID_BYTES {
            return None;
        }
        bytes_like_slice(ptr).map(|slice| slice.to_vec())
    }
}

// Runtime fnmatch ABI entrypoints.
#[unsafe(no_mangle)]
pub extern "C" fn molt_fnmatchcase(name_bits: u64, pat_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) {
            let Some(pat) = string_obj_to_owned(obj_from_bits(pat_bits)) else {
                if fnmatch_bytes_from_bits(pat_bits).is_some() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "cannot use a bytes pattern on a string-like object",
                    );
                }
                return raise_exception::<_>(_py, "TypeError", "expected str or bytes pattern");
            };
            return MoltObject::from_bool(fnmatch_match_impl(&name, &pat)).bits();
        }
        if let Some(name) = fnmatch_bytes_from_bits(name_bits) {
            let Some(pat) = fnmatch_bytes_from_bits(pat_bits) else {
                if string_obj_to_owned(obj_from_bits(pat_bits)).is_some() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "cannot use a string pattern on a bytes-like object",
                    );
                }
                return raise_exception::<_>(_py, "TypeError", "expected str or bytes pattern");
            };
            return MoltObject::from_bool(fnmatch_match_bytes_impl(&name, &pat)).bits();
        }
        raise_exception::<_>(_py, "TypeError", "expected str or bytes name")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fnmatch(name_bits: u64, pat_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) {
            let Some(pat) = string_obj_to_owned(obj_from_bits(pat_bits)) else {
                if fnmatch_bytes_from_bits(pat_bits).is_some() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "cannot use a bytes pattern on a string-like object",
                    );
                }
                return raise_exception::<_>(_py, "TypeError", "expected str or bytes pattern");
            };
            let name_norm = fnmatch_normcase_text(&name);
            let pat_norm = fnmatch_normcase_text(&pat);
            return MoltObject::from_bool(fnmatch_match_impl(&name_norm, &pat_norm)).bits();
        }
        if let Some(name) = fnmatch_bytes_from_bits(name_bits) {
            let Some(pat) = fnmatch_bytes_from_bits(pat_bits) else {
                if string_obj_to_owned(obj_from_bits(pat_bits)).is_some() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "cannot use a string pattern on a bytes-like object",
                    );
                }
                return raise_exception::<_>(_py, "TypeError", "expected str or bytes pattern");
            };
            let name_norm = fnmatch_normcase_bytes(&name);
            let pat_norm = fnmatch_normcase_bytes(&pat);
            return MoltObject::from_bool(fnmatch_match_bytes_impl(&name_norm, &pat_norm)).bits();
        }
        raise_exception::<_>(_py, "TypeError", "expected str or bytes name")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fnmatch_filter(names_bits: u64, pat_bits: u64, invert_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let pat_str = string_obj_to_owned(obj_from_bits(pat_bits));
        let pat_bytes = if pat_str.is_none() {
            fnmatch_bytes_from_bits(pat_bits)
        } else {
            None
        };
        if pat_str.is_none() && pat_bytes.is_none() {
            return raise_exception::<_>(_py, "TypeError", "expected str or bytes pattern");
        }
        let invert = is_truthy(_py, obj_from_bits(invert_bits));
        let iter_bits = molt_iter(names_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }

        let mut out_bits: Vec<u64> = Vec::new();
        loop {
            let (item_bits, done) = match iter_next_pair(_py, iter_bits) {
                Ok(value) => value,
                Err(bits) => {
                    for bits in out_bits {
                        dec_ref_bits(_py, bits);
                    }
                    return bits;
                }
            };
            if done {
                break;
            }
            if let Some(pat) = &pat_str {
                let Some(name) = string_obj_to_owned(obj_from_bits(item_bits)) else {
                    for bits in out_bits {
                        dec_ref_bits(_py, bits);
                    }
                    return raise_exception::<_>(_py, "TypeError", "expected str item");
                };
                let name_norm = fnmatch_normcase_text(&name);
                let pat_norm = fnmatch_normcase_text(pat);
                let matched = fnmatch_match_impl(&name_norm, &pat_norm);
                if matched != invert {
                    inc_ref_bits(_py, item_bits);
                    out_bits.push(item_bits);
                }
            } else if let Some(pat) = &pat_bytes {
                let Some(name) = fnmatch_bytes_from_bits(item_bits) else {
                    if string_obj_to_owned(obj_from_bits(item_bits)).is_some() {
                        for bits in out_bits {
                            dec_ref_bits(_py, bits);
                        }
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "cannot use a string pattern on a bytes-like object",
                        );
                    }
                    for bits in out_bits {
                        dec_ref_bits(_py, bits);
                    }
                    return raise_exception::<_>(_py, "TypeError", "expected bytes item");
                };
                let name_norm = fnmatch_normcase_bytes(&name);
                let pat_norm = fnmatch_normcase_bytes(pat);
                let matched = fnmatch_match_bytes_impl(&name_norm, &pat_norm);
                if matched != invert {
                    let ptr = alloc_bytes(_py, &name);
                    if ptr.is_null() {
                        for bits in out_bits {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    }
                    out_bits.push(MoltObject::from_ptr(ptr).bits());
                }
            }
        }
        let list_ptr = alloc_list_with_capacity(_py, out_bits.as_slice(), out_bits.len());
        for bits in out_bits {
            dec_ref_bits(_py, bits);
        }
        if list_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(list_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fnmatch_translate(pat_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(pat) = string_obj_to_owned(obj_from_bits(pat_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "expected str pattern");
        };
        let out = fnmatch_translate_impl(&pat);
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}
