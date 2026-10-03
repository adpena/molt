//! Code-object local/cell/free slot projection, shared by frame consumers.
//!
//! Public code metadata determines logical slots, not a binding's physical
//! storage at a particular instruction. PEP 709 can temporarily replace a
//! plain local with an isolated cell at the same index.

use std::collections::HashSet;

use super::layout::{
    code_argcount, code_cellvars_bits, code_freevars_bits, code_kwonlyargcount, code_vararg_bits,
    code_varkw_bits, code_varnames_bits,
};
use crate::{TYPE_ID_CODE, TYPE_ID_STRING, TYPE_ID_TUPLE, obj_from_bits, object_type_id};

/// Name edges borrow from the code object. Its owner must outlive this view.
pub(crate) struct CodeSlots {
    pub(crate) names: Vec<u64>,
}

fn str_items(bits: u64) -> Result<Vec<u64>, &'static str> {
    // Unpublished synthetic code objects may not have lexical tables yet.
    if bits == 0 || obj_from_bits(bits).is_none() {
        return Ok(Vec::new());
    }
    let items = obj_from_bits(bits)
        .as_ptr()
        .filter(|ptr| unsafe { object_type_id(*ptr) } == TYPE_ID_TUPLE)
        .and_then(|ptr| unsafe {
            super::seq_access::with_immutable_tuple_slice(ptr, |items| items.to_vec())
        })
        .ok_or("code slot names are not a tuple")?;
    if items.iter().any(|&bits| {
        obj_from_bits(bits)
            .as_ptr()
            .is_none_or(|ptr| unsafe { object_type_id(ptr) } != TYPE_ID_STRING)
    }) {
        return Err("code slot names are not all str");
    }
    Ok(items)
}

/// Derive CPython's localsplus indices without comparing every name pair.
///
/// Local/cell overlaps always merge in place, including non-parameter locals
/// introduced by inlined comprehensions. Cell-only slots and then free slots
/// follow the locals. This does not infer physical cell storage from spelling.
///
/// # Safety
/// `code_ptr` must be a live runtime object, kept alive throughout this call
/// and while the returned borrowed name edges are used. Hold the GIL.
pub(crate) unsafe fn code_slots(code_ptr: *mut u8) -> Result<CodeSlots, &'static str> {
    unsafe {
        crate::gil_assert();
        if object_type_id(code_ptr) != TYPE_ID_CODE {
            return Err("frame binding layout requires a code object");
        }
        let varnames = str_items(code_varnames_bits(code_ptr))?;
        let cellvars = str_items(code_cellvars_bits(code_ptr))?;
        let freevars = str_items(code_freevars_bits(code_ptr))?;
        let declared = |bits: u64| usize::from(bits != 0 && !obj_from_bits(bits).is_none());
        usize::try_from(code_argcount(code_ptr))
            .ok()
            .zip(usize::try_from(code_kwonlyargcount(code_ptr)).ok())
            .and_then(|(positional, keyword)| positional.checked_add(keyword))
            .and_then(|count| count.checked_add(declared(code_vararg_bits(code_ptr))))
            .and_then(|count| count.checked_add(declared(code_varkw_bits(code_ptr))))
            .filter(|&count| count <= varnames.len())
            .ok_or("code parameters exceed co_varnames")?;

        // All keys borrow validated immutable strings; no Python callbacks
        // occur here. Keep the code's tuple owners alive until the set dies.
        let name_key = |bits: u64| {
            let ptr = obj_from_bits(bits).as_ptr().expect("validated code name");
            std::slice::from_raw_parts(super::string_bytes(ptr), super::string_len(ptr))
        };
        let mut local_names = HashSet::with_capacity(varnames.len());
        for &name in &varnames {
            // CodeType can expose repeated public names; preserve every local
            // slot while merging overlapping cell names.
            local_names.insert(name_key(name));
        }
        let mut names = varnames;
        names.reserve(cellvars.len() + freevars.len());
        for name in cellvars {
            if !local_names.contains(name_key(name)) {
                names.push(name);
            }
        }
        names.extend(freevars);
        Ok(CodeSlots { names })
    }
}

#[cfg(test)]
mod tests;
