use super::*;

pub(super) fn importlib_reader_collect_unique_strings(
    _py: &PyToken<'_>,
    values_bits: u64,
    _invalid_entry_message: &str,
) -> Result<Vec<String>, u64> {
    let iter_bits = molt_iter(values_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    loop {
        let pair_bits = molt_iter_next(iter_bits);
        let Some(pair_ptr) = maybe_ptr_from_bits(pair_bits) else {
            return Err(MoltObject::none().bits());
        };
        unsafe {
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                return Err(MoltObject::none().bits());
            }
        }
        let pair = unsafe { seq_vec_ref(pair_ptr) };
        if pair.len() < 2 {
            return Err(MoltObject::none().bits());
        }
        if is_truthy(_py, obj_from_bits(pair[1])) {
            break;
        }
        let Some(entry) = string_obj_to_owned(obj_from_bits(pair[0])) else {
            // CPython ResourceReader handling is tolerant of non-string entries in
            // iterator-style views; skip malformed entries instead of aborting.
            continue;
        };
        if entry.is_empty() {
            continue;
        }
        if seen.insert(entry.clone()) {
            out.push(entry);
        }
    }
    Ok(out)
}
pub(super) fn importlib_reader_collect_unique_paths(
    _py: &PyToken<'_>,
    values_bits: u64,
    invalid_entry_message: &str,
) -> Result<Vec<String>, u64> {
    let iter_bits = molt_iter(values_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    loop {
        let pair_bits = molt_iter_next(iter_bits);
        let Some(pair_ptr) = maybe_ptr_from_bits(pair_bits) else {
            return Err(MoltObject::none().bits());
        };
        unsafe {
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                return Err(MoltObject::none().bits());
            }
        }
        let pair = unsafe { seq_vec_ref(pair_ptr) };
        if pair.len() < 2 {
            return Err(MoltObject::none().bits());
        }
        if is_truthy(_py, obj_from_bits(pair[1])) {
            break;
        }
        let path = match path_from_bits(_py, pair[0]) {
            Ok(path_buf) => path_buf.to_string_lossy().into_owned(),
            Err(_) => {
                if exception_pending(_py) {
                    clear_exception(_py);
                }
                return Err(raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    invalid_entry_message,
                ));
            }
        };
        if path.is_empty() {
            continue;
        }
        if seen.insert(path.clone()) {
            out.push(path);
        }
    }
    Ok(out)
}
pub(super) fn importlib_reader_collect_bytes(
    _py: &PyToken<'_>,
    value_bits: u64,
) -> Option<Vec<u8>> {
    let ptr = obj_from_bits(value_bits).as_ptr()?;
    unsafe { bytes_like_slice(ptr).map(|slice| slice.to_vec()) }
}
pub(super) fn importlib_reader_files_traversable_bits(
    _py: &PyToken<'_>,
    reader_bits: u64,
) -> Result<Option<u64>, u64> {
    let files_name = intern_runtime_static_name(_py, b"files");
    let Some(call_bits) = importlib_reader_lookup_callable(_py, reader_bits, files_name)? else {
        return Ok(None);
    };
    let value_bits = unsafe { call_callable0(_py, call_bits) };
    dec_ref_bits(_py, call_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if obj_from_bits(value_bits).is_none() {
        return Ok(None);
    }
    Ok(Some(value_bits))
}
pub(super) fn importlib_traversable_joinpath_bits(
    _py: &PyToken<'_>,
    traversable_bits: u64,
    name: &str,
) -> Result<Option<u64>, u64> {
    let joinpath_name = intern_runtime_static_name(_py, b"joinpath");
    let Some(call_bits) = importlib_reader_lookup_callable(_py, traversable_bits, joinpath_name)?
    else {
        return Ok(None);
    };
    let name_bits = alloc_str_bits(_py, name)?;
    let value_bits = unsafe { call_callable1(_py, call_bits, name_bits) };
    dec_ref_bits(_py, name_bits);
    dec_ref_bits(_py, call_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if obj_from_bits(value_bits).is_none() {
        return Ok(None);
    }
    Ok(Some(value_bits))
}
pub(super) fn importlib_traversable_bits_for_parts(
    _py: &PyToken<'_>,
    reader_bits: u64,
    parts: &[String],
) -> Result<Option<u64>, u64> {
    let Some(mut current_bits) = importlib_reader_files_traversable_bits(_py, reader_bits)? else {
        return Ok(None);
    };
    for part in parts {
        if part.is_empty() {
            continue;
        }
        let next_bits = match importlib_traversable_joinpath_bits(_py, current_bits, part)? {
            Some(bits) => bits,
            None => {
                if !obj_from_bits(current_bits).is_none() {
                    dec_ref_bits(_py, current_bits);
                }
                return Ok(None);
            }
        };
        if !obj_from_bits(current_bits).is_none() {
            dec_ref_bits(_py, current_bits);
        }
        current_bits = next_bits;
    }
    Ok(Some(current_bits))
}
pub(super) fn importlib_traversable_iterdir_names(
    _py: &PyToken<'_>,
    traversable_bits: u64,
) -> Result<Vec<String>, u64> {
    let iterdir_name = intern_runtime_static_name(_py, b"iterdir");
    let Some(call_bits) = importlib_reader_lookup_callable(_py, traversable_bits, iterdir_name)?
    else {
        return Ok(Vec::new());
    };
    let iterable_bits = unsafe { call_callable0(_py, call_bits) };
    dec_ref_bits(_py, call_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let iter_bits = molt_iter(iterable_bits);
    if !obj_from_bits(iterable_bits).is_none() {
        dec_ref_bits(_py, iterable_bits);
    }
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let name_attr = intern_runtime_static_name(_py, b"name");
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    loop {
        let pair_bits = molt_iter_next(iter_bits);
        let Some(pair_ptr) = maybe_ptr_from_bits(pair_bits) else {
            return Err(MoltObject::none().bits());
        };
        unsafe {
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                return Err(MoltObject::none().bits());
            }
        }
        let pair = unsafe { seq_vec_ref(pair_ptr) };
        if pair.len() < 2 {
            return Err(MoltObject::none().bits());
        }
        if is_truthy(_py, obj_from_bits(pair[1])) {
            break;
        }
        let entry_bits = pair[0];
        let missing = missing_bits(_py);
        let name_bits = molt_getattr_builtin(entry_bits, name_attr, missing);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        if is_missing_bits(_py, name_bits) {
            if !obj_from_bits(name_bits).is_none() {
                dec_ref_bits(_py, name_bits);
            }
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "invalid loader resource traversable payload: missing name",
            ));
        }
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            if !obj_from_bits(name_bits).is_none() {
                dec_ref_bits(_py, name_bits);
            }
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "invalid loader resource traversable payload: name must be str",
            ));
        };
        if !obj_from_bits(name_bits).is_none() {
            dec_ref_bits(_py, name_bits);
        }
        if name.is_empty() {
            continue;
        }
        if seen.insert(name.clone()) {
            out.push(name);
        }
    }
    Ok(out)
}
pub(super) fn importlib_traversable_is_file(
    _py: &PyToken<'_>,
    traversable_bits: u64,
) -> Result<bool, u64> {
    let is_file_name = intern_runtime_static_name(_py, b"is_file");
    let Some(call_bits) = importlib_reader_lookup_callable(_py, traversable_bits, is_file_name)?
    else {
        return Ok(false);
    };
    let value_bits = unsafe { call_callable0(_py, call_bits) };
    dec_ref_bits(_py, call_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let true_bits = MoltObject::from_bool(true).bits();
    let false_bits = MoltObject::from_bool(false).bits();
    if value_bits == true_bits {
        if !obj_from_bits(value_bits).is_none() {
            dec_ref_bits(_py, value_bits);
        }
        return Ok(true);
    }
    if value_bits == false_bits {
        if !obj_from_bits(value_bits).is_none() {
            dec_ref_bits(_py, value_bits);
        }
        return Ok(false);
    }
    if !obj_from_bits(value_bits).is_none() {
        dec_ref_bits(_py, value_bits);
    }
    Err(raise_exception::<_>(
        _py,
        "RuntimeError",
        "invalid loader resource traversable payload: is_file must be bool",
    ))
}
pub(super) fn importlib_traversable_is_dir(
    _py: &PyToken<'_>,
    traversable_bits: u64,
) -> Result<bool, u64> {
    let is_dir_name = intern_runtime_static_name(_py, b"is_dir");
    if let Some(call_bits) = importlib_reader_lookup_callable(_py, traversable_bits, is_dir_name)? {
        let value_bits = unsafe { call_callable0(_py, call_bits) };
        dec_ref_bits(_py, call_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        let true_bits = MoltObject::from_bool(true).bits();
        let false_bits = MoltObject::from_bool(false).bits();
        if value_bits == true_bits {
            if !obj_from_bits(value_bits).is_none() {
                dec_ref_bits(_py, value_bits);
            }
            return Ok(true);
        }
        if value_bits == false_bits {
            if !obj_from_bits(value_bits).is_none() {
                dec_ref_bits(_py, value_bits);
            }
            return Ok(false);
        }
        if !obj_from_bits(value_bits).is_none() {
            dec_ref_bits(_py, value_bits);
        }
        return Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            "invalid loader resource traversable payload: is_dir must be bool",
        ));
    }

    match path_from_bits(_py, traversable_bits) {
        Ok(path) => Ok(std::fs::metadata(path)
            .map(|meta| meta.is_dir())
            .unwrap_or(false)),
        Err(_) => {
            if exception_pending(_py) {
                clear_exception(_py);
            }
            Ok(false)
        }
    }
}
pub(super) fn importlib_traversable_exists(
    _py: &PyToken<'_>,
    traversable_bits: u64,
) -> Result<bool, u64> {
    let exists_name = intern_runtime_static_name(_py, b"exists");
    if let Some(call_bits) = importlib_reader_lookup_callable(_py, traversable_bits, exists_name)? {
        let value_bits = unsafe { call_callable0(_py, call_bits) };
        dec_ref_bits(_py, call_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        let true_bits = MoltObject::from_bool(true).bits();
        let false_bits = MoltObject::from_bool(false).bits();
        if value_bits == true_bits {
            if !obj_from_bits(value_bits).is_none() {
                dec_ref_bits(_py, value_bits);
            }
            return Ok(true);
        }
        if value_bits == false_bits {
            if !obj_from_bits(value_bits).is_none() {
                dec_ref_bits(_py, value_bits);
            }
            return Ok(false);
        }
        if !obj_from_bits(value_bits).is_none() {
            dec_ref_bits(_py, value_bits);
        }
        return Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            "invalid loader resource traversable payload: exists must be bool",
        ));
    }

    match path_from_bits(_py, traversable_bits) {
        Ok(path) => Ok(std::fs::metadata(path).is_ok()),
        Err(_) => {
            if exception_pending(_py) {
                clear_exception(_py);
            }
            let is_file = importlib_traversable_is_file(_py, traversable_bits)?;
            if is_file {
                return Ok(true);
            }
            importlib_traversable_is_dir(_py, traversable_bits)
        }
    }
}
pub(super) fn importlib_traversable_open_bytes(
    _py: &PyToken<'_>,
    traversable_bits: u64,
) -> Result<Vec<u8>, u64> {
    let open_name = intern_runtime_static_name(_py, b"open");
    let Some(call_bits) = importlib_reader_lookup_callable(_py, traversable_bits, open_name)?
    else {
        return Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            "invalid loader resource traversable payload: missing open()",
        ));
    };
    let mode_bits = alloc_str_bits(_py, "rb")?;
    let handle_bits = unsafe { call_callable1(_py, call_bits, mode_bits) };
    dec_ref_bits(_py, mode_bits);
    dec_ref_bits(_py, call_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if let Some(bytes) = importlib_reader_collect_bytes(_py, handle_bits) {
        if !obj_from_bits(handle_bits).is_none() {
            dec_ref_bits(_py, handle_bits);
        }
        return Ok(bytes);
    }
    let read_name = intern_runtime_static_name(_py, b"read");
    let read_bits = match importlib_reader_lookup_callable(_py, handle_bits, read_name)? {
        Some(bits) => bits,
        None => {
            if !obj_from_bits(handle_bits).is_none() {
                dec_ref_bits(_py, handle_bits);
            }
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "invalid loader open_resource payload",
            ));
        }
    };
    let payload_bits = unsafe { call_callable0(_py, read_bits) };
    dec_ref_bits(_py, read_bits);
    if exception_pending(_py) {
        if !obj_from_bits(handle_bits).is_none() {
            dec_ref_bits(_py, handle_bits);
        }
        return Err(MoltObject::none().bits());
    }
    let close_name = intern_runtime_static_name(_py, b"close");
    if let Some(close_bits) = importlib_reader_lookup_callable(_py, handle_bits, close_name)? {
        let _ = unsafe { call_callable0(_py, close_bits) };
        dec_ref_bits(_py, close_bits);
        if exception_pending(_py) {
            if !obj_from_bits(payload_bits).is_none() {
                dec_ref_bits(_py, payload_bits);
            }
            if !obj_from_bits(handle_bits).is_none() {
                dec_ref_bits(_py, handle_bits);
            }
            return Err(MoltObject::none().bits());
        }
    }
    if !obj_from_bits(handle_bits).is_none() {
        dec_ref_bits(_py, handle_bits);
    }
    let Some(bytes) = importlib_reader_collect_bytes(_py, payload_bits) else {
        if !obj_from_bits(payload_bits).is_none() {
            dec_ref_bits(_py, payload_bits);
        }
        return Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            "invalid loader open_resource payload",
        ));
    };
    if !obj_from_bits(payload_bits).is_none() {
        dec_ref_bits(_py, payload_bits);
    }
    Ok(bytes)
}
