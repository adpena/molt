use super::*;

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

pub(crate) fn molt_keyword_lists_attached_body(_py: &crate::PyToken<'_>) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
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

pub(crate) fn molt_keyword_iskeyword_attached_body(
    _py: &crate::PyToken<'_>,
    value_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        MoltObject::from_bool(keyword_contains(value_bits, HARD_KEYWORDS)).bits()
    })
}

pub(crate) fn molt_keyword_issoftkeyword_attached_body(
    _py: &crate::PyToken<'_>,
    value_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        MoltObject::from_bool(keyword_contains(value_bits, SOFT_KEYWORDS)).bits()
    })
}

include!("../../attached_exports_generated/builtins/types/keyword_metadata.rs");
