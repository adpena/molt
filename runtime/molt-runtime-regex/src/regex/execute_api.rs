use super::*;
use std::sync::{Arc, Mutex};

/// `molt_re_execute(handle, text, pos, end, mode) -> match_result | None`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_execute(
    handle_bits: u64,
    text_bits: u64,
    pos_bits: u64,
    end_bits: u64,
    mode_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(compiled) = pattern_handle_arc(handle_bits) else {
            return raise_exception::<_>(_py, "ValueError", "invalid regex handle");
        };
        let Some(pos) = to_i64(obj_from_bits(pos_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pos must be int");
        };
        let Some(end) = to_i64(obj_from_bits(end_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "end must be int");
        };
        // The mode protocol is strict UTF-8; arbitrary regex text is not.
        // SAFETY: the argument owns this immutable string under the core GIL.
        let mode_bytes = match unsafe { string_storage(_py, mode_bits, "mode must be str") } {
            Ok(bytes) => bytes,
            Err(bits) => return bits,
        };
        let mode = match std::str::from_utf8(mode_bytes)
            .map_err(|_| MatchError::InvalidMode)
            .and_then(MatchMode::try_from)
        {
            Ok(mode) => mode,
            Err(error) => return match_error_result(_py, error),
        };
        let pos = usize::try_from(pos.max(0)).unwrap_or(usize::MAX);
        let end = usize::try_from(end.max(0)).unwrap_or(usize::MAX);
        let subject_obj = obj_from_bits(text_bits);
        let matched = if compiled.compiled.is_bytes {
            let Some(bytes) = (unsafe { bytes_obj_to_borrowed(subject_obj) }) else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "cannot use a bytes pattern on a string-like object",
                );
            };
            execute_match_bytes(&compiled.compiled, bytes, pos, end, mode)
        } else {
            // SAFETY: the retained argument owns immutable surrogatepass storage.
            let text = match unsafe {
                string_storage(
                    _py,
                    subject_obj.bits(),
                    "cannot use a string pattern on a bytes-like object",
                )
            } {
                Ok(bytes) => bytes,
                Err(bits) => return bits,
            };
            execute_match(&compiled.compiled, text, pos, end, mode)
        };
        match matched {
            Ok(Some(result)) => {
                build_match_result_bits(_py, &result, compiled.compiled.group_count)
            }
            Ok(None) => MoltObject::none().bits(),
            Err(error) => match_error_result(_py, error),
        }
    })
}

/// Create a GREEN scanner state for one Pattern.finditer call. The Python
/// iterator owns the original subject and the memoryview lease for byte input.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_scanner_new(
    handle_bits: u64,
    subject_bits: u64,
    pos_bits: u64,
    end_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(pattern) = pattern_handle_arc(handle_bits) else {
            return raise_exception::<_>(_py, "ValueError", "invalid regex handle");
        };
        let Some(pos) = to_i64(obj_from_bits(pos_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pos must be int");
        };
        let Some(end) = to_i64(obj_from_bits(end_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "end must be int");
        };
        let pos = usize::try_from(pos.max(0)).unwrap_or(usize::MAX);
        let end = usize::try_from(end.max(0)).unwrap_or(usize::MAX);
        let subject = obj_from_bits(subject_bits);
        let (text_units, subject_len) = if pattern.compiled.is_bytes {
            let Some(bytes) = (unsafe { bytes_obj_to_borrowed(subject) }) else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "cannot use a bytes pattern on a string-like object",
                );
            };
            (None, bytes.len())
        } else {
            let units = match string_codepoints(
                _py,
                subject_bits,
                "cannot use a string pattern on a bytes-like object",
            ) {
                Ok(units) => units,
                Err(bits) => return bits,
            };
            let len = units.len();
            (Some(units), len)
        };
        let end = end.min(subject_len);
        let scanner = ScannerHandle::new(pattern, pos.min(subject_len), end, text_units);
        match scanner_handle_new(Arc::new(Mutex::new(scanner))) {
            Some(bits) => bits,
            None => MoltObject::none().bits(),
        }
    })
}

/// Advance the scanner once and return one ordinary match-result tuple, or
/// None on exhaustion. The live memoryview is borrowed only during matching.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_finditer_next(scanner_bits: u64, subject_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(scanner) = scanner_handle_arc(scanner_bits) else {
            return raise_exception::<_>(_py, "ValueError", "invalid regex scanner");
        };
        let mut scanner = match scanner.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        if scanner.finished {
            return MoltObject::none().bits();
        }
        let pattern = Arc::clone(&scanner.pattern);
        let group_count = pattern.compiled.group_count;
        let subject_obj = obj_from_bits(subject_bits);
        let next = if pattern.compiled.is_bytes {
            let Some(bytes) = (unsafe { bytes_obj_to_borrowed(subject_obj) }) else {
                drop(scanner);
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "expected a contiguous bytes-like memoryview",
                );
            };
            advance_scanner(&mut scanner, SubjectStorage::Bytes(bytes))
        } else {
            let Some(units) = scanner.text_units.take() else {
                drop(scanner);
                return raise_exception::<_>(
                    _py,
                    "SystemError",
                    "text scanner has no prepared subject",
                );
            };
            let result = advance_scanner(&mut scanner, SubjectStorage::Text(&units));
            scanner.text_units = Some(units);
            result
        };
        drop(scanner);
        match next {
            Ok(Some(result)) => build_match_result_bits(_py, &result, group_count),
            Ok(None) => MoltObject::none().bits(),
            Err(error) => match_error_result(_py, error),
        }
    })
}

pub(super) fn advance_scanner(
    scanner: &mut ScannerHandle,
    subject: SubjectStorage<'_>,
) -> Result<Option<MatchResult>, MatchError> {
    if scanner.finished || scanner.pos > scanner.end {
        scanner.finished = true;
        return Ok(None);
    }
    let mode = if scanner.must_advance {
        MatchMode::SearchNonempty
    } else {
        MatchMode::Search
    };
    // Search already retries every eligible start, including a consuming
    // alternative at the previous empty match. No post-failure rescan is needed.
    // Keep cursor state unchanged until execution returns a successful outcome.
    let result = execute_match_subject(
        &scanner.pattern.compiled,
        subject,
        scanner.pos,
        scanner.end,
        mode,
    )?;
    if let Some(result) = &result {
        scanner.must_advance = result.start == result.end;
        scanner.pos = result.end;
    } else {
        scanner.finished = true;
    }
    Ok(result)
}

pub(super) fn build_match_result_bits(
    _py: &CoreGilToken,
    result: &MatchResult,
    group_count: u32,
) -> u64 {
    let start_bits = MoltObject::from_int(result.start as i64).bits();
    let end_bits = MoltObject::from_int(result.end as i64).bits();
    let mut group_elems: Vec<u64> = Vec::with_capacity(group_count as usize);
    let mut owned_pairs = Vec::with_capacity(group_count as usize);
    for i in 1..=(group_count as usize) {
        if i < result.groups.len() {
            match result.groups[i] {
                Some((gs, ge)) => {
                    let pair_ptr = alloc_tuple(
                        _py,
                        &[
                            MoltObject::from_int(gs as i64).bits(),
                            MoltObject::from_int(ge as i64).bits(),
                        ],
                    );
                    if pair_ptr.is_null() {
                        for bits in owned_pairs {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    }
                    let bits = MoltObject::from_ptr(pair_ptr).bits();
                    group_elems.push(bits);
                    owned_pairs.push(bits);
                }
                None => group_elems.push(MoltObject::none().bits()),
            }
        } else {
            group_elems.push(MoltObject::none().bits());
        }
    }
    let groups_ptr = alloc_tuple(_py, &group_elems);
    for bits in owned_pairs {
        dec_ref_bits(_py, bits);
    }
    if groups_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let groups_bits = MoltObject::from_ptr(groups_ptr).bits();
    let lastindex_bits = result.lastindex.map_or_else(
        || MoltObject::none().bits(),
        |index| MoltObject::from_int(i64::from(index)).bits(),
    );
    let result_ptr = alloc_tuple(_py, &[start_bits, end_bits, groups_bits, lastindex_bits]);
    dec_ref_bits(_py, groups_bits);
    if result_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(result_ptr).bits()
    }
}

pub(super) fn match_error_result(py: &CoreGilToken, error: MatchError) -> u64 {
    match error {
        MatchError::Memory => {
            raise_exception::<_>(py, "MemoryError", "regex workspace allocation failed")
        }
        MatchError::InvalidMode => {
            raise_exception::<_>(py, "ValueError", "invalid regex match mode")
        }
        MatchError::Internal => {
            raise_exception::<_>(py, "SystemError", "invalid regex capture marks")
        }
    }
}
