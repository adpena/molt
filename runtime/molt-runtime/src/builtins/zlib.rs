use crate::*;
use flate2::Compression;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use std::io::{Read, Write};

#[unsafe(no_mangle)]
pub extern "C" fn molt_deflate_raw(data_bits: u64, level_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(data_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "deflate expects bytes");
        };
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id != TYPE_ID_BYTES && type_id != TYPE_ID_BYTEARRAY {
                let msg = format!("deflate expects bytes, got {}", type_name(_py, obj));
                return raise_exception::<u64>(_py, "TypeError", &msg);
            }
            let len = bytes_len(ptr);
            let data = bytes_data(ptr);
            let slice = std::slice::from_raw_parts(data, len);
            let level_obj = obj_from_bits(level_bits);
            let compression = if level_obj.is_none() {
                Compression::default()
            } else {
                let val = index_i64_from_obj(_py, level_bits, "deflate level must be int");
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                if !(-1..=9).contains(&val) {
                    return raise_exception::<u64>(
                        _py,
                        "ValueError",
                        "deflate level must be in range -1..9",
                    );
                }
                if val == -1 {
                    Compression::default()
                } else {
                    Compression::new(val as u32)
                }
            };
            let mut encoder = DeflateEncoder::new(Vec::new(), compression);
            if encoder.write_all(slice).is_err() {
                return raise_exception::<u64>(_py, "ValueError", "deflate failed");
            }
            let out = match encoder.finish() {
                Ok(val) => val,
                Err(_) => return raise_exception::<u64>(_py, "ValueError", "deflate failed"),
            };
            let out_ptr = alloc_bytes(_py, &out);
            if out_ptr.is_null() {
                return raise_exception::<u64>(_py, "MemoryError", "deflate out of memory");
            }
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inflate_raw(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(data_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "inflate expects bytes");
        };
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id != TYPE_ID_BYTES && type_id != TYPE_ID_BYTEARRAY {
                let msg = format!("inflate expects bytes, got {}", type_name(_py, obj));
                return raise_exception::<u64>(_py, "TypeError", &msg);
            }
            let len = bytes_len(ptr);
            let data = bytes_data(ptr);
            let slice = std::slice::from_raw_parts(data, len);
            let mut decoder = DeflateDecoder::new(slice);
            let mut out = Vec::new();
            if decoder.read_to_end(&mut out).is_err() {
                return raise_exception::<u64>(_py, "ValueError", "invalid deflate data");
            }
            let out_ptr = alloc_bytes(_py, &out);
            if out_ptr.is_null() {
                return raise_exception::<u64>(_py, "MemoryError", "inflate out of memory");
            }
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}
