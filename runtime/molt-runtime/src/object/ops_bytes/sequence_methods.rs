use super::*;
use crate::runtime_flags::{self, RuntimeFlag};

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_join(sep_bits: u64, items_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { bytes_join(py, sep_bits, items_bits, false) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_join(sep_bits: u64, items_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { bytes_join(py, sep_bits, items_bits, true) })
}

fn bytes_join(py: &PyToken<'_>, sep_bits: u64, items_bits: u64, mutable: bool) -> u64 {
    use crate::object::buffer_exports::ScopedBufferExport;
    use molt_cpython_abi::abi_types::PyBUF_SIMPLE;
    unsafe {
        let separator_kind = if mutable {
            TYPE_ID_BYTEARRAY
        } else {
            TYPE_ID_BYTES
        };
        let Some(sep_ptr) = obj_from_bits(sep_bits)
            .as_ptr()
            .filter(|&ptr| object_type_id(ptr) == separator_kind)
        else {
            return raise_exception(
                py,
                "TypeError",
                if mutable {
                    "join expects a bytearray separator"
                } else {
                    "join expects a bytes separator"
                },
            );
        };
        // CPython bytearray.join exports its physical separator before __iter__:
        // callbacks may change bytes in place, but cannot resize its storage.
        let _separator_export = if mutable {
            match ScopedBufferExport::new(py, sep_bits) {
                Ok(export) => Some(export),
                Err(()) => return MoltObject::none().bits(),
            }
        } else {
            None
        };
        // Drop sequence custody before releasing acquired views, in acquisition
        // order. Each entry owns either exact bytes or one actual buffer lease.
        let mut buffers = Vec::new();
        let Some(items) = crate::object::iterable::materialize_sequence(
            py,
            items_bits,
            "can only join an iterable",
        ) else {
            return MoltObject::none().bits();
        };
        let result = (|| {
            let items_ptr = obj_from_bits(items.bits()).as_ptr().unwrap();
            let item_count = crate::object::seq_access::locked_len(items_ptr);
            if !mutable && item_count == 1 {
                let item = crate::object::seq_access::pin_item(py, items_ptr, 0).unwrap();
                if type_of_bits(py, item.bits()) == builtin_classes(py).bytes {
                    return item.into_bits();
                }
            }
            let sep_bytes = bytes_like_slice(sep_ptr).unwrap();
            let sep_data = sep_bytes.as_ptr();
            let sep_len = sep_bytes.len();
            let mut parts = Vec::new();
            let mut total_len = 0usize;
            let mut all_same = true;
            for idx in 0..item_count {
                // Retain only the current item across its exporter callback. A
                // displaced future list item must finalize at the replacement.
                let Some(pin) = crate::object::seq_access::pin_item(py, items_ptr, idx) else {
                    return raise_exception(
                        py,
                        "RuntimeError",
                        "sequence changed size during iteration",
                    );
                };
                let bits = pin.bits();
                let physical_bytes = obj_from_bits(bits)
                    .as_ptr()
                    .filter(|&ptr| object_type_id(ptr) == TYPE_ID_BYTES);
                let (data, len) = if let Some(ptr) = physical_bytes
                    && type_of_bits(py, bits) == builtin_classes(py).bytes
                {
                    buffers.push((Some(pin), None));
                    (bytes_data(ptr), bytes_len(ptr))
                } else {
                    // Use the canonical simple-buffer protocol, including foreign
                    // exporters and their exact flags/release callback. A generic
                    // memoryview conversion would request a different protocol.
                    let lease = match molt_cpython_abi::native_custody::acquire_buffer(
                        bits,
                        PyBUF_SIMPLE,
                    ) {
                        Ok(lease) => lease,
                        Err(molt_cpython_abi::api::memory::BufferAcquireError::Preparation) => {
                            return MoltObject::none().bits();
                        }
                        Err(molt_cpython_abi::api::memory::BufferAcquireError::Exporter) => {
                            molt_cpython_abi::api::errors::PyErr_Clear();
                            return raise_exception(
                                py,
                                "TypeError",
                                &format!(
                                    "sequence item {idx}: expected a bytes-like object, {} found",
                                    type_name(py, obj_from_bits(bits)),
                                ),
                            );
                        }
                    };
                    // The exported view now owns its backing object. Extra item
                    // pins would postpone its deallocator beyond later releases.
                    drop(pin);
                    let view = &*lease.descriptor();
                    let data = view.buf as *const u8;
                    let len = view.len;
                    buffers.push((None, Some(lease)));
                    (data, len as usize)
                };
                let Some(length) = total_len
                    .checked_add(len)
                    .and_then(|n| n.checked_add(if idx == 0 { 0 } else { sep_len }))
                    .filter(|&n| n <= isize::MAX as usize)
                else {
                    return raise_exception(py, "OverflowError", "join() result is too long");
                };
                total_len = length;
                if crate::object::seq_access::locked_len(items_ptr) != item_count {
                    return raise_exception(
                        py,
                        "RuntimeError",
                        "sequence changed size during iteration",
                    );
                }
                // A repeated exporter may return different buffers each time;
                // object identity alone does not prove identical spans.
                if let Some(&(first_data, first_len)) = parts.first()
                    && (first_data != data || first_len != len)
                {
                    all_same = false;
                }
                parts.push((data, len));
            }
            let out_ptr = if mutable {
                alloc_bytearray_with_len(py, total_len)
            } else {
                alloc_inline_bytes_with_len(py, total_len, InlineBytesKind::Bytes)
            };
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let mut cursor = if mutable {
                crate::object::buffer_exports::bytearray_data(out_ptr).0
            } else {
                crate::object::layout::InlineBytesStorage::data(out_ptr)
            };
            // Retain the existing repeated-element bulk-copy path for both result
            // kinds. All lengths are bounded by the checked total above.
            if all_same && parts.len() > 1 {
                let (first_data, first_len) = parts[0];
                if first_len != 0 {
                    std::ptr::copy_nonoverlapping(first_data, cursor, first_len);
                    cursor = cursor.add(first_len);
                }
                let pattern_len = sep_len + first_len;
                let repeated_len = pattern_len * (parts.len() - 1);
                if repeated_len != 0 {
                    if sep_len != 0 {
                        std::ptr::copy_nonoverlapping(sep_data, cursor, sep_len);
                    }
                    if first_len != 0 {
                        std::ptr::copy_nonoverlapping(first_data, cursor.add(sep_len), first_len);
                    }
                    let mut filled = pattern_len;
                    while filled < repeated_len {
                        let count = (repeated_len - filled).min(filled);
                        std::ptr::copy_nonoverlapping(cursor, cursor.add(filled), count);
                        filled += count;
                    }
                }
            } else {
                for (idx, &(data, len)) in parts.iter().enumerate() {
                    if idx != 0 && sep_len != 0 {
                        std::ptr::copy_nonoverlapping(sep_data, cursor, sep_len);
                        cursor = cursor.add(sep_len);
                    }
                    if len != 0 {
                        std::ptr::copy_nonoverlapping(data, cursor, len);
                        cursor = cursor.add(len);
                    }
                }
            }
            MoltObject::from_ptr(out_ptr).bits()
        })();
        drop(items);
        for buffer in buffers {
            drop(buffer);
        }
        result
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_find(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        molt_bytes_find_slice(
            hay_bits,
            needle_bits,
            none_bits,
            none_bits,
            false_bits,
            false_bits,
        )
    })
}

// ── 4-arg method-dispatch wrappers for bytes/bytearray ──────────────
// Mirror the str wrappers: (self, sub, start=None, end=None) with None
// sentinel → has_start/has_end conversion.

macro_rules! bytes_slice_method_wrapper {
    ($name:ident, $delegate:ident) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn $name(
            hay_bits: u64,
            needle_bits: u64,
            start_bits: u64,
            end_bits: u64,
        ) -> u64 {
            let has_start = obj_from_bits(start_bits).is_none() as u64 ^ 1;
            let has_end = obj_from_bits(end_bits).is_none() as u64 ^ 1;
            let start = if has_start != 0 {
                start_bits
            } else {
                MoltObject::from_int(0).bits()
            };
            let end = if has_end != 0 {
                end_bits
            } else {
                MoltObject::from_int(0).bits()
            };
            $delegate(
                hay_bits,
                needle_bits,
                start,
                end,
                MoltObject::from_int(has_start as i64).bits(),
                MoltObject::from_int(has_end as i64).bits(),
            )
        }
    };
}

bytes_slice_method_wrapper!(molt_bytes_find_method, molt_bytes_find_slice);
bytes_slice_method_wrapper!(molt_bytes_rfind_method, molt_bytes_rfind_slice);
bytes_slice_method_wrapper!(molt_bytes_index_method, molt_bytes_index_slice);
bytes_slice_method_wrapper!(molt_bytes_rindex_method, molt_bytes_rindex_slice);
bytes_slice_method_wrapper!(molt_bytes_count_method, molt_bytes_count_slice);
bytes_slice_method_wrapper!(molt_bytes_startswith_method, molt_bytes_startswith_slice);
bytes_slice_method_wrapper!(molt_bytes_endswith_method, molt_bytes_endswith_slice);

// bytearray reuses the same runtime functions as bytes, so these wrappers
// delegate to the bytes _slice variants directly.
bytes_slice_method_wrapper!(molt_bytearray_find_method, molt_bytearray_find_slice);
bytes_slice_method_wrapper!(molt_bytearray_rfind_method, molt_bytearray_rfind_slice);
bytes_slice_method_wrapper!(molt_bytearray_index_method, molt_bytearray_index_slice);
bytes_slice_method_wrapper!(molt_bytearray_rindex_method, molt_bytearray_rindex_slice);
bytes_slice_method_wrapper!(molt_bytearray_count_method, molt_bytearray_count_slice);
bytes_slice_method_wrapper!(
    molt_bytearray_startswith_method,
    molt_bytearray_startswith_slice
);
bytes_slice_method_wrapper!(
    molt_bytearray_endswith_method,
    molt_bytearray_endswith_slice
);

#[derive(Clone, Copy)]
enum ByteSearch {
    Find,
    RFind,
    Count,
    StartsWith,
    EndsWith,
}

/// Clinic's optional `start`/`end` slice bounds as the search ABI passes them.
#[derive(Clone, Copy)]
struct SliceBoundBits {
    start: u64,
    end: u64,
    has_start: u64,
    has_end: u64,
}

fn byte_search(
    py: &PyToken<'_>,
    hay_bits: u64,
    needle_bits: u64,
    bounds: SliceBoundBits,
    kind: u32,
    operation: ByteSearch,
) -> u64 {
    let Some(hay_ptr) = obj_from_bits(hay_bits).as_ptr() else {
        return MoltObject::none().bits();
    };
    if unsafe { object_type_id(hay_ptr) } != kind {
        return MoltObject::none().bits();
    }
    let index = |bits, present, default| {
        if to_i64(obj_from_bits(present)).unwrap_or(0) == 0 || obj_from_bits(bits).is_none() {
            Some(default)
        } else {
            crate::builtins::numbers::index_ssize_clamped_from_obj(
                py,
                bits,
                "slice indices must be integers or None or have an __index__ method",
            )
        }
    };
    // Clinic converts both bounds before entering the byte operation.
    let Some(start) = index(bounds.start, bounds.has_start, 0) else {
        return MoltObject::none().bits();
    };
    let Some(end) = index(bounds.end, bounds.has_end, isize::MAX) else {
        return MoltObject::none().bits();
    };
    let (start, end) = (start as i64, end as i64);
    // 3.14's _bytearray_with_buffer introduces the receiver pin only after
    // bound conversion. Earlier versions do not reject exporter-side resize;
    // reacquire their live receiver pointer after callbacks instead of keeping
    // a potentially invalid pre-callback address.
    let _receiver = if kind == TYPE_ID_BYTEARRAY
        && crate::object::ops_sys::runtime_target_at_least(py, 3, 14)
    {
        match crate::object::buffer_exports::ScopedBufferExport::new(py, hay_bits) {
            Ok(owner) => Some(owner),
            Err(()) => return MoltObject::none().bits(),
        }
    } else {
        None
    };
    let tail = matches!(operation, ByteSearch::StartsWith | ByteSearch::EndsWith);
    let run = |source: u64| -> Result<i64, u64> {
        let error = || {
            format!(
                "a bytes-like object is required, not '{}'",
                type_name(py, obj_from_bits(source))
            )
        };
        let numeric;
        let guard;
        let needle: &[u8];
        if tail
            && let Some(pointer) = obj_from_bits(source).as_ptr()
            && unsafe { object_type_id(pointer) } == TYPE_ID_BYTES
        {
            needle = unsafe { std::slice::from_raw_parts(bytes_data(pointer), bytes_len(pointer)) };
        } else if !tail
            && !crate::object::buffer_exports::supports_buffer(py, source)
                .map_err(|error| error.raise(py, "invalid buffer preparation"))?
        {
            let Some(value) = crate::builtins::numbers::index_ssize_clamped_from_obj(
                py,
                source,
                &format!(
                    "argument should be integer or bytes-like object, not '{}'",
                    type_name(py, obj_from_bits(source))
                ),
            ) else {
                return Err(MoltObject::none().bits());
            };
            if !(0..=255).contains(&value) {
                return Err(raise_exception(
                    py,
                    "ValueError",
                    "byte must be in range(0, 256)",
                ));
            }
            numeric = [value as u8];
            needle = &numeric;
        } else {
            let Some(pointer) = obj_from_bits(source).as_ptr() else {
                return Err(raise_exception(py, "TypeError", &error()));
            };
            guard = bytes_like_arg_or_type_error(py, pointer, error)?;
            needle = guard.as_slice();
        }
        let hay = unsafe { std::slice::from_raw_parts(bytes_data(hay_ptr), bytes_len(hay_ptr)) };
        let len = hay.len() as i64;
        let raw_start = if start < 0 {
            start.saturating_add(len).max(0)
        } else {
            start
        };
        let start = raw_start.min(len);
        let end = if end < 0 {
            end.saturating_add(len).max(0)
        } else {
            end.min(len)
        };
        if end < start || raw_start > len {
            return Ok(
                if matches!(operation, ByteSearch::Find | ByteSearch::RFind) {
                    -1
                } else {
                    0
                },
            );
        }
        let hay = &hay[start as usize..end as usize];
        Ok(match operation {
            ByteSearch::Find | ByteSearch::RFind => {
                let found = if matches!(operation, ByteSearch::Find) {
                    bytes_find_impl(hay, needle)
                } else {
                    bytes_rfind_impl(hay, needle)
                };
                if found < 0 { -1 } else { start + found }
            }
            ByteSearch::Count => bytes_count_impl(hay, needle),
            ByteSearch::StartsWith => i64::from(hay.starts_with(needle)),
            ByteSearch::EndsWith => i64::from(hay.ends_with(needle)),
        })
    };
    let result = if tail
        && let Some(tuple) = obj_from_bits(needle_bits).as_ptr()
        && unsafe { object_type_id(tuple) } == TYPE_ID_TUPLE
    {
        let mut matched = 0;
        for index in 0..unsafe { crate::object::seq_access::len(tuple) } {
            let Some(item) = (unsafe { crate::object::seq_access::item(tuple, index) }) else {
                return MoltObject::none().bits();
            };
            match run(item) {
                Ok(0) => {}
                Ok(value) => {
                    matched = value;
                    break;
                }
                Err(error) => return error,
            }
        }
        matched
    } else {
        match run(needle_bits) {
            Ok(value) => value,
            Err(error) => {
                if tail && exception_pending(py) {
                    let pending = crate::builtins::exceptions::molt_exception_last_pending();
                    let type_error = crate::builtins::exceptions::exception_matches_builtin_name(
                        py,
                        pending,
                        "TypeError",
                    );
                    dec_ref_bits(py, pending);
                    if type_error {
                        clear_exception(py);
                        let method = if matches!(operation, ByteSearch::StartsWith) {
                            "startswith"
                        } else {
                            "endswith"
                        };
                        return raise_exception(
                            py,
                            "TypeError",
                            &format!(
                                "{method} first arg must be bytes or a tuple of bytes, not {}",
                                type_name(py, obj_from_bits(needle_bits))
                            ),
                        );
                    }
                }
                return error;
            }
        }
    };
    if tail {
        MoltObject::from_bool(result != 0).bits()
    } else {
        MoltObject::from_int(result).bits()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_find_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        byte_search(
            py,
            hay_bits,
            needle_bits,
            SliceBoundBits {
                start: start_bits,
                end: end_bits,
                has_start: has_start_bits,
                has_end: has_end_bits,
            },
            TYPE_ID_BYTES,
            ByteSearch::Find,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_rfind(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        molt_bytes_rfind_slice(
            hay_bits,
            needle_bits,
            none_bits,
            none_bits,
            false_bits,
            false_bits,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_rfind_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        byte_search(
            py,
            hay_bits,
            needle_bits,
            SliceBoundBits {
                start: start_bits,
                end: end_bits,
                has_start: has_start_bits,
                has_end: has_end_bits,
            },
            TYPE_ID_BYTES,
            ByteSearch::RFind,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_startswith(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        molt_bytes_startswith_slice(
            hay_bits,
            needle_bits,
            none_bits,
            none_bits,
            false_bits,
            false_bits,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_endswith(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        molt_bytes_endswith_slice(
            hay_bits,
            needle_bits,
            none_bits,
            none_bits,
            false_bits,
            false_bits,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_startswith_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        byte_search(
            py,
            hay_bits,
            needle_bits,
            SliceBoundBits {
                start: start_bits,
                end: end_bits,
                has_start: has_start_bits,
                has_end: has_end_bits,
            },
            TYPE_ID_BYTES,
            ByteSearch::StartsWith,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_endswith_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        byte_search(
            py,
            hay_bits,
            needle_bits,
            SliceBoundBits {
                start: start_bits,
                end: end_bits,
                has_start: has_start_bits,
                has_end: has_end_bits,
            },
            TYPE_ID_BYTES,
            ByteSearch::EndsWith,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_count(hay_bits: u64, needle_bits: u64) -> u64 {
    molt_bytes_count_slice(
        hay_bits,
        needle_bits,
        MoltObject::none().bits(),
        MoltObject::none().bits(),
        MoltObject::from_int(0).bits(),
        MoltObject::from_int(0).bits(),
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_count_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        byte_search(
            py,
            hay_bits,
            needle_bits,
            SliceBoundBits {
                start: start_bits,
                end: end_bits,
                has_start: has_start_bits,
                has_end: has_end_bits,
            },
            TYPE_ID_BYTES,
            ByteSearch::Count,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_find(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        molt_bytearray_find_slice(
            hay_bits,
            needle_bits,
            none_bits,
            none_bits,
            false_bits,
            false_bits,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_rfind(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        molt_bytearray_rfind_slice(
            hay_bits,
            needle_bits,
            none_bits,
            none_bits,
            false_bits,
            false_bits,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_startswith(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        molt_bytearray_startswith_slice(
            hay_bits,
            needle_bits,
            none_bits,
            none_bits,
            false_bits,
            false_bits,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_endswith(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        molt_bytearray_endswith_slice(
            hay_bits,
            needle_bits,
            none_bits,
            none_bits,
            false_bits,
            false_bits,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_find_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        byte_search(
            py,
            hay_bits,
            needle_bits,
            SliceBoundBits {
                start: start_bits,
                end: end_bits,
                has_start: has_start_bits,
                has_end: has_end_bits,
            },
            TYPE_ID_BYTEARRAY,
            ByteSearch::Find,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_rfind_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        byte_search(
            py,
            hay_bits,
            needle_bits,
            SliceBoundBits {
                start: start_bits,
                end: end_bits,
                has_start: has_start_bits,
                has_end: has_end_bits,
            },
            TYPE_ID_BYTEARRAY,
            ByteSearch::RFind,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_startswith_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        byte_search(
            py,
            hay_bits,
            needle_bits,
            SliceBoundBits {
                start: start_bits,
                end: end_bits,
                has_start: has_start_bits,
                has_end: has_end_bits,
            },
            TYPE_ID_BYTEARRAY,
            ByteSearch::StartsWith,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_endswith_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        byte_search(
            py,
            hay_bits,
            needle_bits,
            SliceBoundBits {
                start: start_bits,
                end: end_bits,
                has_start: has_start_bits,
                has_end: has_end_bits,
            },
            TYPE_ID_BYTEARRAY,
            ByteSearch::EndsWith,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_count(hay_bits: u64, needle_bits: u64) -> u64 {
    molt_bytearray_count_slice(
        hay_bits,
        needle_bits,
        MoltObject::none().bits(),
        MoltObject::none().bits(),
        MoltObject::from_int(0).bits(),
        MoltObject::from_int(0).bits(),
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_count_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        byte_search(
            py,
            hay_bits,
            needle_bits,
            SliceBoundBits {
                start: start_bits,
                end: end_bits,
                has_start: has_start_bits,
                has_end: has_end_bits,
            },
            TYPE_ID_BYTEARRAY,
            ByteSearch::Count,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_splitlines(hay_bits: u64, keepends_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let keepends = is_truthy(_py, obj_from_bits(keepends_bits));
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let list_bits =
                splitlines_bytes_to_list(_py, hay_bytes, keepends, |bytes| alloc_bytes(_py, bytes));
            list_bits.unwrap_or_else(|| MoltObject::none().bits())
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_splitlines(hay_bits: u64, keepends_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let keepends = is_truthy(_py, obj_from_bits(keepends_bits));
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let list_bits = splitlines_bytes_to_list(_py, hay_bytes, keepends, |bytes| {
                alloc_bytearray(_py, bytes)
            });
            list_bits.unwrap_or_else(|| MoltObject::none().bits())
        }
    })
}

/// A partition part either owns newly copied storage, the actual separator,
/// or the original immutable receiver. The caller supplies the family protocol.
#[derive(Clone, Copy)]
pub(crate) enum PartitionPart {
    Slice,
    Separator,
    Unchanged,
}

/// Search once and transfer exactly three owned references to the result.
/// No-match immutable identity and mutable copying share the same construction.
pub(crate) fn partition_to_tuple<F>(
    py: &PyToken<'_>,
    hay: &[u8],
    sep: &[u8],
    right: bool,
    mut owned_part: F,
) -> Option<u64>
where
    F: FnMut(PartitionPart, &[u8]) -> Option<u64>,
{
    let index = if right {
        bytes_rfind_impl(hay, sep)
    } else {
        bytes_find_impl(hay, sep)
    };
    use PartitionPart::*;
    let parts = if index < 0 {
        if right {
            [(Slice, &[][..]), (Slice, &[][..]), (Unchanged, hay)]
        } else {
            [(Unchanged, hay), (Slice, &[][..]), (Slice, &[][..])]
        }
    } else {
        let index = index as usize;
        [
            (Slice, &hay[..index]),
            (Separator, sep),
            (Slice, &hay[index + sep.len()..]),
        ]
    };
    let mut owned = [0; 3];
    for (index, (kind, bytes)) in parts.into_iter().enumerate() {
        let Some(bits) = owned_part(kind, bytes) else {
            for bits in &owned[..index] {
                dec_ref_bits(py, *bits);
            }
            return None;
        };
        owned[index] = bits;
    }
    let tuple = crate::object::builders::alloc_tuple_owned(py, &owned);
    if tuple.is_null() {
        for bits in owned {
            dec_ref_bits(py, bits);
        }
        return None;
    }
    Some(MoltObject::from_ptr(tuple).bits())
}

fn bytes_partition_protocol(py: &PyToken<'_>, hay_bits: u64, sep_bits: u64, right: bool) -> u64 {
    let Some(hay) = obj_from_bits(hay_bits).as_ptr() else {
        return MoltObject::none().bits();
    };
    if unsafe { object_type_id(hay) } != TYPE_ID_BYTES {
        return MoltObject::none().bits();
    }
    let sep = obj_from_bits(sep_bits);
    let invalid = || {
        format!(
            "a bytes-like object is required, not '{}'",
            type_name(py, sep)
        )
    };
    let Some(sep_ptr) = sep.as_ptr() else {
        return raise_exception(py, "TypeError", &invalid());
    };
    let buffer = match bytes_like_arg_or_type_error(py, sep_ptr, invalid) {
        Ok(buffer) => buffer,
        Err(bits) => return bits,
    };
    let separator = buffer.as_slice();
    if separator.is_empty() {
        return raise_exception(py, "ValueError", "empty separator");
    }
    // Acquisition has completed before observing either immutable payload.
    let receiver = unsafe { std::slice::from_raw_parts(bytes_data(hay), bytes_len(hay)) };
    partition_to_tuple(py, receiver, separator, right, |kind, bytes| match kind {
        PartitionPart::Separator => buffer.owned_exporter(sep_bits),
        PartitionPart::Unchanged => {
            inc_ref_bits(py, hay_bits);
            Some(hay_bits)
        }
        PartitionPart::Slice => {
            let ptr = alloc_bytes(py, bytes);
            (!ptr.is_null()).then(|| MoltObject::from_ptr(ptr).bits())
        }
    })
    .unwrap_or_else(|| MoltObject::none().bits())
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_partition(hay_bits: u64, sep_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        bytes_partition_protocol(py, hay_bits, sep_bits, false)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_rpartition(hay_bits: u64, sep_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        bytes_partition_protocol(py, hay_bits, sep_bits, true)
    })
}

fn bytearray_partition_protocol(
    py: &PyToken<'_>,
    hay_bits: u64,
    sep_bits: u64,
    right: bool,
) -> u64 {
    let Some(hay) = obj_from_bits(hay_bits).as_ptr() else {
        return MoltObject::none().bits();
    };
    if unsafe { object_type_id(hay) } != TYPE_ID_BYTEARRAY {
        return MoltObject::none().bits();
    }
    // _PyByteArray_FromBufferObject requests FULL_RO, copies possibly strided
    // bytes, then releases sep before observing self or allocating result parts.
    let separator = {
        let buffer = match crate::object::buffer_exports::ScopedBuffer::new(py, sep_bits) {
            Ok(buffer) => buffer,
            Err(error) => return error.raise(py, "a bytes-like object is required"),
        };
        match unsafe {
            crate::object::memoryview::collect_bytes_from_descriptor(
                py,
                &crate::cpython_abi_hooks::abi_buffer_view_from_runtime(*buffer.view()),
            )
        } {
            Some(bytes) => bytes,
            None => return MoltObject::none().bits(),
        }
    };
    if separator.is_empty() {
        return raise_exception(py, "ValueError", "empty separator");
    }
    let receiver = unsafe { std::slice::from_raw_parts(bytes_data(hay), bytes_len(hay)) }.to_vec();
    partition_to_tuple(py, &receiver, &separator, right, |_, bytes| {
        let ptr = alloc_bytearray(py, bytes);
        (!ptr.is_null()).then(|| MoltObject::from_ptr(ptr).bits())
    })
    .unwrap_or_else(|| MoltObject::none().bits())
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_partition(hay_bits: u64, sep_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        bytearray_partition_protocol(py, hay_bits, sep_bits, false)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_rpartition(hay_bits: u64, sep_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        bytearray_partition_protocol(py, hay_bits, sep_bits, true)
    })
}

fn bytes_split_impl(
    _py: &PyToken<'_>,
    hay_bits: u64,
    needle_bits: u64,
    maxsplit_bits: u64,
    type_id: u32,
    from_right: bool,
) -> u64 {
    let hay = obj_from_bits(hay_bits);
    let hay_ptr = hay
        .as_ptr()
        .filter(|ptr| unsafe { object_type_id(*ptr) == type_id });
    let Some(hay_ptr) = hay_ptr else {
        let method = if from_right { "rsplit" } else { "split" };
        let receiver_type = if type_id == TYPE_ID_BYTES {
            "bytes"
        } else {
            "bytearray"
        };
        let msg = format!(
            "descriptor '{method}' for '{receiver_type}' objects doesn't apply to a '{}' object",
            type_name(_py, hay)
        );
        return raise_exception::<_>(_py, "TypeError", &msg);
    };
    // __index__ can resize either bytearray argument. Do not acquire any byte
    // slice until it has completed; then consume the live post-callback storage.
    let maxsplit = index_ssize_from_obj(_py, maxsplit_bits);
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    let needle = obj_from_bits(needle_bits);
    unsafe {
        let alloc = |bytes: &[u8]| {
            if type_id == TYPE_ID_BYTES {
                alloc_bytes(_py, bytes)
            } else {
                alloc_bytearray(_py, bytes)
            }
        };
        let result = if needle.is_none() {
            let hay_bytes = bytes_like_slice(hay_ptr).expect("validated bytes receiver");
            if from_right {
                rsplit_bytes_whitespace_to_list_maxsplit(_py, hay_bytes, maxsplit, alloc)
            } else {
                split_bytes_whitespace_to_list_maxsplit(_py, hay_bytes, maxsplit, alloc)
            }
        } else {
            let Some(needle_ptr) = needle.as_ptr() else {
                let msg = format!(
                    "a bytes-like object is required, not '{}'",
                    type_name(_py, needle)
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            };
            let needle_bytes_guard = match bytes_like_arg_or_type_error(_py, needle_ptr, || {
                format!(
                    "a bytes-like object is required, not '{}'",
                    type_name(_py, needle)
                )
            }) {
                Ok(slice) => slice,
                Err(bits) => return bits,
            };
            let needle_bytes = needle_bytes_guard.as_slice();
            if needle_bytes.is_empty() {
                return raise_exception::<_>(_py, "ValueError", "empty separator");
            }
            let hay_bytes = bytes_like_slice(hay_ptr).expect("validated bytes receiver");
            if from_right {
                rsplit_bytes_to_list_maxsplit(_py, hay_bytes, needle_bytes, maxsplit, alloc)
            } else {
                split_bytes_to_list_maxsplit(_py, hay_bytes, needle_bytes, maxsplit, alloc)
            }
        };
        result.unwrap_or_else(|| MoltObject::none().bits())
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_split(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let maxsplit_bits = MoltObject::from_int(-1).bits();
        molt_bytes_split_max(hay_bits, needle_bits, maxsplit_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_split_max(hay_bits: u64, needle_bits: u64, maxsplit_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_split_impl(
            _py,
            hay_bits,
            needle_bits,
            maxsplit_bits,
            TYPE_ID_BYTES,
            false,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_rsplit(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let maxsplit_bits = MoltObject::from_int(-1).bits();
        molt_bytes_rsplit_max(hay_bits, needle_bits, maxsplit_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_rsplit_max(
    hay_bits: u64,
    needle_bits: u64,
    maxsplit_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_split_impl(
            _py,
            hay_bits,
            needle_bits,
            maxsplit_bits,
            TYPE_ID_BYTES,
            true,
        )
    })
}

fn trace_bytes_strip() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_BYTES_STRIP)
}

fn bytes_strip_impl<F>(
    _py: &PyToken<'_>,
    hay_bits: u64,
    chars_bits: u64,
    type_id: u32,
    mut alloc: F,
    left: bool,
    right: bool,
) -> u64
where
    F: FnMut(&[u8]) -> *mut u8,
{
    const ASCII_WHITESPACE: [u8; 6] = [b' ', b'\t', b'\n', b'\r', 0x0b, 0x0c];
    let trace = trace_bytes_strip();
    let hay = obj_from_bits(hay_bits);
    let chars = obj_from_bits(chars_bits);
    let Some(hay_ptr) = hay.as_ptr() else {
        return MoltObject::none().bits();
    };
    unsafe {
        if object_type_id(hay_ptr) != type_id {
            return MoltObject::none().bits();
        }
        let strip_guard = if chars.is_none() {
            None
        } else {
            let Some(chars_ptr) = chars.as_ptr() else {
                let msg = format!(
                    "a bytes-like object is required, not '{}'",
                    type_name(_py, chars)
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            };
            match bytes_like_arg_or_type_error(_py, chars_ptr, || {
                format!(
                    "a bytes-like object is required, not '{}'",
                    type_name(_py, chars)
                )
            }) {
                Ok(buffer) => Some(buffer),
                Err(bits) => return bits,
            }
        };
        let strip_bytes = strip_guard
            .as_ref()
            .map_or(&ASCII_WHITESPACE[..], |buffer| buffer.as_slice());
        let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
        let (start, end) = bytes_strip_range(hay_bytes, strip_bytes, left, right);
        let ptr = alloc(&hay_bytes[start..end]);
        if trace {
            eprintln!(
                "bytes_strip_impl start={start} end={end} out_len={} ptr_null={}",
                end.saturating_sub(start),
                ptr.is_null()
            );
        }
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_strip(hay_bits: u64, chars_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if trace_bytes_strip() {
            let hay = obj_from_bits(hay_bits);
            let info = hay
                .as_ptr()
                .map(|ptr| unsafe { (object_type_id(ptr), bytes_len(ptr)) });
            eprintln!("molt_bytes_strip hay_bits={hay_bits} info={info:?}");
        }
        bytes_strip_impl(
            _py,
            hay_bits,
            chars_bits,
            TYPE_ID_BYTES,
            |bytes| alloc_bytes(_py, bytes),
            true,
            true,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_lstrip(hay_bits: u64, chars_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_strip_impl(
            _py,
            hay_bits,
            chars_bits,
            TYPE_ID_BYTES,
            |bytes| alloc_bytes(_py, bytes),
            true,
            false,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_rstrip(hay_bits: u64, chars_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_strip_impl(
            _py,
            hay_bits,
            chars_bits,
            TYPE_ID_BYTES,
            |bytes| alloc_bytes(_py, bytes),
            false,
            true,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_split(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let maxsplit_bits = MoltObject::from_int(-1).bits();
        molt_bytearray_split_max(hay_bits, needle_bits, maxsplit_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_split_max(
    hay_bits: u64,
    needle_bits: u64,
    maxsplit_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_split_impl(
            _py,
            hay_bits,
            needle_bits,
            maxsplit_bits,
            TYPE_ID_BYTEARRAY,
            false,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_rsplit(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let maxsplit_bits = MoltObject::from_int(-1).bits();
        molt_bytearray_rsplit_max(hay_bits, needle_bits, maxsplit_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_rsplit_max(
    hay_bits: u64,
    needle_bits: u64,
    maxsplit_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_split_impl(
            _py,
            hay_bits,
            needle_bits,
            maxsplit_bits,
            TYPE_ID_BYTEARRAY,
            true,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_strip(hay_bits: u64, chars_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_strip_impl(
            _py,
            hay_bits,
            chars_bits,
            TYPE_ID_BYTEARRAY,
            |bytes| alloc_bytearray(_py, bytes),
            true,
            true,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_lstrip(hay_bits: u64, chars_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_strip_impl(
            _py,
            hay_bits,
            chars_bits,
            TYPE_ID_BYTEARRAY,
            |bytes| alloc_bytearray(_py, bytes),
            true,
            false,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_rstrip(hay_bits: u64, chars_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_strip_impl(
            _py,
            hay_bits,
            chars_bits,
            TYPE_ID_BYTEARRAY,
            |bytes| alloc_bytearray(_py, bytes),
            false,
            true,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_index_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let idx_bits = molt_bytes_find_slice(
            hay_bits,
            needle_bits,
            start_bits,
            end_bits,
            has_start_bits,
            has_end_bits,
        );
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if to_i64(obj_from_bits(idx_bits)).unwrap_or(-1) < 0 {
            return raise_exception::<_>(_py, "ValueError", "subsection not found");
        }
        idx_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_rindex_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let idx_bits = molt_bytes_rfind_slice(
            hay_bits,
            needle_bits,
            start_bits,
            end_bits,
            has_start_bits,
            has_end_bits,
        );
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if to_i64(obj_from_bits(idx_bits)).unwrap_or(-1) < 0 {
            return raise_exception::<_>(_py, "ValueError", "subsection not found");
        }
        idx_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_index_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let idx_bits = molt_bytearray_find_slice(
            hay_bits,
            needle_bits,
            start_bits,
            end_bits,
            has_start_bits,
            has_end_bits,
        );
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if to_i64(obj_from_bits(idx_bits)).unwrap_or(-1) < 0 {
            return raise_exception::<_>(_py, "ValueError", "subsection not found");
        }
        idx_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_rindex_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let idx_bits = molt_bytearray_rfind_slice(
            hay_bits,
            needle_bits,
            start_bits,
            end_bits,
            has_start_bits,
            has_end_bits,
        );
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if to_i64(obj_from_bits(idx_bits)).unwrap_or(-1) < 0 {
            return raise_exception::<_>(_py, "ValueError", "subsection not found");
        }
        idx_bits
    })
}

#[cfg(test)]
mod partition_contract_tests {
    use super::*;
    use crate::object::seq_access::with_immutable_tuple_slice;
    use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};

    fn refs(bits: u64) -> u32 {
        unsafe {
            (*header_from_obj_ptr(obj_from_bits(bits).as_ptr().unwrap())).ref_count_snapshot()
        }
    }

    fn value(py: &PyToken<'_>, kind: u32, data: &[u8]) -> u64 {
        let ptr = match kind {
            TYPE_ID_STRING => alloc_string(py, data),
            TYPE_ID_BYTES => alloc_bytes(py, data),
            TYPE_ID_BYTEARRAY => alloc_bytearray(py, data),
            _ => unreachable!(),
        };
        assert!(!ptr.is_null());
        MoltObject::from_ptr(ptr).bits()
    }

    #[test]
    fn partition_identity_and_copied_parts_transfer_only_one_owned_reference() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for (kind, functions) in [
                (
                    TYPE_ID_BYTES,
                    [
                        molt_bytes_partition as extern "C" fn(u64, u64) -> u64,
                        molt_bytes_rpartition,
                    ],
                ),
                (
                    TYPE_ID_BYTEARRAY,
                    [
                        molt_bytearray_partition as extern "C" fn(u64, u64) -> u64,
                        molt_bytearray_rpartition,
                    ],
                ),
                (
                    TYPE_ID_STRING,
                    [
                        crate::object::ops_string::molt_string_partition
                            as extern "C" fn(u64, u64) -> u64,
                        crate::object::ops_string::molt_string_rpartition,
                    ],
                ),
            ] {
                let hay = value(py, kind, b"left:<>right:<>tail:");
                let sep = value(py, kind, b"<>");
                let absent = value(py, kind, b"not found");
                let hay_refs = refs(hay);
                let sep_refs = refs(sep);
                for (right, partition) in functions.into_iter().enumerate() {
                    let result = partition(hay, sep);
                    assert!(!exception_pending(py));
                    let parts = unsafe {
                        with_immutable_tuple_slice(
                            obj_from_bits(result).as_ptr().unwrap(),
                            |items| <[u64; 3]>::try_from(items).unwrap(),
                        )
                    }
                    .unwrap();
                    if kind == TYPE_ID_BYTEARRAY {
                        assert_ne!(parts[1], sep);
                    } else {
                        assert_eq!(parts[1], sep);
                        assert_eq!(refs(sep), sep_refs + 1);
                    }
                    for part in [parts[0], parts[2]] {
                        assert_eq!(refs(part), 1, "the tuple transfers newly allocated parts");
                    }
                    dec_ref_bits(py, result);
                    assert_eq!(refs(sep), sep_refs);
                    let result = partition(hay, absent);
                    assert!(!exception_pending(py));
                    let whole = unsafe {
                        with_immutable_tuple_slice(
                            obj_from_bits(result).as_ptr().unwrap(),
                            |items| items[if right == 0 { 0 } else { 2 }],
                        )
                    }
                    .unwrap();
                    if kind == TYPE_ID_BYTEARRAY {
                        assert_ne!(whole, hay);
                        assert_eq!(refs(whole), 1);
                    } else {
                        assert_eq!(whole, hay);
                        assert_eq!(refs(hay), hay_refs + 1);
                    }
                    dec_ref_bits(py, result);
                    assert_eq!(refs(hay), hay_refs);
                }
                for bits in [hay, sep, absent] {
                    dec_ref_bits(py, bits);
                }
            }
        });
    }

    #[test]
    fn partition_bytes_retains_the_actual_memoryview_separator_without_an_export_pin() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let hay = value(py, TYPE_ID_BYTES, b"left<>right");
            let storage = value(py, TYPE_ID_BYTEARRAY, b"<>");
            let view = crate::object::ops_memoryview::molt_memoryview_new(storage);
            assert!(!exception_pending(py));
            let count = refs(view);
            for partition in [molt_bytes_partition, molt_bytes_rpartition] {
                let result = partition(hay, view);
                assert!(!exception_pending(py));
                assert_eq!(
                    unsafe {
                        with_immutable_tuple_slice(
                            obj_from_bits(result).as_ptr().unwrap(),
                            |items| items[1],
                        )
                    },
                    Some(view)
                );
                assert_eq!(refs(view), count + 1);
                dec_ref_bits(py, result);
                assert_eq!(refs(view), count);
            }
            crate::object::ops_memoryview::molt_memoryview_release(view);
            assert!(!exception_pending(py));
            for bits in [view, storage, hay] {
                dec_ref_bits(py, bits);
            }
        });
    }

    #[test]
    fn partition_failure_retires_every_prepared_reference() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let item = value(py, TYPE_ID_BYTEARRAY, b"owned-part");
            let count = refs(item);
            for fail_at in [0, 1, 2, 3] {
                let mut calls = 0;
                let result = partition_to_tuple(py, b"left<>right", b"<>", false, |_, _| {
                    let fail = calls == fail_at;
                    calls += 1;
                    if fail {
                        None
                    } else {
                        inc_ref_bits(py, item);
                        Some(item)
                    }
                });
                if fail_at < 3 {
                    assert!(result.is_none());
                } else {
                    dec_ref_bits(py, result.unwrap());
                }
                assert_eq!(refs(item), count);
            }
            struct RestoreBudget;
            impl Drop for RestoreBudget {
                fn drop(&mut self) {
                    set_tracker(Box::new(UnlimitedTracker));
                }
            }
            set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                max_memory: Some(0),
                max_allocations: Some(0),
                ..Default::default()
            })));
            let budget = RestoreBudget;
            let result = partition_to_tuple(py, b"left<>right", b"<>", false, |_, _| {
                inc_ref_bits(py, item);
                Some(item)
            });
            drop(budget);
            assert!(result.is_none());
            assert!(exception_pending(py));
            clear_exception(py);
            assert_eq!(refs(item), count);
            dec_ref_bits(py, item);
        });
    }
}
