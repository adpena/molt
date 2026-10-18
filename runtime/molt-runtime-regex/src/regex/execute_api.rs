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
        let Some(mode) = string_obj_to_owned(obj_from_bits(mode_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "mode must be str");
        };
        let pos = pos.max(0) as usize;
        let end = end.max(0) as usize;
        let subject_obj = obj_from_bits(text_bits);
        let matched = if compiled.compiled.is_bytes {
            let Some(bytes) = (unsafe { bytes_obj_to_borrowed(subject_obj) }) else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "cannot use a bytes pattern on a string-like object",
                );
            };
            execute_match_bytes(&compiled.compiled, bytes, pos, end, &mode)
        } else {
            let Some(text) = string_obj_to_owned(subject_obj) else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "cannot use a string pattern on a bytes-like object",
                );
            };
            execute_match(&compiled.compiled, &text, pos, end, &mode)
        };
        match matched {
            Some(result) => build_match_result_bits(_py, &result, compiled.compiled.group_count),
            None => MoltObject::none().bits(),
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
        let pos = pos.max(0) as usize;
        let end = end.max(0) as usize;
        let subject = obj_from_bits(subject_bits);
        let (text_chars, subject_len) = if pattern.compiled.is_bytes {
            let Some(bytes) = (unsafe { bytes_obj_to_borrowed(subject) }) else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "cannot use a bytes pattern on a string-like object",
                );
            };
            if pos > bytes.len() {
                return raise_exception::<_>(_py, "ValueError", "pos out of range");
            }
            (None, bytes.len())
        } else {
            let Some(text) = string_obj_to_owned(subject) else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "cannot use a string pattern on a bytes-like object",
                );
            };
            let chars: Vec<char> = text.chars().collect();
            if pos > chars.len() {
                return raise_exception::<_>(_py, "ValueError", "pos out of range");
            }
            let len = chars.len();
            (Some(chars), len)
        };
        let end = end.min(subject_len);
        let scanner = ScannerHandle::new(pattern, pos.min(end), end, text_chars);
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
            advance_scanner(
                &pattern.compiled,
                &mut scanner,
                SubjectStorage::Bytes(bytes),
            )
        } else {
            let Some(chars) = scanner.text_chars.take() else {
                drop(scanner);
                return raise_exception::<_>(
                    _py,
                    "SystemError",
                    "text scanner has no prepared subject",
                );
            };
            let result = advance_scanner(
                &pattern.compiled,
                &mut scanner,
                SubjectStorage::Text(&chars),
            );
            scanner.text_chars = Some(chars);
            result
        };
        drop(scanner);
        match next {
            Some(result) => build_match_result_bits(_py, &result, group_count),
            None => MoltObject::none().bits(),
        }
    })
}

pub(super) fn advance_scanner(
    compiled: &CompiledPattern,
    scanner: &mut ScannerHandle,
    subject: SubjectStorage<'_>,
) -> Option<MatchResult> {
    while scanner.pos <= scanner.end {
        let mode = if scanner.must_advance {
            "search_nonempty"
        } else {
            "search"
        };
        let result = execute_match_subject(compiled, subject, scanner.pos, scanner.end, mode);
        let Some(result) = result else {
            if scanner.must_advance && scanner.pos < scanner.end {
                scanner.pos += 1;
                scanner.must_advance = false;
                continue;
            }
            scanner.finished = true;
            return None;
        };
        scanner.must_advance = result.start == result.end;
        scanner.pos = if result.start == result.end {
            result.start
        } else {
            result.end
        };
        return Some(result);
    }
    scanner.finished = true;
    None
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
    let result_ptr = alloc_tuple(_py, &[start_bits, end_bits, groups_bits]);
    dec_ref_bits(_py, groups_bits);
    if result_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(result_ptr).bits()
    }
}
