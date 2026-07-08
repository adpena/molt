use molt_obj_model::MoltObject;
use molt_runtime_platform::stat_support;

use crate::object::ops_sys::runtime_target_minor;
use crate::{alloc_string, alloc_tuple, obj_from_bits, raise_exception, to_i64};

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_constants() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let has_313_constants = runtime_target_minor(_py) >= 13;
        let values = stat_support::stat_constants_payload(has_313_constants);
        let payload: Vec<u64> = values
            .into_iter()
            .map(|value| MoltObject::from_int(value).bits())
            .collect();
        let tuple_ptr = alloc_tuple(_py, &payload);
        if tuple_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

fn parse_stat_mode(_py: &crate::PyToken<'_>, mode_bits: u64) -> Result<i64, u64> {
    let Some(mode) = to_i64(obj_from_bits(mode_bits)) else {
        return Err(raise_exception::<_>(_py, "TypeError", "mode must be int"));
    };
    Ok(mode)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_ifmt(mode_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_int(stat_support::stat_ifmt(mode)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_imode(mode_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_int(stat_support::stat_imode(mode)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_isdir(mode_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(stat_support::stat_isdir(mode)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_isreg(mode_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(stat_support::stat_isreg(mode)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_ischr(mode_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(stat_support::stat_ischr(mode)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_isblk(mode_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(stat_support::stat_isblk(mode)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_isfifo(mode_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(stat_support::stat_isfifo(mode)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_islnk(mode_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(stat_support::stat_islnk(mode)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_issock(mode_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(stat_support::stat_issock(mode)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_isdoor(mode_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if stat_support::S_IFDOOR == 0 {
            return MoltObject::from_bool(false).bits();
        }
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(stat_support::stat_isdoor(mode)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_isport(mode_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if stat_support::S_IFPORT == 0 {
            return MoltObject::from_bool(false).bits();
        }
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(stat_support::stat_isport(mode)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_iswht(mode_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if stat_support::S_IFWHT == 0 {
            return MoltObject::from_bool(false).bits();
        }
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(stat_support::stat_iswht(mode)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stat_filemode(mode_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        let out = stat_support::stat_filemode(mode);
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}
