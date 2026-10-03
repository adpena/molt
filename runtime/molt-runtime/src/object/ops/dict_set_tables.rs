use super::*;

#[cfg(test)]
#[path = "dict_binding_tests.rs"]
mod dict_binding_tests;
#[cfg(test)]
#[path = "dict_increment_tests.rs"]
mod dict_increment_tests;

pub(crate) unsafe fn dict_structural_epoch(ptr: *mut u8) -> u64 {
    unsafe { crate::object::backing::tracked_vec_mutation_epoch(dict_hashes(ptr) as *mut Vec<u64>) }
}

pub(crate) unsafe fn dict_commit_structure(ptr: *mut u8) {
    unsafe {
        crate::object::backing::tracked_vec_bump_mutation_epoch(dict_hashes(ptr) as *mut Vec<u64>);
    }
}

/// Publish a fully prepared dictionary without changing the live dictionary's
/// identity or releasing any displaced Python value. The staged dictionary
/// receives the old contents; its caller releases that owner only after all
/// dependent class/cache metadata has committed.
///
/// # Safety
/// Both arguments are distinct live, mutable exact dictionaries. The caller
/// holds the GIL and exclusive custody of `staged`; no borrowed backing view
/// may survive this operation. All fallible preparation precedes this commit.
pub(crate) unsafe fn dict_publish_staged(py: &PyToken<'_>, live: *mut u8, staged: *mut u8) {
    unsafe {
        crate::gil_assert();
        assert_ne!(live, staged);
        assert_eq!(object_type_id(live), TYPE_ID_DICT);
        assert_eq!(object_type_id(staged), TYPE_ID_DICT);
        assert!(
            !(*header_from_obj_ptr(live)).has_flag(crate::object::HEADER_FLAG_FROZEN_LAYOUT_MAP)
        );
        assert!(
            !(*header_from_obj_ptr(staged)).has_flag(crate::object::HEADER_FLAG_FROZEN_LAYOUT_MAP)
        );
        let live_order = dict_order(live) as *mut Vec<u64>;
        let live_hashes = dict_hashes(live) as *mut Vec<u64>;
        let live_table = dict_table(live) as *mut Vec<usize>;
        let _order_lock = crate::object::backing::tracked_vec_mutation_lock(live_order);
        let _hashes_lock = crate::object::backing::tracked_vec_mutation_lock(live_hashes);
        let _table_lock = crate::object::backing::tracked_vec_mutation_lock(live_table);
        let tracked =
            crate::object::gc::gc_is_tracked(live) || crate::object::gc::gc_is_tracked(staged);
        crate::object::backing::tracked_vec_swap_contents(live_order, dict_order(staged));
        crate::object::backing::tracked_vec_swap_contents(live_hashes, dict_hashes(staged));
        crate::object::backing::tracked_vec_swap_contents(live_table, dict_table(staged));
        let live_refs =
            (*header_from_obj_ptr(live)).has_flag(crate::object::HEADER_FLAG_CONTAINS_REFS);
        let staged_refs =
            (*header_from_obj_ptr(staged)).has_flag(crate::object::HEADER_FLAG_CONTAINS_REFS);
        for (object, has_refs) in [(live, staged_refs), (staged, live_refs)] {
            if has_refs {
                (*header_from_obj_ptr(object))
                    .fetch_or_flags(crate::object::HEADER_FLAG_CONTAINS_REFS);
            } else {
                (*header_from_obj_ptr(object))
                    .fetch_and_flags(!crate::object::HEADER_FLAG_CONTAINS_REFS);
            }
            dict_commit_structure(object);
            if tracked {
                crate::object::gc::gc_track_dict(py, object);
            }
        }
    }
}

/// Detached dictionary ownership, released only after the caller has published
/// any dependent metadata. No borrow of dictionary backing survives this value.
pub(crate) struct DetachedDictReferences<'a, 'py, Storage: AsRef<[u64]> = [u64; 2]> {
    py: &'a PyToken<'py>,
    bits: Storage,
}

impl<Storage: AsRef<[u64]>> Drop for DetachedDictReferences<'_, '_, Storage> {
    fn drop(&mut self) {
        for &bits in self.bits.as_ref() {
            dec_ref_bits(self.py, bits);
        }
    }
}

impl<Storage: AsRef<[u64]>> DetachedDictReferences<'_, '_, Storage> {
    /// Transfer the displaced references to the caller instead of releasing
    /// them. The dictionary was already published without them.
    pub(crate) fn into_owned_bits(self) -> Storage {
        let detached = std::mem::ManuallyDrop::new(self);
        // SAFETY: `detached` is never dropped, so `bits` moves out exactly once.
        unsafe { std::ptr::read(&detached.bits) }
    }
}

/// Retain, publish and promote tracking; transfer the displaced reference to the
/// caller so destructor re-entry cannot observe a partially committed owner.
#[inline]
unsafe fn dict_commit_value_replacement<'a, 'py>(
    _py: &'a PyToken<'py>,
    ptr: *mut u8,
    value_index: usize,
    new_bits: u64,
) -> DetachedDictReferences<'a, 'py> {
    DetachedDictReferences {
        py: _py,
        bits: [
            unsafe { dict_replace_value(_py, ptr, value_index, new_bits) },
            0,
        ],
    }
}

/// The one value-replacement commit. It returns the displaced owned reference,
/// or zero when the slot already holds `new_bits`, for release only after every
/// dependent commit.
#[inline]
unsafe fn dict_replace_value(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    value_index: usize,
    new_bits: u64,
) -> u64 {
    unsafe {
        let old_bits = dict_order(ptr)[value_index];
        if old_bits == new_bits {
            return 0;
        }
        if crate::object::refcount_opt::is_heap_ref(new_bits) {
            inc_ref_bits(_py, new_bits);
            (*header_from_obj_ptr(ptr)).fetch_or_flags(crate::object::HEADER_FLAG_CONTAINS_REFS);
        }
        // Publish the slot and tracking state before releasing the old edge.
        dict_order(ptr)[value_index] = new_bits;
        crate::object::gc::gc_track_dict_references(_py, ptr, &[new_bits]);
        old_bits
    }
}

#[inline]
unsafe fn dict_commit_insertion(_py: &PyToken<'_>, ptr: *mut u8, key_bits: u64, value_bits: u64) {
    unsafe {
        if crate::object::refcount_opt::is_heap_ref(key_bits)
            || crate::object::refcount_opt::is_heap_ref(value_bits)
        {
            (*header_from_obj_ptr(ptr)).fetch_or_flags(crate::object::HEADER_FLAG_CONTAINS_REFS);
        }
        dict_commit_structure(ptr);
        crate::object::gc::gc_track_dict_references(_py, ptr, &[key_bits, value_bits]);
    }
}

pub(crate) extern "C" fn dict_keys_method(self_bits: u64) -> i64 {
    molt_dict_keys(self_bits) as i64
}

pub(crate) extern "C" fn dict_values_method(self_bits: u64) -> i64 {
    molt_dict_values(self_bits) as i64
}

pub(crate) extern "C" fn dict_items_method(self_bits: u64) -> i64 {
    molt_dict_items(self_bits) as i64
}

pub(crate) extern "C" fn dict_get_method(self_bits: u64, key_bits: u64, default_bits: u64) -> i64 {
    molt_dict_get(self_bits, key_bits, default_bits) as i64
}

pub(crate) extern "C" fn dict_clear_method(self_bits: u64) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(self_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict.clear expects dict");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(_py, "TypeError", "dict.clear expects dict");
            }
            dict_clear_in_place(_py, ptr);
        }
        MoltObject::none().bits() as i64
    })
}

pub(crate) extern "C" fn dict_copy_method(self_bits: u64) -> i64 {
    crate::object::ops_dict::molt_dict_copy(self_bits) as i64
}

pub(crate) extern "C" fn dict_popitem_method(self_bits: u64) -> i64 {
    crate::object::ops_dict::molt_dict_popitem(self_bits) as i64
}

pub(crate) extern "C" fn dict_setdefault_method(
    self_bits: u64,
    key_bits: u64,
    default_bits: u64,
) -> i64 {
    molt_dict_setdefault(self_bits, key_bits, default_bits) as i64
}

pub(crate) extern "C" fn dict_fromkeys_method(
    self_bits: u64,
    iterable_bits: u64,
    default_bits: u64,
) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        let class_bits = if let Some(ptr) = maybe_ptr_from_bits(self_bits) {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_TYPE {
                    self_bits
                } else {
                    type_of_bits(_py, self_bits)
                }
            }
        } else {
            type_of_bits(_py, self_bits)
        };
        let builtins = builtin_classes(_py);
        if !issubclass_bits(class_bits, builtins.dict) {
            return raise_exception::<_>(_py, "TypeError", "dict.fromkeys expects dict type");
        }
        let capacity_hint = {
            let obj = obj_from_bits(iterable_bits);
            let mut hint = if let Some(ptr) = obj.as_ptr() {
                unsafe {
                    match object_type_id(ptr) {
                        TYPE_ID_LIST => list_len(ptr),
                        TYPE_ID_TUPLE => tuple_len(ptr),
                        TYPE_ID_DICT => dict_len(ptr),
                        TYPE_ID_SET | TYPE_ID_FROZENSET => set_len(ptr),
                        TYPE_ID_DICT_KEYS_VIEW
                        | TYPE_ID_DICT_VALUES_VIEW
                        | TYPE_ID_DICT_ITEMS_VIEW => dict_view_len(ptr),
                        TYPE_ID_BYTES | TYPE_ID_BYTEARRAY => bytes_len(ptr),
                        TYPE_ID_STRING => string_len(ptr),
                        TYPE_ID_RANGE => {
                            if let Some((start, stop, step)) = range_components_bigint(ptr) {
                                let len = range_len_bigint(&start, &stop, &step);
                                len.to_usize().unwrap_or(usize::MAX)
                            } else {
                                0
                            }
                        }
                        _ => 0,
                    }
                }
            } else {
                0
            };
            let max_entries = (isize::MAX as usize) / 2;
            if hint > max_entries {
                hint = max_entries;
            }
            hint
        };
        let dict_bits = if class_bits == builtins.dict {
            molt_dict_new(capacity_hint as u64)
        } else {
            let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
                return MoltObject::none().bits() as i64;
            };
            unsafe { call_class_init_with_args(_py, class_ptr, &[]) }
        };
        if exception_pending(_py) {
            return MoltObject::none().bits() as i64;
        }
        let iter_bits = molt_iter(iterable_bits);
        if obj_from_bits(iter_bits).is_none() {
            return raise_not_iterable(_py, iterable_bits);
        }
        loop {
            let pair_bits = molt_iter_next(iter_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits() as i64;
            }
            let pair_obj = obj_from_bits(pair_bits);
            let Some(pair_ptr) = pair_obj.as_ptr() else {
                return MoltObject::none().bits() as i64;
            };
            unsafe {
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                }
                let Some((key_bits, done_bits)) =
                    crate::object::seq_access::with_immutable_tuple_slice(pair_ptr, |items| {
                        items.first().copied().zip(items.get(1).copied())
                    })
                    .flatten()
                else {
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                };
                if is_truthy(_py, obj_from_bits(done_bits)) {
                    break;
                }
                let _ = molt_store_index(dict_bits, key_bits, default_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits() as i64;
                }
            }
        }
        dict_bits as i64
    })
}

pub(crate) extern "C" fn dict_update_method(self_bits: u64, other_bits: u64) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        if other_bits == missing_bits(_py) {
            return MoltObject::none().bits() as i64;
        }
        molt_dict_update(self_bits, other_bits) as i64
    })
}

pub(crate) unsafe fn dict_update_set_via_store(
    _py: &PyToken<'_>,
    target_bits: u64,
    key_bits: u64,
    val_bits: u64,
) {
    crate::gil_assert();
    let _ = molt_store_index(target_bits, key_bits, val_bits);
}

/// Class layout metadata maps refuse every write.
#[inline]
unsafe fn dict_layout_frozen(dict: *mut u8) -> bool {
    unsafe { (*header_from_obj_ptr(dict)).has_flag(crate::object::HEADER_FLAG_FROZEN_LAYOUT_MAP) }
}

/// Admission for every increment projection, including callback-free writes.
unsafe fn dict_increment_writable(py: &PyToken<'_>, dict: *mut u8) -> bool {
    unsafe {
        if dict_layout_frozen(dict) {
            raise_exception::<()>(py, "TypeError", "class layout metadata is immutable");
            false
        } else {
            true
        }
    }
}

#[inline]
fn inline_increment_sum(current: u64, delta: u64) -> Option<u64> {
    let sum = obj_from_bits(current)
        .as_int()?
        .checked_add(obj_from_bits(delta).as_int()?)?;
    MoltObject::try_from_int(sum).map(|sum| sum.bits())
}

/// The arithmetic boundary owns the mapping, key, delta and selected value.
/// No dictionary view or index survives Python arithmetic; the assignment is a
/// fresh lookup, just as d[key] = d.get(key, 0) + delta evaluates read then write.
pub(crate) unsafe fn dict_inc_in_place(
    py: &PyToken<'_>,
    dict: *mut u8,
    key: u64,
    delta: u64,
) -> bool {
    unsafe {
        if !dict_increment_writable(py, dict) {
            return false;
        }
        let dictionary = MoltObject::from_ptr(dict).bits();
        for bits in [dictionary, key, delta] {
            inc_ref_bits(py, bits);
        }
        let result = (|| {
            let current =
                dict_get_in_place(py, dict, key).unwrap_or(MoltObject::from_int(0).bits());
            if exception_pending(py) {
                return false;
            }
            inc_ref_bits(py, current);
            let sum =
                inline_increment_sum(current, delta).unwrap_or_else(|| molt_add(current, delta));
            if !exception_pending(py) {
                dict_set_in_place(py, dict, key, sum);
            }
            // __add__ returning None is a valid value, not a failure sentinel.
            dec_ref_bits(py, sum);
            dec_ref_bits(py, current);
            !exception_pending(py)
        })();
        for bits in [delta, key, dictionary] {
            dec_ref_bits(py, bits);
        }
        result && !exception_pending(py)
    }
}

/// A direct update exists only for inline operands and an inline result. It
/// cannot allocate, dispatch arithmetic, or release a mortal old value.
unsafe fn dict_increment_scalar_entry(
    py: &PyToken<'_>,
    dict: *mut u8,
    entry: usize,
    delta: u64,
) -> bool {
    unsafe {
        let index = entry * 2 + 1;
        let current = dict_order(dict)[index];
        let Some(sum) = inline_increment_sum(current, delta) else {
            return false;
        };
        drop(dict_commit_value_replacement(py, dict, index, sum));
        true
    }
}

/// `d[key] = d.get(key, 0) + delta` as one fused statement, when it provably
/// runs no Python code: `d` is a writable exact dict, `key` an exact str whose
/// probe decides without Python equality, and `delta` and any value found are
/// exact ints or bools. Then the key object is the one the statement inserts
/// and the value exactly the statement's. `Ok(false)` has done nothing and the
/// caller runs the statement itself; `Err(())` has the exception pending.
pub(crate) unsafe fn dict_increment_exact_statement(
    py: &PyToken<'_>,
    dict_bits: u64,
    key_bits: u64,
    delta_bits: u64,
) -> Result<bool, ()> {
    unsafe {
        let Some(dict) = obj_from_bits(dict_bits).as_ptr() else {
            return Ok(false);
        };
        if !object_is_exact_builtin_dict(py, dict)
            || dict_layout_frozen(dict)
            || !exact_int_or_bool_bits(delta_bits)
        {
            return Ok(false);
        }
        let Some(key) = exact_string_bytes(py, key_bits) else {
            return Ok(false);
        };
        let hash = hash_string_bytes(py, key) as u64;
        match dict_exact_string_lookup(py, dict, key, hash) {
            ExactStringLookup::Found(entry) => {
                if !exact_int_or_bool_bits(dict_order(dict)[entry * 2 + 1]) {
                    return Ok(false);
                }
                if dict_increment_scalar_entry(py, dict, entry, delta_bits) {
                    profile_hit_unchecked(&DICT_STR_INT_PREHASH_HIT_COUNT);
                    return Ok(true);
                }
            }
            ExactStringLookup::Absent => {}
            ExactStringLookup::Undecided => return Ok(false),
        }
        profile_hit_unchecked(&DICT_STR_INT_PREHASH_MISS_COUNT);
        if dict_inc_in_place(py, dict, key_bits, delta_bits) {
            Ok(true)
        } else {
            Err(())
        }
    }
}

/// Outcome of the callback-free exact `str` probe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExactStringLookup {
    /// The entry ordinary lookup selects.
    Found(usize),
    /// Ordinary lookup misses without calling Python.
    Absent,
    /// A same-hash key of another kind precedes any exact match: only ordinary
    /// equality, which may call Python, can decide.
    Undecided,
}

/// Borrow the bytes of an exact `str`; any other object is `None`. Hashing and
/// comparing exact `str` bytes cannot call Python.
#[inline]
unsafe fn exact_string_bytes<'s>(py: &PyToken<'_>, bits: u64) -> Option<&'s [u8]> {
    unsafe {
        let ptr = obj_from_bits(bits).as_ptr()?;
        let class = object_class_bits(ptr);
        if object_type_id(ptr) != TYPE_ID_STRING || (class != 0 && class != builtin_classes(py).str)
        {
            return None;
        }
        Some(std::slice::from_raw_parts(
            string_bytes(ptr),
            string_len(ptr),
        ))
    }
}

/// Allocation- and callback-free probe for an exact `str` key given by its bytes
/// and `str` hash. Whenever it decides, ordinary lookup reaches the same answer
/// without calling Python: every earlier same-hash candidate is an exact `str`.
/// A same-hash key of any other kind ends speculation, since its equality may
/// match or have side effects.
pub(crate) unsafe fn dict_exact_string_lookup(
    py: &PyToken<'_>,
    dict: *mut u8,
    token: &[u8],
    hash: u64,
) -> ExactStringLookup {
    unsafe {
        let table = dict_table(dict);
        if table.is_empty() {
            return ExactStringLookup::Absent;
        }
        let mask = table.len() - 1;
        let mut slot = hash as usize & mask;
        for _ in 0..table.len() {
            let entry = table[slot];
            if entry == 0 {
                return ExactStringLookup::Absent;
            }
            if entry != TABLE_TOMBSTONE {
                let index = entry - 1;
                if dict_hashes(dict).get(index).copied() == Some(hash) {
                    let Some(key_index) = index.checked_mul(2) else {
                        return ExactStringLookup::Undecided;
                    };
                    let Some(&key) = dict_order(dict).get(key_index) else {
                        return ExactStringLookup::Undecided;
                    };
                    let Some(key) = exact_string_bytes(py, key) else {
                        return ExactStringLookup::Undecided;
                    };
                    if key.len() == token.len()
                        && (key.as_ptr() == token.as_ptr()
                            || simd_bytes_eq(key.as_ptr(), token.as_ptr(), token.len()))
                    {
                        return ExactStringLookup::Found(index);
                    }
                }
            }
            slot = (slot + 1) & mask;
        }
        // A table without an empty slot is corrupt; ordinary lookup reports it.
        ExactStringLookup::Undecided
    }
}

/// An exact int or bool: adding it to another runs no Python code.
#[inline]
fn exact_int_or_bool_bits(bits: u64) -> bool {
    let obj = obj_from_bits(bits);
    obj.is_int() || obj.is_bool() || bigint_ptr_from_bits(bits).is_some()
}

/// `d[word] = d.get(word, 0) + delta` for one word of a validated fused
/// split/count loop: the word's probe decides without Python equality and any
/// value it finds, like `delta`, is an exact int or bool, so nothing here runs
/// Python code. `Some(Some(key))` is the key object this call inserted (owned),
/// `Some(None)` an existing key; `None` has the exception pending.
unsafe fn dict_increment_validated_word(
    py: &PyToken<'_>,
    dict: *mut u8,
    word: &[u8],
    delta: u64,
) -> Option<Option<u64>> {
    unsafe {
        let hash = hash_string_bytes(py, word) as u64;
        if let ExactStringLookup::Found(entry) = dict_exact_string_lookup(py, dict, word, hash) {
            if dict_increment_scalar_entry(py, dict, entry, delta) {
                return Some(None);
            }
            let key = dict_order(dict)[entry * 2];
            inc_ref_bits(py, key);
            let done = dict_inc_in_place(py, dict, key, delta);
            dec_ref_bits(py, key);
            return done.then_some(None);
        }
        let key_ptr = alloc_string(py, word);
        if key_ptr.is_null() {
            return None;
        }
        let key = MoltObject::from_ptr(key_ptr).bits();
        if !dict_inc_in_place(py, dict, key, delta) {
            dec_ref_bits(py, key);
            return None;
        }
        Some(Some(key))
    }
}

/// A fused split/count loop handles at most this many words: its kernel runs
/// between two of its loop's observations of pending asynchronous work, so a
/// longer line runs the ordinary loop, which observes them per word.
const SPLIT_COUNT_MAX_WORDS: usize = 1 << 12;

/// The words of `line` as `str.split(sep)` delimits them, whitespace words when
/// `sep` is `None`, in order; `None` when there are more than `limit`, found
/// without scanning past the word after the limit.
fn split_word_bounds(line: &[u8], sep: Option<&[u8]>, limit: usize) -> Option<Vec<(usize, usize)>> {
    let Some(sep) = sep else {
        // `limit` splits leave the rest of the line as one more part.
        let maxsplit = i64::try_from(limit).unwrap_or(i64::MAX);
        let parts = crate::builtins::strings::string_whitespace_parts(line, maxsplit, false);
        return (parts.len() <= limit).then_some(parts);
    };
    let separators: Box<dyn Iterator<Item = usize> + '_> = if sep.len() == 1 {
        Box::new(memchr::memchr_iter(sep[0], line))
    } else {
        Box::new(memmem::find_iter(line, sep))
    };
    let mut bounds = Vec::new();
    let mut start = 0usize;
    for index in separators {
        if bounds.len() == limit {
            return None;
        }
        bounds.push((start, index));
        start = index + sep.len();
    }
    if bounds.len() == limit {
        return None;
    }
    bounds.push((start, line.len()));
    Some(bounds)
}

enum SplitIncrement {
    /// The loop ran; the loop target's final value, owned.
    Done(u64),
    /// Nothing observable happened: the loop runs itself.
    Declined,
    /// The exception is pending.
    Failed,
}

/// A fused `for word in line.split([sep]): d[word] = d.get(word, 0) + delta`.
/// It runs no Python code when `line` and `sep` are exact strs (`sep`
/// non-empty), `d` is a writable exact dict, `delta` is an exact int or bool,
/// every word's probe of `d` decides without Python equality, every value found
/// is an exact int or bool, and releasing the loop target's previous value runs
/// nothing. All of that is checked before `d` changes; the words are the ones
/// `str.split` produces. The target's final value is the last word: the dict's
/// key object when that word was inserted by its own iteration, else a new str.
unsafe fn split_dict_increment_exact(
    py: &PyToken<'_>,
    line_bits: u64,
    sep_bits: Option<u64>,
    dict_bits: u64,
    delta_bits: u64,
    target_bits: u64,
) -> SplitIncrement {
    unsafe {
        if !crate::object::ops_vec::loop_target_release_is_inert(py, target_bits) {
            return SplitIncrement::Declined;
        }
        let Some(line) = exact_string_bytes(py, line_bits) else {
            return SplitIncrement::Declined;
        };
        let sep = match sep_bits {
            None => None,
            Some(bits) => match exact_string_bytes(py, bits) {
                Some(sep) if !sep.is_empty() => Some(sep),
                _ => return SplitIncrement::Declined,
            },
        };
        let Some(dict) = obj_from_bits(dict_bits).as_ptr() else {
            return SplitIncrement::Declined;
        };
        if !object_is_exact_builtin_dict(py, dict)
            || dict_layout_frozen(dict)
            || !exact_int_or_bool_bits(delta_bits)
        {
            return SplitIncrement::Declined;
        }
        let Some(bounds) = split_word_bounds(line, sep, SPLIT_COUNT_MAX_WORDS) else {
            return SplitIncrement::Declined;
        };
        if bounds.is_empty() {
            return SplitIncrement::Declined;
        }
        for &(start, end) in &bounds {
            let word = &line[start..end];
            let hash = hash_string_bytes(py, word) as u64;
            match dict_exact_string_lookup(py, dict, word, hash) {
                ExactStringLookup::Found(entry) => {
                    if !exact_int_or_bool_bits(dict_order(dict)[entry * 2 + 1]) {
                        return SplitIncrement::Declined;
                    }
                }
                ExactStringLookup::Absent => {}
                ExactStringLookup::Undecided => return SplitIncrement::Declined,
            }
        }
        let last_index = bounds.len() - 1;
        let mut inserted_last = None;
        for (index, &(start, end)) in bounds.iter().enumerate() {
            match dict_increment_validated_word(py, dict, &line[start..end], delta_bits) {
                Some(Some(key)) if index == last_index => inserted_last = Some(key),
                Some(Some(key)) => dec_ref_bits(py, key),
                Some(None) => {}
                None => return SplitIncrement::Failed,
            }
        }
        match inserted_last {
            Some(key) => SplitIncrement::Done(key),
            None => {
                let (start, end) = bounds[last_index];
                let word = alloc_string(py, &line[start..end]);
                if word.is_null() {
                    return SplitIncrement::Failed;
                }
                SplitIncrement::Done(MoltObject::from_ptr(word).bits())
            }
        }
    }
}

/// `(last, ok)`: `ok` when the fused loop ran and `last` is the loop target's
/// final value; otherwise the caller runs the loop.
fn split_dict_increment(
    py: &PyToken<'_>,
    line_bits: u64,
    sep_bits: Option<u64>,
    dict_bits: u64,
    delta_bits: u64,
    target_bits: u64,
) -> u64 {
    let none = MoltObject::none().bits();
    let (last, ok) = match unsafe {
        split_dict_increment_exact(py, line_bits, sep_bits, dict_bits, delta_bits, target_bits)
    } {
        SplitIncrement::Done(last) => (last, true),
        SplitIncrement::Declined => (none, false),
        SplitIncrement::Failed => return none,
    };
    let pair = alloc_tuple(py, &[last, MoltObject::from_bool(ok).bits()]);
    dec_ref_bits(py, last);
    if pair.is_null() {
        return none;
    }
    MoltObject::from_ptr(pair).bits()
}

/// `for word in line.split(): d[word] = d.get(word, 0) + delta`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_string_split_ws_dict_inc(
    line_bits: u64,
    dict_bits: u64,
    delta_bits: u64,
    target_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        split_dict_increment(_py, line_bits, None, dict_bits, delta_bits, target_bits)
    })
}

/// `for word in line.split(sep): d[word] = d.get(word, 0) + delta`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_string_split_sep_dict_inc(
    line_bits: u64,
    sep_bits: u64,
    dict_bits: u64,
    delta_bits: u64,
    target_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        split_dict_increment(
            _py,
            line_bits,
            Some(sep_bits),
            dict_bits,
            delta_bits,
            target_bits,
        )
    })
}

pub(crate) fn checked_dict_table_capacity(entries: usize) -> Option<usize> {
    let mut cap = entries.checked_mul(2)?.checked_next_power_of_two()?;
    if cap < 8 {
        cap = 8;
    }
    Some(cap)
}

pub(crate) fn dict_table_capacity(entries: usize) -> usize {
    checked_dict_table_capacity(entries)
        .expect("live dict entry count must fit the addressable table capacity")
}

const TABLE_TOMBSTONE: usize = usize::MAX;

#[inline]
unsafe fn reserve_dict_order(_py: &PyToken<'_>, order: &mut Vec<u64>, additional: usize) -> bool {
    let Some(required_len) = order.len().checked_add(additional) else {
        let _ = raise_exception::<u64>(_py, "MemoryError", "dict allocation failed");
        return false;
    };
    unsafe {
        crate::object::backing::tracked_vec_reserve_or_raise(
            _py,
            order as *mut Vec<u64>,
            required_len,
            "dict allocation failed",
        )
    }
}

#[inline]
unsafe fn reserve_set_order(_py: &PyToken<'_>, order: &mut Vec<u64>, additional: usize) -> bool {
    let Some(required_len) = order.len().checked_add(additional) else {
        let _ = raise_exception::<u64>(_py, "MemoryError", "set allocation failed");
        return false;
    };
    unsafe {
        crate::object::backing::tracked_vec_reserve_or_raise(
            _py,
            order as *mut Vec<u64>,
            required_len,
            "set allocation failed",
        )
    }
}

#[inline]
unsafe fn reserve_hashes(
    _py: &PyToken<'_>,
    hashes: &mut Vec<u64>,
    additional: usize,
    message: &'static str,
) -> bool {
    let Some(required_len) = hashes.len().checked_add(additional) else {
        let _ = raise_exception::<u64>(_py, "MemoryError", message);
        return false;
    };
    unsafe {
        crate::object::backing::tracked_vec_reserve_or_raise(
            _py,
            hashes as *mut Vec<u64>,
            required_len,
            message,
        )
    }
}

fn dict_insert_entry(_py: &PyToken<'_>, hashes: &[u64], table: &mut [usize], entry_idx: usize) {
    let mask = table.len() - 1;
    let hash = hashes[entry_idx];
    let mut slot = (hash as usize) & mask;
    let mut first_tombstone = None;
    loop {
        let entry = table[slot];
        if entry == 0 {
            let target = first_tombstone.unwrap_or(slot);
            table[target] = entry_idx + 1;
            return;
        }
        if entry == TABLE_TOMBSTONE && first_tombstone.is_none() {
            first_tombstone = Some(slot);
        }
        slot = (slot + 1) & mask;
    }
}

pub(crate) fn dict_insert_entry_with_hash(
    _py: &PyToken<'_>,
    _order: &[u64],
    table: &mut [usize],
    entry_idx: usize,
    hash: u64,
) {
    let mask = table.len() - 1;
    let mut slot = (hash as usize) & mask;
    let mut first_tombstone = None;
    loop {
        let entry = table[slot];
        if entry == 0 {
            let target = first_tombstone.unwrap_or(slot);
            table[target] = entry_idx + 1;
            return;
        }
        if entry == TABLE_TOMBSTONE && first_tombstone.is_none() {
            first_tombstone = Some(slot);
        }
        slot = (slot + 1) & mask;
    }
}
pub(crate) fn dict_rebuild(
    _py: &PyToken<'_>,
    order: &[u64],
    hashes: &[u64],
    table: &mut Vec<usize>,
    capacity: usize,
) {
    if !unsafe {
        crate::object::backing::tracked_vec_reserve_or_raise(
            _py,
            table as *mut Vec<usize>,
            capacity,
            "dict allocation failed",
        )
    } {
        return;
    }
    table.clear();
    table.resize(capacity, 0);
    let entry_count = order.len() / 2;
    for entry_idx in 0..entry_count {
        dict_insert_entry(_py, hashes, table, entry_idx);
    }
}

/// The dictionary key's hashability/hash protocol, shared by ordinary reads,
/// writes and setdefault. A successful result hashes a user key exactly once.
#[inline]
unsafe fn dict_key_hash(py: &PyToken<'_>, key_bits: u64) -> Option<u64> {
    unsafe {
        if !exception_pending(py) && exact_string_bytes(py, key_bits).is_some() {
            return Some(crate::object::ops_hash::hash_string(
                py,
                obj_from_bits(key_bits).as_ptr().unwrap(),
            ) as u64);
        }
        if !ensure_hashable(py, key_bits, HashContext::DictKey) {
            return None;
        }
        let pending_before = exception_pending(py);
        let previous = exception_last_bits_noinc(py);
        let hash = hash_bits(py, key_bits);
        if exception_pending(py) && (!pending_before || exception_last_bits_noinc(py) != previous) {
            None
        } else {
            Some(hash)
        }
    }
}

/// Admit ordinary dictionary lookup once for reads, membership, and
/// mutation lookup phases. Exact strings use the shared callback-free
/// probe; Undecided resumes general lookup before equality has run.
/// No backing or string-byte borrow crosses a hash/equality callback.
#[inline]
pub(crate) unsafe fn dict_find_entry(
    py: &PyToken<'_>,
    dict: *mut u8,
    key_bits: u64,
) -> Option<usize> {
    unsafe {
        let key = obj_from_bits(key_bits);
        // Existing pending exceptions retain the ordinary read contract. In
        // particular, do not turn a failed hashability admission into a hit.
        if !exception_pending(py)
            && let Some(bytes) = exact_string_bytes(py, key_bits)
        {
            // Reuse the string object's canonical cached hash. Rehashing bytes
            // here would make repeated reads of long keys linear in key length.
            let hash = crate::object::ops_hash::hash_string(py, key.as_ptr().unwrap()) as u64;
            match dict_exact_string_lookup(py, dict, bytes, hash) {
                ExactStringLookup::Found(index) => return Some(index),
                ExactStringLookup::Absent => return None,
                ExactStringLookup::Undecided => {}
            }
        }
        let pending_before = exception_pending(py);
        let previous = if pending_before {
            exception_last_bits_noinc(py).unwrap_or(0)
        } else {
            0
        };
        let hash = dict_key_hash(py, key_bits)?;
        let found = dict_find_entry_with_hash(py, dict, key_bits, hash);
        if exception_pending(py)
            && (!pending_before || exception_last_bits_noinc(py).unwrap_or(0) != previous)
        {
            return None;
        }
        found
    }
}

// ---------------------------------------------------------------------------
// SIMD-accelerated byte-level equality for string/bytes comparisons.
// For short strings (< 32 bytes), the compiler-generated memcmp is fast enough.
// For longer strings, explicit SIMD provides measurable wins especially on
// Apple Silicon where NEON is always available with no runtime detection cost.
// ---------------------------------------------------------------------------

/// SIMD byte equality: returns true if `a[..len] == b[..len]`.
/// Precondition: both pointers are valid for `len` bytes.
#[inline(always)]
pub(in crate::object) unsafe fn simd_bytes_eq(a: *const u8, b: *const u8, len: usize) -> bool {
    unsafe {
        // Tiny strings (<=8 bytes): direct comparison, no SIMD overhead.
        if len <= 8 {
            if len == 0 {
                return true;
            }
            return std::slice::from_raw_parts(a, len) == std::slice::from_raw_parts(b, len);
        }

        // Short strings (9-15 bytes): compare overlapping 8-byte windows.
        // This covers the full range without underflowing the tail pointer.
        if len < 16 {
            return simd_bytes_eq_short_u64(a, b, len);
        }

        // Short strings (16-31 bytes): use NEON/SSE2 16-byte loads instead of
        // scalar memcmp. Two overlapping 16-byte loads cover any length in
        // 16..31 without a loop, which is measurably faster for dict-key
        // comparisons where keys are typically short identifiers (< 32 bytes).
        #[cfg(target_arch = "aarch64")]
        if len < 32 {
            return simd_bytes_eq_short_neon(a, b, len);
        }
        #[cfg(target_arch = "x86_64")]
        if len < 32 {
            if std::arch::is_x86_feature_detected!("sse2") {
                return simd_bytes_eq_short_sse2(a, b, len);
            }
            return std::slice::from_raw_parts(a, len) == std::slice::from_raw_parts(b, len);
        }
        #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
        if len < 32 {
            return std::slice::from_raw_parts(a, len) == std::slice::from_raw_parts(b, len);
        }

        // Long strings (>= 32 bytes): full SIMD loops.
        #[cfg(target_arch = "x86_64")]
        {
            if std::arch::is_x86_feature_detected!("avx2") {
                return simd_bytes_eq_avx2(a, b, len);
            }
            return simd_bytes_eq_sse2(a, b, len);
        }
        #[cfg(target_arch = "aarch64")]
        {
            return simd_bytes_eq_neon(a, b, len);
        }
        #[cfg(target_arch = "wasm32")]
        {
            return simd_bytes_eq_wasm32(a, b, len);
        }
        #[allow(unreachable_code)]
        {
            std::slice::from_raw_parts(a, len) == std::slice::from_raw_parts(b, len)
        }
    }
}

/// Short-string equality for 9-15 bytes: overlapping unaligned 8-byte loads.
#[inline(always)]
unsafe fn simd_bytes_eq_short_u64(a: *const u8, b: *const u8, len: usize) -> bool {
    debug_assert!((9..16).contains(&len));
    unsafe {
        let head_a = std::ptr::read_unaligned(a as *const u64);
        let head_b = std::ptr::read_unaligned(b as *const u64);
        if head_a != head_b {
            return false;
        }
        let tail_a = std::ptr::read_unaligned(a.add(len - 8) as *const u64);
        let tail_b = std::ptr::read_unaligned(b.add(len - 8) as *const u64);
        tail_a == tail_b
    }
}

/// NEON short-string equality for 16-31 bytes: two overlapping 16-byte loads.
#[cfg(target_arch = "aarch64")]
#[inline(always)]
unsafe fn simd_bytes_eq_short_neon(a: *const u8, b: *const u8, len: usize) -> bool {
    use std::arch::aarch64::*;
    debug_assert!((16..32).contains(&len));
    unsafe {
        // Load from the start
        let va0 = vld1q_u8(a);
        let vb0 = vld1q_u8(b);
        let cmp0 = vceqq_u8(va0, vb0);
        // Load from (end - 16), overlapping with the first load for short strings
        let va1 = vld1q_u8(a.add(len - 16));
        let vb1 = vld1q_u8(b.add(len - 16));
        let cmp1 = vceqq_u8(va1, vb1);
        // Both loads must match: AND the comparison results and check all-0xFF
        let combined = vandq_u8(cmp0, cmp1);
        vminvq_u8(combined) == 0xFF
    }
}

/// SSE2 short-string equality for 16-31 bytes: two overlapping 16-byte loads.
#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn simd_bytes_eq_short_sse2(a: *const u8, b: *const u8, len: usize) -> bool {
    unsafe {
        use std::arch::x86_64::*;
        debug_assert!((16..32).contains(&len));
        // Load from the start
        let va0 = _mm_loadu_si128(a as *const __m128i);
        let vb0 = _mm_loadu_si128(b as *const __m128i);
        let cmp0 = _mm_cmpeq_epi8(va0, vb0);
        // Load from (end - 16), overlapping with the first load
        let va1 = _mm_loadu_si128(a.add(len - 16) as *const __m128i);
        let vb1 = _mm_loadu_si128(b.add(len - 16) as *const __m128i);
        let cmp1 = _mm_cmpeq_epi8(va1, vb1);
        // Both must be all-equal: AND the masks
        let mask0 = _mm_movemask_epi8(cmp0);
        let mask1 = _mm_movemask_epi8(cmp1);
        (mask0 & mask1) == 0xFFFF
    }
}

#[cfg(target_arch = "wasm32")]
#[inline]
unsafe fn simd_bytes_eq_wasm32(a: *const u8, b: *const u8, len: usize) -> bool {
    unsafe {
        use std::arch::wasm32::*;
        let mut i = 0usize;
        while i + 16 <= len {
            let va = v128_load(a.add(i) as *const v128);
            let vb = v128_load(b.add(i) as *const v128);
            let cmp = u8x16_eq(va, vb);
            if u8x16_bitmask(cmp) != 0xFFFF {
                return false;
            }
            i += 16;
        }
        std::slice::from_raw_parts(a.add(i), len - i)
            == std::slice::from_raw_parts(b.add(i), len - i)
    }
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn simd_bytes_eq_sse2(a: *const u8, b: *const u8, len: usize) -> bool {
    unsafe {
        use std::arch::x86_64::*;
        let mut i = 0usize;
        while i + 16 <= len {
            let va = _mm_loadu_si128(a.add(i) as *const __m128i);
            let vb = _mm_loadu_si128(b.add(i) as *const __m128i);
            let cmp = _mm_cmpeq_epi8(va, vb);
            if _mm_movemask_epi8(cmp) != 0xFFFF {
                return false;
            }
            i += 16;
        }
        // Tail: compare remaining bytes
        std::slice::from_raw_parts(a.add(i), len - i)
            == std::slice::from_raw_parts(b.add(i), len - i)
    }
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn simd_bytes_eq_avx2(a: *const u8, b: *const u8, len: usize) -> bool {
    unsafe {
        use std::arch::x86_64::*;
        let mut i = 0usize;
        while i + 32 <= len {
            let va = _mm256_loadu_si256(a.add(i) as *const __m256i);
            let vb = _mm256_loadu_si256(b.add(i) as *const __m256i);
            let cmp = _mm256_cmpeq_epi8(va, vb);
            if _mm256_movemask_epi8(cmp) != -1i32 {
                return false;
            }
            i += 32;
        }
        // SSE2 tail for 16-byte remainder
        if i + 16 <= len {
            let va = _mm_loadu_si128(a.add(i) as *const __m128i);
            let vb = _mm_loadu_si128(b.add(i) as *const __m128i);
            let cmp = _mm_cmpeq_epi8(va, vb);
            if _mm_movemask_epi8(cmp) != 0xFFFF {
                return false;
            }
            i += 16;
        }
        std::slice::from_raw_parts(a.add(i), len - i)
            == std::slice::from_raw_parts(b.add(i), len - i)
    }
}

#[cfg(target_arch = "aarch64")]
#[inline]
unsafe fn simd_bytes_eq_neon(a: *const u8, b: *const u8, len: usize) -> bool {
    unsafe {
        use std::arch::aarch64::*;
        let mut i = 0usize;
        while i + 16 <= len {
            let va = vld1q_u8(a.add(i));
            let vb = vld1q_u8(b.add(i));
            let cmp = vceqq_u8(va, vb);
            // vminvq_u8 returns 0xFF if all lanes equal, < 0xFF if any differ
            if vminvq_u8(cmp) != 0xFF {
                return false;
            }
            i += 16;
        }
        std::slice::from_raw_parts(a.add(i), len - i)
            == std::slice::from_raw_parts(b.add(i), len - i)
    }
}

unsafe fn string_bits_eq(_py: &PyToken<'_>, a_bits: u64, b_bits: u64) -> Option<bool> {
    unsafe {
        let a_obj = obj_from_bits(a_bits);
        let b_obj = obj_from_bits(b_bits);
        let a_ptr = a_obj.as_ptr()?;
        let b_ptr = b_obj.as_ptr()?;
        if object_type_id(a_ptr) != TYPE_ID_STRING || object_type_id(b_ptr) != TYPE_ID_STRING {
            return None;
        }
        if a_ptr == b_ptr {
            return Some(true);
        }
        if [a_ptr, b_ptr].into_iter().any(|ptr| {
            let class = object_class_bits(ptr);
            class != 0 && class != builtin_classes(_py).str
        }) {
            return None;
        }
        let a_len = string_len(a_ptr);
        let b_len = string_len(b_ptr);
        if a_len != b_len {
            return Some(false);
        }
        Some(simd_bytes_eq(
            string_bytes(a_ptr),
            string_bytes(b_ptr),
            a_len,
        ))
    }
}

pub(crate) unsafe fn dict_find_entry_with_hash(
    _py: &PyToken<'_>,
    dict: *mut u8,
    key_bits: u64,
    hash: u64,
) -> Option<usize> {
    unsafe {
        'restart: loop {
            let epoch = dict_structural_epoch(dict);
            let (table_address, table_len) = {
                let table = dict_table(dict);
                (table.as_ptr(), table.len())
            };
            if table_len == 0 {
                return None;
            }
            let mask = table_len - 1;
            let mut slot = hash as usize & mask;
            for _ in 0..table_len {
                let entry = dict_table(dict)[slot];
                if entry == 0 {
                    return None;
                }
                if entry != TABLE_TOMBSTONE {
                    let index = entry - 1;
                    let key_index = index.checked_mul(2);
                    let candidate =
                        key_index.and_then(|index| dict_order(dict).get(index).copied());
                    let candidate_hash = dict_hashes(dict).get(index).copied();
                    let (Some(candidate), Some(candidate_hash)) = (candidate, candidate_hash)
                    else {
                        return raise_exception::<_>(
                            _py,
                            "SystemError",
                            "dictionary table references an invalid entry",
                        );
                    };
                    if candidate_hash == hash {
                        if candidate == key_bits {
                            return Some(index);
                        }
                        if let Some(equal) = string_bits_eq(_py, candidate, key_bits) {
                            if equal {
                                return Some(index);
                            }
                        } else {
                            inc_ref_bits(_py, candidate);
                            let equal = eq_bool_from_bits(_py, candidate, key_bits);
                            dec_ref_bits(_py, candidate);
                            let equal = equal?;
                            // Python equality/truth/destruction may mutate this
                            // dict, including resizing or removing the candidate.
                            // Re-acquire backing and restart from its new table.
                            let unchanged = {
                                let table = dict_table(dict);
                                dict_structural_epoch(dict) == epoch
                                    && table.as_ptr() == table_address
                                    && table.len() == table_len
                                    && dict_order(dict).get(index * 2).copied() == Some(candidate)
                                    && dict_hashes(dict).get(index).copied() == Some(hash)
                            };
                            if !unchanged {
                                continue 'restart;
                            }
                            if equal {
                                return Some(index);
                            }
                        }
                    }
                }
                slot = (slot + 1) & mask;
            }
            return raise_exception::<_>(_py, "SystemError", "dictionary table has no empty slot");
        }
    }
}

/// Pointer-based cached-hash lookup for reentrant set consumers. No Vec
/// reference survives rich equality, its truth conversion, or candidate drop.
pub(crate) unsafe fn set_find_entry_in_place_with_hash(
    py: &PyToken<'_>,
    set: *mut u8,
    key: u64,
    hash: u64,
) -> Option<usize> {
    unsafe {
        'restart: loop {
            let (address, length) = {
                let table = set_table(set);
                (table.as_ptr(), table.len())
            };
            if length == 0 {
                return None;
            }
            let mask = length - 1;
            let mut slot = hash as usize & mask;
            for _ in 0..length {
                let entry = set_table(set)[slot];
                if entry == 0 {
                    return None;
                }
                if entry != TABLE_TOMBSTONE {
                    let index = entry - 1;
                    let candidate = set_order(set).get(index).copied();
                    let candidate_hash = set_hashes(set).get(index).copied();
                    let (Some(candidate), Some(candidate_hash)) = (candidate, candidate_hash)
                    else {
                        return raise_exception(
                            py,
                            "SystemError",
                            "set table references an invalid entry",
                        );
                    };
                    if candidate_hash == hash {
                        if candidate == key {
                            return Some(index);
                        }
                        if let Some(equal) = string_bits_eq(py, candidate, key) {
                            if equal {
                                return Some(index);
                            }
                        } else {
                            inc_ref_bits(py, candidate);
                            let equal = eq_bool_from_bits(py, candidate, key);
                            dec_ref_bits(py, candidate);
                            let equal = equal?;
                            let unchanged = {
                                let table = set_table(set);
                                table.as_ptr() == address
                                    && table.len() == length
                                    && table.get(slot).copied() == Some(entry)
                                    && set_order(set).get(index).copied() == Some(candidate)
                                    && set_hashes(set).get(index).copied() == Some(hash)
                            };
                            if !unchanged {
                                continue 'restart;
                            }
                            if equal {
                                return Some(index);
                            }
                        }
                    }
                }
                slot = (slot + 1) & mask;
            }
            return raise_exception(py, "SystemError", "set table has no empty slot");
        }
    }
}

pub(crate) fn set_table_capacity(entries: usize) -> usize {
    dict_table_capacity(entries)
}

fn set_insert_entry(_py: &PyToken<'_>, hashes: &[u64], table: &mut [usize], entry_idx: usize) {
    let mask = table.len() - 1;
    let mut slot = (hashes[entry_idx] as usize) & mask;
    let mut first_tombstone = None;
    loop {
        let entry = table[slot];
        if entry == 0 {
            let target = first_tombstone.unwrap_or(slot);
            table[target] = entry_idx + 1;
            return;
        }
        if entry == TABLE_TOMBSTONE && first_tombstone.is_none() {
            first_tombstone = Some(slot);
        }
        slot = (slot + 1) & mask;
    }
}

fn set_insert_entry_with_hash(
    _py: &PyToken<'_>,
    _order: &[u64],
    table: &mut [usize],
    entry_idx: usize,
    hash: u64,
) {
    let mask = table.len() - 1;
    let mut slot = (hash as usize) & mask;
    let mut first_tombstone = None;
    loop {
        let entry = table[slot];
        if entry == 0 {
            let target = first_tombstone.unwrap_or(slot);
            table[target] = entry_idx + 1;
            return;
        }
        if entry == TABLE_TOMBSTONE && first_tombstone.is_none() {
            first_tombstone = Some(slot);
        }
        slot = (slot + 1) & mask;
    }
}
pub(in crate::object) fn set_rebuild(
    _py: &PyToken<'_>,
    order: &[u64],
    hashes: &[u64],
    table: &mut Vec<usize>,
    capacity: usize,
) {
    crate::gil_assert();
    if !unsafe {
        crate::object::backing::tracked_vec_reserve_or_raise(
            _py,
            table as *mut Vec<usize>,
            capacity,
            "set allocation failed",
        )
    } {
        return;
    }
    table.clear();
    table.resize(capacity, 0);
    for entry_idx in 0..order.len() {
        set_insert_entry(_py, hashes, table, entry_idx);
    }
}

/// A lookup caller supplies an owner pointer, never a borrowed table triple.
pub(crate) unsafe fn set_find_entry(py: &PyToken<'_>, set: *mut u8, key: u64) -> Option<usize> {
    let _key = PinnedSetEntry::borrow(py, key, 0);
    let hash = hash_bits(py, key);
    if exception_pending(py) {
        return None;
    }
    unsafe { set_find_entry_in_place_with_hash(py, set, key, hash) }
}

/// Own one source element and its stored hash across reentrant operations.
pub(crate) struct PinnedSetEntry<'a, 'py> {
    py: &'a PyToken<'py>,
    bits: u64,
    hash: u64,
}

impl<'a, 'py> PinnedSetEntry<'a, 'py> {
    fn borrow(py: &'a PyToken<'py>, bits: u64, hash: u64) -> Self {
        inc_ref_bits(py, bits);
        Self { py, bits, hash }
    }
    pub(crate) fn bits(&self) -> u64 {
        self.bits
    }
    pub(crate) fn hash(&self) -> u64 {
        self.hash
    }
}

impl Drop for PinnedSetEntry<'_, '_> {
    fn drop(&mut self) {
        dec_ref_bits(self.py, self.bits);
    }
}

pub(crate) unsafe fn set_pin_entry<'a, 'py>(
    py: &'a PyToken<'py>,
    set: *mut u8,
    index: usize,
) -> Option<PinnedSetEntry<'a, 'py>> {
    unsafe {
        Some(PinnedSetEntry::borrow(
            py,
            *set_order(set).get(index)?,
            *set_hashes(set).get(index)?,
        ))
    }
}

pub(in crate::object) fn concat_bytes_like(
    _py: &PyToken<'_>,
    left: &[u8],
    right: &[u8],
    type_id: u32,
) -> Option<u64> {
    let total = left.len().checked_add(right.len())?;
    if type_id == TYPE_ID_BYTEARRAY {
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(left);
        out.extend_from_slice(right);
        let ptr = alloc_bytearray(_py, &out);
        if ptr.is_null() {
            return None;
        }
        return Some(MoltObject::from_ptr(ptr).bits());
    }
    // Dynamic sequence dispatch must select an admitted physical layout; no
    // arbitrary heap type ID may reach the inline storage constructor.
    let kind = match type_id {
        TYPE_ID_STRING => InlineBytesKind::String,
        TYPE_ID_BYTES => InlineBytesKind::Bytes,
        _ => return None,
    };
    let ptr = alloc_inline_bytes_with_len(_py, total, kind);
    if ptr.is_null() {
        return None;
    }
    unsafe {
        let data_ptr = crate::object::layout::InlineBytesStorage::data(ptr);
        std::ptr::copy_nonoverlapping(left.as_ptr(), data_ptr, left.len());
        std::ptr::copy_nonoverlapping(right.as_ptr(), data_ptr.add(left.len()), right.len());
    }
    Some(MoltObject::from_ptr(ptr).bits())
}

pub(in crate::object) fn fill_repeated_bytes(dst: &mut [u8], pattern: &[u8]) {
    if pattern.is_empty() {
        return;
    }
    if pattern.len() == 1 {
        dst.fill(pattern[0]);
        return;
    }
    let mut filled = pattern.len().min(dst.len());
    dst[..filled].copy_from_slice(&pattern[..filled]);
    while filled < dst.len() {
        let copy_len = std::cmp::min(filled, dst.len() - filled);
        let (head, tail) = dst.split_at_mut(filled);
        tail[..copy_len].copy_from_slice(&head[..copy_len]);
        filled += copy_len;
    }
}

pub(crate) unsafe fn dict_set_in_place(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    key_bits: u64,
    val_bits: u64,
) {
    drop(unsafe { dict_set_deferred(_py, ptr, key_bits, val_bits) });
}

/// Publish a mapping update while retaining displaced ownership until the
/// containing namespace has committed its derived facts. Errors never mint a
/// success receipt, and ordinary setters consume this same transaction.
pub(crate) unsafe fn dict_set_deferred<'a, 'py>(
    _py: &'a PyToken<'py>,
    ptr: *mut u8,
    key_bits: u64,
    val_bits: u64,
) -> Result<DetachedDictReferences<'a, 'py>, ()> {
    unsafe {
        crate::gil_assert();
        if (*header_from_obj_ptr(ptr)).has_flag(crate::object::HEADER_FLAG_FROZEN_LAYOUT_MAP) {
            raise_exception::<()>(_py, "TypeError", "class layout metadata is immutable");
            return Err(());
        }
        // Fast path: inline NaN-boxed ints bypass all exception checks,
        // hashability validation, and refcounting overhead.
        let key_obj = obj_from_bits(key_bits);
        if let Some(i) = key_obj.as_int() {
            return dict_set_with_hash_deferred(_py, ptr, key_bits, val_bits, hash_int(i) as u64);
        }
        let hash = dict_key_hash(_py, key_bits).ok_or(())?;
        if exception_pending(_py) {
            return Err(());
        }
        dict_set_with_hash_deferred(_py, ptr, key_bits, val_bits, hash)
    }
}

/// Merge prehashed dictionary entries without replaying Python __hash__.
pub(crate) unsafe fn dict_set_with_hash_in_place(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    key_bits: u64,
    val_bits: u64,
    hash: u64,
) {
    drop(unsafe { dict_set_with_hash_deferred(_py, ptr, key_bits, val_bits, hash) });
}

unsafe fn dict_set_with_hash_deferred<'a, 'py>(
    _py: &'a PyToken<'py>,
    ptr: *mut u8,
    key_bits: u64,
    val_bits: u64,
    hash: u64,
) -> Result<DetachedDictReferences<'a, 'py>, ()> {
    unsafe {
        if (*header_from_obj_ptr(ptr)).has_flag(crate::object::HEADER_FLAG_FROZEN_LAYOUT_MAP) {
            raise_exception::<()>(_py, "TypeError", "class layout metadata is immutable");
            return Err(());
        }
        let found = dict_find_entry_with_hash(_py, ptr, key_bits, hash);
        if exception_pending(_py) {
            return Err(());
        }
        if let Some(entry_idx) = found {
            let val_idx = entry_idx * 2 + 1;
            return Ok(dict_commit_value_replacement(_py, ptr, val_idx, val_bits));
        }

        if !dict_reserve_entries(_py, ptr, 1) {
            return Err(());
        }
        dict_append_reserved_entry(_py, ptr, key_bits, val_bits, hash);
        Ok(DetachedDictReferences {
            py: _py,
            bits: [0; 2],
        })
    }
}

/// Return an owned existing value, or insert and return one owned default.
/// `Some` borrows the supplied default; `None` requests the optimized fresh list.
/// Hash/equality run once through the shared authorities. After their final
/// live-table result, reservation, empty-list allocation and append cannot call
/// Python, so no absent-key probe or user hash is replayed before publication.
pub(crate) unsafe fn dict_setdefault_in_place(
    py: &PyToken<'_>,
    dict: *mut u8,
    key: u64,
    default: Option<u64>,
) -> Option<u64> {
    unsafe {
        crate::gil_assert();
        if exception_pending(py) {
            return None;
        }
        // Equality can remove the very namespace edges that supplied the call
        // operands. Keep them live without holding any dictionary backing view.
        let inputs = [MoltObject::from_ptr(dict).bits(), key, default.unwrap_or(0)];
        for bits in inputs {
            inc_ref_bits(py, bits);
        }
        let result = (|| {
            let hash = dict_key_hash(py, key)?;
            let found = dict_find_entry_with_hash(py, dict, key, hash);
            if exception_pending(py) {
                return None;
            }
            if let Some(index) = found {
                let value = dict_order(dict)[index * 2 + 1];
                inc_ref_bits(py, value);
                return Some(value);
            }
            if dict_layout_frozen(dict) {
                return raise_exception(py, "TypeError", "class layout metadata is immutable");
            }
            if !dict_reserve_entries(py, dict, 1) {
                return None;
            }
            let value = match default {
                Some(value) => {
                    inc_ref_bits(py, value);
                    value
                }
                None => {
                    // alloc_list records GC work for a later safepoint; it does
                    // not run Python here. The fresh list's initial owner is
                    // transferred to the result, not retained an extra time.
                    let list = alloc_list(py, &[]);
                    if list.is_null() {
                        return None;
                    }
                    MoltObject::from_ptr(list).bits()
                }
            };
            dict_append_reserved_entry(py, dict, key, value, hash);
            Some(value)
        })();
        for bits in inputs.into_iter().rev() {
            dec_ref_bits(py, bits);
        }
        result
    }
}

/// Reserve table load, order and hash capacity for `added` new entries, so the
/// appends that follow cannot grow or fail. Calls no Python and moves no entry:
/// every index and the structural epoch are unchanged.
#[inline]
unsafe fn dict_reserve_entries(_py: &PyToken<'_>, ptr: *mut u8, added: usize) -> bool {
    unsafe {
        let order = dict_order(ptr);
        let hashes = dict_hashes(ptr);
        let table = dict_table(ptr);
        let new_entries = (order.len() / 2) + added;
        let needs_resize = table.is_empty() || new_entries * 10 >= table.len() * 7;
        if needs_resize {
            let capacity = dict_table_capacity(new_entries);
            dict_rebuild(_py, order, hashes, table, capacity);
            if exception_pending(_py) {
                return false;
            }
        }
        reserve_dict_order(_py, order, 2 * added)
            && reserve_hashes(_py, hashes, added, "dict allocation failed")
    }
}

/// Append one new entry into reserved capacity, then publish its references,
/// tracking and structure. Cannot grow, fail or call Python.
#[inline]
unsafe fn dict_append_reserved_entry(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    key_bits: u64,
    val_bits: u64,
    hash: u64,
) {
    unsafe {
        let order = dict_order(ptr);
        let hashes = dict_hashes(ptr);
        order.push(key_bits);
        order.push(val_bits);
        hashes.push(hash);
        if crate::object::refcount_opt::is_heap_ref(key_bits) {
            inc_ref_bits(_py, key_bits);
        }
        if crate::object::refcount_opt::is_heap_ref(val_bits) {
            inc_ref_bits(_py, val_bits);
        }
        let entry_idx = order.len() / 2 - 1;
        dict_insert_entry_with_hash(_py, order, dict_table(ptr), entry_idx, hash);
        dict_commit_insertion(_py, ptr, key_bits, val_bits);
    }
}

/// Largest key set one string-binding transition publishes: a registry or
/// namespace binding names a key and at most a few spellings of it.
pub(crate) const DICT_STRING_BINDING_LIMIT: usize = 4;

/// Bind distinct exact `str` keys in one live dictionary as a single transition,
/// without staging a copy of the mapping.
///
/// Every fallible or Python-visible step precedes the first write. Hashing an
/// exact `str` cannot call Python. Probing may call `__eq__` of an existing
/// same-hash key of another kind before anything is written; a callback that
/// restructures the mapping restarts the probe against the live table, so its
/// effects order before this transition instead of being overwritten or
/// duplicated. Capacity reservation calls no Python and moves no entry. The
/// commit then neither allocates nor calls Python. A failure leaves every
/// binding, the insertion order and the structural epoch unchanged. A present
/// key keeps its key object, as item assignment does. Displaced values are
/// returned for release after the caller's dependent state has committed.
///
/// # Safety
/// The caller holds the GIL, keeps the live dictionary and every key and value
/// alive for the whole call, and holds no view of the dictionary backing.
pub(crate) unsafe fn dict_bind_string_entries<'a, 'py>(
    _py: &'a PyToken<'py>,
    dict: *mut u8,
    entries: &[(u64, u64)],
) -> Result<DetachedDictReferences<'a, 'py, [u64; DICT_STRING_BINDING_LIMIT]>, ()> {
    unsafe {
        crate::gil_assert();
        if exception_pending(_py) {
            return Err(());
        }
        if (*header_from_obj_ptr(dict)).has_flag(crate::object::HEADER_FLAG_FROZEN_LAYOUT_MAP) {
            raise_exception::<()>(_py, "TypeError", "class layout metadata is immutable");
            return Err(());
        }
        if entries.len() > DICT_STRING_BINDING_LIMIT {
            raise_exception::<()>(_py, "SystemError", "string binding exceeds its bound");
            return Err(());
        }
        let mut hashes = [0u64; DICT_STRING_BINDING_LIMIT];
        for (index, &(key_bits, _)) in entries.iter().enumerate() {
            let Some(key) = exact_string_bytes(_py, key_bits) else {
                raise_exception::<()>(_py, "SystemError", "string binding keys must be str");
                return Err(());
            };
            for &(earlier, _) in &entries[..index] {
                if exact_string_bytes(_py, earlier) == Some(key) {
                    raise_exception::<()>(_py, "SystemError", "string binding repeats a key");
                    return Err(());
                }
            }
            hashes[index] = hash_bits(_py, key_bits);
        }
        let mut found = [None; DICT_STRING_BINDING_LIMIT];
        loop {
            let epoch = dict_structural_epoch(dict);
            for (index, &(key_bits, _)) in entries.iter().enumerate() {
                found[index] = dict_find_entry_with_hash(_py, dict, key_bits, hashes[index]);
                if exception_pending(_py) {
                    return Err(());
                }
            }
            let added = found[..entries.len()]
                .iter()
                .filter(|entry| entry.is_none())
                .count();
            if added != 0 && !dict_reserve_entries(_py, dict, added) {
                return Err(());
            }
            // Only a probe callback can restructure the mapping; reservation
            // cannot. Every probe result is current while the epoch is.
            if dict_structural_epoch(dict) == epoch {
                break;
            }
        }
        let mut displaced = [0; DICT_STRING_BINDING_LIMIT];
        for (index, &(key_bits, value_bits)) in entries.iter().enumerate() {
            match found[index] {
                Some(entry) => {
                    displaced[index] = dict_replace_value(_py, dict, entry * 2 + 1, value_bits);
                }
                None => {
                    dict_append_reserved_entry(_py, dict, key_bits, value_bits, hashes[index]);
                }
            }
        }
        Ok(DetachedDictReferences {
            py: _py,
            bits: displaced,
        })
    }
}

/// Prehashed scalar entry points retain the same lookup/equality authority:
/// an integer probe may still encounter an equal bool, float or user key.
#[inline]
pub(crate) unsafe fn dict_set_inline_int_in_place(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    key_bits: u64,
    key_int: i64,
    val_bits: u64,
) {
    unsafe { dict_set_with_hash_in_place(_py, ptr, key_bits, val_bits, hash_int(key_int) as u64) }
}

#[inline]
pub(crate) unsafe fn dict_get_inline_int_in_place(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    key_bits: u64,
    key_int: i64,
) -> Option<u64> {
    unsafe { dict_get_with_hash_in_place(_py, ptr, key_bits, hash_int(key_int) as u64) }
}
pub(crate) unsafe fn set_add_in_place(py: &PyToken<'_>, ptr: *mut u8, key: u64, ctx: HashContext) {
    let _key = PinnedSetEntry::borrow(py, key, 0);
    if !ensure_hashable(py, key, ctx) {
        return;
    }
    let hash = hash_bits(py, key);
    if exception_pending(py) {
        return;
    }
    unsafe { set_add_with_hash_in_place(py, ptr, key, hash) };
}

/// Trusted hash from an existing set entry: no repeat __hash__ invocation.
pub(crate) unsafe fn set_add_with_hash_in_place(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    key_bits: u64,
    hash: u64,
) {
    let _key = PinnedSetEntry::borrow(_py, key_bits, hash);
    unsafe {
        crate::gil_assert();
        let found = set_find_entry_in_place_with_hash(_py, ptr, key_bits, hash);
        if exception_pending(_py) || found.is_some() {
            return;
        }
        set_insert_absent_with_hash(_py, ptr, key_bits, hash);
    }
}

/// The caller has proved absence or is copying distinct stored source keys.
/// This callback-free insertion is shared by normal insertion and exact copy.
unsafe fn set_insert_absent_with_hash(_py: &PyToken<'_>, ptr: *mut u8, key_bits: u64, hash: u64) {
    unsafe {
        // Lookup may mutate/reallocate storage; acquire backing only now.
        let order = set_order(ptr);
        let hashes = set_hashes(ptr);
        let table = set_table(ptr);
        let new_entries = order.len() + 1;
        let needs_resize = table.is_empty() || new_entries * 10 >= table.len() * 7;
        if needs_resize {
            let capacity = set_table_capacity(new_entries);
            set_rebuild(_py, order, hashes, table, capacity);
            if exception_pending(_py) {
                return;
            }
        }

        if !reserve_set_order(_py, order, 1)
            || !reserve_hashes(_py, hashes, 1, "set allocation failed")
        {
            return;
        }
        order.push(key_bits);
        hashes.push(hash);
        inc_ref_bits(_py, key_bits);
        let entry_idx = order.len() - 1;
        set_insert_entry_with_hash(_py, order, table, entry_idx, hash);
        if crate::object::refcount_opt::is_heap_ref(key_bits) {
            (*header_from_obj_ptr(ptr)).fetch_or_flags(crate::object::HEADER_FLAG_CONTAINS_REFS);
        }
    }
}

/// Copy preserves all stored entries even if a key's equality changed since
/// insertion. Re-inserting through rich lookup would incorrectly deduplicate
/// them and invoke user equality during set.copy().
pub(crate) unsafe fn set_copy_into_empty(py: &PyToken<'_>, source: *mut u8, target: *mut u8) {
    unsafe {
        assert!(set_order(target).is_empty());
        let mut index = 0;
        while let Some(entry) = set_pin_entry(py, source, index) {
            set_insert_absent_with_hash(py, target, entry.bits(), entry.hash());
            if exception_pending(py) {
                return;
            }
            index += 1;
        }
    }
}

/// Borrow a value using a trusted supplied hash, without hashability admission
/// or a hash callback. Candidate equality and mutation restart remain owned by
/// dict_find_entry_with_hash; no borrowed backing survives its callbacks.
pub(crate) unsafe fn dict_get_with_hash_in_place(
    py: &PyToken<'_>,
    dict: *mut u8,
    key_bits: u64,
    hash: u64,
) -> Option<u64> {
    unsafe {
        let index = dict_find_entry_with_hash(py, dict, key_bits, hash)?;
        Some(dict_order(dict)[index * 2 + 1])
    }
}

pub(crate) unsafe fn dict_get_in_place(
    py: &PyToken<'_>,
    dict: *mut u8,
    key_bits: u64,
) -> Option<u64> {
    unsafe {
        if let Some(integer) = obj_from_bits(key_bits).as_int() {
            return dict_get_inline_int_in_place(py, dict, key_bits, integer);
        }
        let index = dict_find_entry(py, dict, key_bits)?;
        Some(dict_order(dict)[index * 2 + 1])
    }
}

/// Physical metadata lookup: no allocation or Python callbacks. This
/// matches string storage under the builtin byte hash (including stored
/// subclasses) and skips other key kinds. It is not ordinary dictionary
/// equality: a user lookup must use dict_find_entry, or preserve Undecided
/// from dict_exact_string_lookup rather than treating it as absence.
pub(crate) unsafe fn dict_get_str_bytes_borrowed(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    key: &[u8],
) -> Option<u64> {
    unsafe {
        if object_type_id(ptr) != TYPE_ID_DICT {
            return None;
        }
        let table = dict_table(ptr);
        if table.is_empty() {
            return None;
        }
        let hash = hash_string_bytes(_py, key) as u64;
        let order = dict_order(ptr);
        let hashes = dict_hashes(ptr);
        let mask = table.len() - 1;
        let mut slot = (hash as usize) & mask;
        loop {
            let entry = table[slot];
            if entry == 0 {
                return None;
            }
            if entry == TABLE_TOMBSTONE {
                slot = (slot + 1) & mask;
                continue;
            }
            let entry_idx = entry - 1;
            if entry_idx * 2 >= order.len() {
                slot = (slot + 1) & mask;
                continue;
            }
            if hashes.get(entry_idx).copied() != Some(hash) {
                slot = (slot + 1) & mask;
                continue;
            }
            let entry_key_bits = order[entry_idx * 2];
            let Some(entry_key_ptr) = obj_from_bits(entry_key_bits).as_ptr() else {
                slot = (slot + 1) & mask;
                continue;
            };
            if object_type_id(entry_key_ptr) == TYPE_ID_STRING {
                let len = string_len(entry_key_ptr);
                if len == key.len() && simd_bytes_eq(string_bytes(entry_key_ptr), key.as_ptr(), len)
                {
                    return Some(order[entry_idx * 2 + 1]);
                }
            }
            slot = (slot + 1) & mask;
        }
    }
}

pub(crate) unsafe fn dict_find_entry_kv_in_place(
    py: &PyToken<'_>,
    dict: *mut u8,
    key_bits: u64,
) -> Option<(u64, u64)> {
    unsafe {
        let index = dict_find_entry(py, dict, key_bits)?;
        let order = dict_order(dict);
        let key_index = index * 2;
        Some((order[key_index], order[key_index + 1]))
    }
}

pub(crate) unsafe fn set_del_in_place(_py: &PyToken<'_>, ptr: *mut u8, key_bits: u64) -> bool {
    let _key = PinnedSetEntry::borrow(_py, key_bits, 0);
    if !ensure_hashable(_py, key_bits, HashContext::SetElement) {
        return false;
    }
    let hash = hash_bits(_py, key_bits);
    if exception_pending(_py) {
        return false;
    }
    unsafe { set_del_with_hash_in_place(_py, ptr, key_bits, hash) }
}

pub(crate) unsafe fn set_del_with_hash_in_place(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    key_bits: u64,
    hash: u64,
) -> bool {
    let _key = PinnedSetEntry::borrow(_py, key_bits, hash);
    unsafe {
        let found = set_find_entry_in_place_with_hash(_py, ptr, key_bits, hash);
        if exception_pending(_py) {
            return false;
        }
        let Some(entry_idx) = found else {
            return false;
        };
        let order = set_order(ptr);
        let hashes = set_hashes(ptr);
        let table = set_table(ptr);
        let key_val = order[entry_idx];
        order.remove(entry_idx);
        hashes.remove(entry_idx);
        let removed_slot_val = entry_idx + 1;
        let mut tombstones = 0usize;
        for slot in table.iter_mut() {
            if *slot == 0 {
                continue;
            }
            if *slot == TABLE_TOMBSTONE {
                tombstones = tombstones.saturating_add(1);
                continue;
            }
            if *slot == removed_slot_val {
                *slot = TABLE_TOMBSTONE;
                tombstones = tombstones.saturating_add(1);
                continue;
            }
            if *slot > removed_slot_val {
                *slot -= 1;
            }
        }
        let entries = order.len();
        let desired_capacity = set_table_capacity(entries.max(1));
        if table.len() > desired_capacity.saturating_mul(4)
            || tombstones.saturating_mul(4) > table.len()
        {
            set_rebuild(_py, order, hashes, table, desired_capacity);
        }
        if order.is_empty() {
            (*header_from_obj_ptr(ptr)).fetch_and_flags(!crate::object::HEADER_FLAG_CONTAINS_REFS);
        }
        dec_ref_bits(_py, key_val);
        true
    }
}

/// Publish a completely owned replacement before releasing old references.
/// The destination and staging set keep their backing-cell identities.
pub(crate) unsafe fn set_publish_staged(_py: &PyToken<'_>, live: *mut u8, staged: *mut u8) {
    unsafe {
        crate::gil_assert();
        assert_ne!(live, staged);
        assert_eq!(object_type_id(live), TYPE_ID_SET);
        assert_eq!(object_type_id(staged), TYPE_ID_SET);
        let order = set_order_ptr(live);
        let hashes = set_hashes_ptr(live);
        let table = set_table_ptr(live);
        let _order_lock = crate::object::backing::tracked_vec_mutation_lock(order);
        let _hashes_lock = crate::object::backing::tracked_vec_mutation_lock(hashes);
        let _table_lock = crate::object::backing::tracked_vec_mutation_lock(table);
        crate::object::backing::tracked_vec_swap_contents(order, set_order(staged));
        crate::object::backing::tracked_vec_swap_contents(hashes, set_hashes(staged));
        crate::object::backing::tracked_vec_swap_contents(table, set_table(staged));
        let live_refs =
            (*header_from_obj_ptr(live)).has_flag(crate::object::HEADER_FLAG_CONTAINS_REFS);
        let staged_refs =
            (*header_from_obj_ptr(staged)).has_flag(crate::object::HEADER_FLAG_CONTAINS_REFS);
        for (ptr, refs) in [(live, staged_refs), (staged, live_refs)] {
            if refs {
                (*header_from_obj_ptr(ptr))
                    .fetch_or_flags(crate::object::HEADER_FLAG_CONTAINS_REFS);
            } else {
                (*header_from_obj_ptr(ptr))
                    .fetch_and_flags(!crate::object::HEADER_FLAG_CONTAINS_REFS);
            }
        }
    }
}

pub(crate) unsafe fn set_clear_in_place(py: &PyToken<'_>, set: *mut u8) {
    let replacement = molt_set_new(0);
    let Some(ptr) = obj_from_bits(replacement).as_ptr() else {
        return;
    };
    unsafe { set_publish_staged(py, set, ptr) };
    dec_ref_bits(py, replacement);
}

pub(crate) unsafe fn dict_del_in_place(_py: &PyToken<'_>, ptr: *mut u8, key_bits: u64) -> bool {
    unsafe { dict_del_deferred(_py, ptr, key_bits) }.is_some()
}

pub(crate) unsafe fn dict_del_deferred<'a, 'py>(
    _py: &'a PyToken<'py>,
    ptr: *mut u8,
    key_bits: u64,
) -> Option<DetachedDictReferences<'a, 'py>> {
    unsafe {
        if (*header_from_obj_ptr(ptr)).has_flag(crate::object::HEADER_FLAG_FROZEN_LAYOUT_MAP) {
            raise_exception::<()>(_py, "TypeError", "class layout metadata is immutable");
            return None;
        }
        let found = dict_find_entry(_py, ptr, key_bits);
        let order = dict_order(ptr);
        let hashes = dict_hashes(ptr);
        let table = dict_table(ptr);
        if exception_pending(_py) {
            return None;
        }
        let entry_idx = found?;
        let key_idx = entry_idx * 2;
        let val_idx = key_idx + 1;
        let removed = [order[key_idx], order[val_idx]];
        order.drain(key_idx..=val_idx);
        hashes.remove(entry_idx);
        let removed_slot_val = entry_idx + 1;
        let mut tombstones = 0usize;
        for slot in table.iter_mut() {
            if *slot == 0 {
                continue;
            }
            if *slot == TABLE_TOMBSTONE {
                tombstones = tombstones.saturating_add(1);
                continue;
            }
            if *slot == removed_slot_val {
                *slot = TABLE_TOMBSTONE;
                tombstones = tombstones.saturating_add(1);
                continue;
            }
            if *slot > removed_slot_val {
                *slot -= 1;
            }
        }
        let entries = order.len() / 2;
        let desired_capacity = dict_table_capacity(entries.max(1));
        if table.len() > desired_capacity.saturating_mul(4)
            || tombstones.saturating_mul(4) > table.len()
        {
            dict_rebuild(_py, order, hashes, table, desired_capacity);
        }
        if order.is_empty() {
            (*header_from_obj_ptr(ptr)).fetch_and_flags(!crate::object::HEADER_FLAG_CONTAINS_REFS);
        }
        dict_commit_structure(ptr);
        Some(DetachedDictReferences {
            py: _py,
            bits: removed,
        })
    }
}

/// Publish an empty dictionary before returning its displaced owned contents.
/// No Python edge is released until the caller drops the returned transaction.
pub(crate) unsafe fn dict_clear_deferred<'a, 'py>(
    _py: &'a PyToken<'py>,
    ptr: *mut u8,
) -> Option<DetachedDictReferences<'a, 'py, Vec<u64>>> {
    unsafe {
        if (*header_from_obj_ptr(ptr)).has_flag(crate::object::HEADER_FLAG_FROZEN_LAYOUT_MAP) {
            raise_exception::<()>(_py, "TypeError", "class layout metadata is immutable");
            return None;
        }
        Some(DetachedDictReferences {
            py: _py,
            bits: dict_detach_contents(_py, ptr),
        })
    }
}

unsafe fn dict_detach_contents(_py: &PyToken<'_>, ptr: *mut u8) -> Vec<u64> {
    unsafe {
        crate::gil_assert();
        let order = dict_order(ptr);
        let removed: Vec<u64> = std::mem::take(order);
        let hashes = dict_hashes(ptr);
        hashes.clear();
        let table = dict_table(ptr);
        table.clear();
        (*header_from_obj_ptr(ptr)).fetch_and_flags(!crate::object::HEADER_FLAG_CONTAINS_REFS);
        dict_commit_structure(ptr);
        removed
    }
}

pub(crate) unsafe fn dict_clear_in_place(_py: &PyToken<'_>, ptr: *mut u8) {
    drop(unsafe { dict_clear_deferred(_py, ptr) });
}

pub(crate) unsafe fn dict_clear_in_place_shutdown(_py: &PyToken<'_>, ptr: *mut u8) {
    // Teardown bypasses mutation admission, not reference ownership: dictionary
    // edges may not revoke a referent's canonical immortal lifetime.
    drop(DetachedDictReferences {
        py: _py,
        bits: unsafe { dict_detach_contents(_py, ptr) },
    });
}
