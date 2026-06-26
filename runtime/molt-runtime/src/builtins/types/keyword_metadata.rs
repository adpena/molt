use super::*;

type FutureRelease = (i64, i64, i64, &'static str, i64);

struct FutureFeatureEntry {
    name: &'static str,
    optional: FutureRelease,
    mandatory: Option<FutureRelease>,
    compiler_flag: i64,
}

const FUTURE_FEATURES: &[FutureFeatureEntry] = &[
    FutureFeatureEntry {
        name: "nested_scopes",
        optional: (2, 1, 0, "beta", 1),
        mandatory: Some((2, 2, 0, "alpha", 0)),
        compiler_flag: 0x0010,
    },
    FutureFeatureEntry {
        name: "generators",
        optional: (2, 2, 0, "alpha", 1),
        mandatory: Some((2, 3, 0, "final", 0)),
        compiler_flag: 0,
    },
    FutureFeatureEntry {
        name: "division",
        optional: (2, 2, 0, "alpha", 2),
        mandatory: Some((3, 0, 0, "alpha", 0)),
        compiler_flag: 0x20000,
    },
    FutureFeatureEntry {
        name: "absolute_import",
        optional: (2, 5, 0, "alpha", 1),
        mandatory: Some((3, 0, 0, "alpha", 0)),
        compiler_flag: 0x40000,
    },
    FutureFeatureEntry {
        name: "with_statement",
        optional: (2, 5, 0, "alpha", 1),
        mandatory: Some((2, 6, 0, "alpha", 0)),
        compiler_flag: 0x80000,
    },
    FutureFeatureEntry {
        name: "print_function",
        optional: (2, 6, 0, "alpha", 2),
        mandatory: Some((3, 0, 0, "alpha", 0)),
        compiler_flag: 0x100000,
    },
    FutureFeatureEntry {
        name: "unicode_literals",
        optional: (2, 6, 0, "alpha", 2),
        mandatory: Some((3, 0, 0, "alpha", 0)),
        compiler_flag: 0x200000,
    },
    FutureFeatureEntry {
        name: "barry_as_FLUFL",
        optional: (3, 1, 0, "alpha", 2),
        mandatory: Some((4, 0, 0, "alpha", 0)),
        compiler_flag: 0x400000,
    },
    FutureFeatureEntry {
        name: "generator_stop",
        optional: (3, 5, 0, "beta", 1),
        mandatory: Some((3, 7, 0, "alpha", 0)),
        compiler_flag: 0x800000,
    },
    FutureFeatureEntry {
        name: "annotations",
        optional: (3, 7, 0, "beta", 1),
        mandatory: None,
        compiler_flag: 0x1000000,
    },
];

pub(crate) const HARD_KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue",
    "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import",
    "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while",
    "with", "yield",
];

const SOFT_KEYWORDS: &[&str] = &["_", "case", "match", "type"];

fn alloc_str_list_bits(_py: &PyToken<'_>, words: &[&str]) -> Option<u64> {
    let mut elems: Vec<u64> = Vec::with_capacity(words.len());
    for word in words {
        let ptr = alloc_string(_py, word.as_bytes());
        if ptr.is_null() {
            for bits in elems {
                dec_ref_bits(_py, bits);
            }
            return None;
        }
        elems.push(MoltObject::from_ptr(ptr).bits());
    }
    let list_ptr = alloc_list(_py, &elems);
    if list_ptr.is_null() {
        for bits in elems {
            dec_ref_bits(_py, bits);
        }
        return None;
    }
    for bits in elems {
        dec_ref_bits(_py, bits);
    }
    Some(MoltObject::from_ptr(list_ptr).bits())
}

fn alloc_release_tuple_bits(_py: &PyToken<'_>, rel: FutureRelease) -> Option<u64> {
    let release_ptr = alloc_string(_py, rel.3.as_bytes());
    if release_ptr.is_null() {
        return None;
    }
    let release_bits = MoltObject::from_ptr(release_ptr).bits();
    let parts = [
        MoltObject::from_int(rel.0).bits(),
        MoltObject::from_int(rel.1).bits(),
        MoltObject::from_int(rel.2).bits(),
        release_bits,
        MoltObject::from_int(rel.4).bits(),
    ];
    let tuple_ptr = alloc_tuple(_py, &parts);
    dec_ref_bits(_py, release_bits);
    if tuple_ptr.is_null() {
        return None;
    }
    Some(MoltObject::from_ptr(tuple_ptr).bits())
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_keyword_lists() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(kwlist_bits) = alloc_str_list_bits(_py, HARD_KEYWORDS) else {
            return MoltObject::none().bits();
        };
        let Some(softkwlist_bits) = alloc_str_list_bits(_py, SOFT_KEYWORDS) else {
            dec_ref_bits(_py, kwlist_bits);
            return MoltObject::none().bits();
        };
        let pair_ptr = alloc_tuple(_py, &[kwlist_bits, softkwlist_bits]);
        dec_ref_bits(_py, kwlist_bits);
        dec_ref_bits(_py, softkwlist_bits);
        if pair_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(pair_ptr).bits()
    })
}

pub(crate) fn keyword_contains(value_bits: u64, keywords: &[&str]) -> bool {
    let value_obj = obj_from_bits(value_bits);
    let Some(value_ptr) = value_obj.as_ptr() else {
        return false;
    };
    unsafe {
        if object_type_id(value_ptr) != TYPE_ID_STRING {
            return false;
        }
    }
    let Some(value) = string_obj_to_owned(value_obj) else {
        return false;
    };
    keywords.contains(&value.as_str())
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_keyword_iskeyword(value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        MoltObject::from_bool(keyword_contains(value_bits, HARD_KEYWORDS)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_keyword_issoftkeyword(value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        MoltObject::from_bool(keyword_contains(value_bits, SOFT_KEYWORDS)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_future_features() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mut rows: Vec<u64> = Vec::with_capacity(FUTURE_FEATURES.len());
        for feature in FUTURE_FEATURES {
            let name_ptr = alloc_string(_py, feature.name.as_bytes());
            if name_ptr.is_null() {
                eprintln!(
                    "MOLT_WARN: molt_future_features: alloc_string failed for '{}'",
                    feature.name
                );
                for bits in rows {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let Some(optional_bits) = alloc_release_tuple_bits(_py, feature.optional) else {
                dec_ref_bits(_py, name_bits);
                for bits in rows {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            };
            let mandatory_bits = if let Some(mandatory) = feature.mandatory {
                let Some(bits) = alloc_release_tuple_bits(_py, mandatory) else {
                    dec_ref_bits(_py, name_bits);
                    dec_ref_bits(_py, optional_bits);
                    for bits in rows {
                        dec_ref_bits(_py, bits);
                    }
                    return MoltObject::none().bits();
                };
                bits
            } else {
                MoltObject::none().bits()
            };
            let compiler_flag_bits = MoltObject::from_int(feature.compiler_flag).bits();
            let row_ptr = alloc_tuple(
                _py,
                &[name_bits, optional_bits, mandatory_bits, compiler_flag_bits],
            );
            dec_ref_bits(_py, name_bits);
            dec_ref_bits(_py, optional_bits);
            if !obj_from_bits(mandatory_bits).is_none() {
                dec_ref_bits(_py, mandatory_bits);
            }
            if row_ptr.is_null() {
                for bits in rows {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            rows.push(MoltObject::from_ptr(row_ptr).bits());
        }
        let list_ptr = alloc_list(_py, &rows);
        if list_ptr.is_null() {
            for bits in rows {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        for bits in rows {
            dec_ref_bits(_py, bits);
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}
