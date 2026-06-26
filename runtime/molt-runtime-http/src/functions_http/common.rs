use super::*;

// ---------------------------------------------------------------------------
// Helper functions (originally in builtins/functions.rs)
// ---------------------------------------------------------------------------

pub(crate) fn alloc_string_bits(_py: &CoreGilToken, value: &str) -> Option<u64> {
    let ptr = alloc_string(_py, value.as_bytes());
    if ptr.is_null() {
        None
    } else {
        Some(MoltObject::from_ptr(ptr).bits())
    }
}

pub(super) fn alloc_string_tuple(_py: &CoreGilToken, values: &[String]) -> u64 {
    let mut item_bits: Vec<u64> = Vec::with_capacity(values.len());
    for value in values {
        let Some(bits) = alloc_string_bits(_py, value) else {
            for bit in &item_bits {
                dec_ref_bits(_py, *bit);
            }
            return MoltObject::none().bits();
        };
        item_bits.push(bits);
    }
    let tuple_ptr = alloc_tuple(_py, &item_bits);
    if tuple_ptr.is_null() {
        for bit in &item_bits {
            dec_ref_bits(_py, *bit);
        }
        return MoltObject::none().bits();
    }
    let out = MoltObject::from_ptr(tuple_ptr).bits();
    for bit in &item_bits {
        dec_ref_bits(_py, *bit);
    }
    out
}

pub(super) fn alloc_qsl_list(_py: &CoreGilToken, items: &[(String, String)]) -> u64 {
    let mut tuple_bits: Vec<u64> = Vec::with_capacity(items.len());
    for (key, value) in items {
        let Some(key_bits) = alloc_string_bits(_py, key) else {
            for bit in &tuple_bits {
                dec_ref_bits(_py, *bit);
            }
            return MoltObject::none().bits();
        };
        let Some(value_bits) = alloc_string_bits(_py, value) else {
            dec_ref_bits(_py, key_bits);
            for bit in &tuple_bits {
                dec_ref_bits(_py, *bit);
            }
            return MoltObject::none().bits();
        };
        let tuple_ptr = alloc_tuple(_py, &[key_bits, value_bits]);
        dec_ref_bits(_py, key_bits);
        dec_ref_bits(_py, value_bits);
        if tuple_ptr.is_null() {
            for bit in &tuple_bits {
                dec_ref_bits(_py, *bit);
            }
            return MoltObject::none().bits();
        }
        tuple_bits.push(MoltObject::from_ptr(tuple_ptr).bits());
    }
    let list_ptr = alloc_list_with_capacity(_py, &tuple_bits, tuple_bits.len());
    if list_ptr.is_null() {
        for bit in &tuple_bits {
            dec_ref_bits(_py, *bit);
        }
        return MoltObject::none().bits();
    }
    let out = MoltObject::from_ptr(list_ptr).bits();
    for bit in &tuple_bits {
        dec_ref_bits(_py, *bit);
    }
    out
}

pub(super) fn alloc_qs_dict(
    _py: &CoreGilToken,
    order: &[String],
    values: &HashMap<String, Vec<String>>,
) -> u64 {
    let mut pairs: Vec<u64> = Vec::with_capacity(order.len() * 2);
    let mut owned_bits: Vec<u64> = Vec::with_capacity(order.len() * 2);
    for key in order {
        let Some(key_bits) = alloc_string_bits(_py, key) else {
            for bit in &owned_bits {
                dec_ref_bits(_py, *bit);
            }
            return MoltObject::none().bits();
        };
        let mut value_bits: Vec<u64> = Vec::new();
        for value in values.get(key).into_iter().flatten() {
            let Some(bits) = alloc_string_bits(_py, value) else {
                dec_ref_bits(_py, key_bits);
                for bit in &value_bits {
                    dec_ref_bits(_py, *bit);
                }
                for bit in &owned_bits {
                    dec_ref_bits(_py, *bit);
                }
                return MoltObject::none().bits();
            };
            value_bits.push(bits);
        }
        let list_ptr = alloc_list_with_capacity(_py, &value_bits, value_bits.len());
        for bit in &value_bits {
            dec_ref_bits(_py, *bit);
        }
        if list_ptr.is_null() {
            dec_ref_bits(_py, key_bits);
            for bit in &owned_bits {
                dec_ref_bits(_py, *bit);
            }
            return MoltObject::none().bits();
        }
        let list_bits = MoltObject::from_ptr(list_ptr).bits();
        pairs.push(key_bits);
        pairs.push(list_bits);
        owned_bits.push(key_bits);
        owned_bits.push(list_bits);
    }
    let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
    for bit in &owned_bits {
        dec_ref_bits(_py, *bit);
    }
    if dict_ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(dict_ptr).bits()
}

pub(super) fn iter_next_pair(_py: &CoreGilToken, iter_bits: u64) -> Result<(u64, bool), u64> {
    let pair_bits = molt_iter_next(iter_bits);
    let pair_obj = obj_from_bits(pair_bits);
    let Some(pair_ptr) = pair_obj.as_ptr() else {
        return Err(MoltObject::none().bits());
    };
    unsafe {
        if object_type_id(pair_ptr) != crate::bridge::type_id_tuple() {
            return Err(MoltObject::none().bits());
        }
        let elems = seq_vec_ref(pair_ptr);
        if elems.len() < 2 {
            return Err(MoltObject::none().bits());
        }
        let val_bits = elems[0];
        let done_bits = elems[1];
        let done = is_truthy(_py, obj_from_bits(done_bits));
        Ok((val_bits, done))
    }
}
