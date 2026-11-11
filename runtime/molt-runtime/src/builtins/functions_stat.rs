use crate::BuiltinExc;
use molt_obj_model::MoltObject;
use molt_runtime_platform::stat_support;

use crate::object::ops_sys::runtime_target_minor;
use crate::{alloc_string, alloc_tuple, obj_from_bits, raise_exception, to_i64};

pub(crate) fn molt_stat_constants_attached_body(_py: &crate::PyToken<'_>) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
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
        return Err(raise_exception::<_>(
            _py,
            BuiltinExc::TypeError,
            "mode must be int",
        ));
    };
    Ok(mode)
}

pub(crate) fn molt_stat_ifmt_attached_body(_py: &crate::PyToken<'_>, mode_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_int(stat_support::stat_ifmt(mode)).bits()
    })
}

pub(crate) fn molt_stat_imode_attached_body(_py: &crate::PyToken<'_>, mode_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_int(stat_support::stat_imode(mode)).bits()
    })
}

pub(crate) fn molt_stat_isdir_attached_body(_py: &crate::PyToken<'_>, mode_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(stat_support::stat_isdir(mode)).bits()
    })
}

pub(crate) fn molt_stat_isreg_attached_body(_py: &crate::PyToken<'_>, mode_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(stat_support::stat_isreg(mode)).bits()
    })
}

pub(crate) fn molt_stat_ischr_attached_body(_py: &crate::PyToken<'_>, mode_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(stat_support::stat_ischr(mode)).bits()
    })
}

pub(crate) fn molt_stat_isblk_attached_body(_py: &crate::PyToken<'_>, mode_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(stat_support::stat_isblk(mode)).bits()
    })
}

pub(crate) fn molt_stat_isfifo_attached_body(_py: &crate::PyToken<'_>, mode_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(stat_support::stat_isfifo(mode)).bits()
    })
}

pub(crate) fn molt_stat_islnk_attached_body(_py: &crate::PyToken<'_>, mode_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(stat_support::stat_islnk(mode)).bits()
    })
}

pub(crate) fn molt_stat_issock_attached_body(_py: &crate::PyToken<'_>, mode_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let mode = match parse_stat_mode(_py, mode_bits) {
            Ok(mode) => mode,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(stat_support::stat_issock(mode)).bits()
    })
}

pub(crate) fn molt_stat_isdoor_attached_body(_py: &crate::PyToken<'_>, mode_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
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

pub(crate) fn molt_stat_isport_attached_body(_py: &crate::PyToken<'_>, mode_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
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

pub(crate) fn molt_stat_iswht_attached_body(_py: &crate::PyToken<'_>, mode_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
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

pub(crate) fn molt_stat_filemode_attached_body(_py: &crate::PyToken<'_>, mode_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
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

include!("../attached_exports_generated/builtins/functions_stat.rs");
