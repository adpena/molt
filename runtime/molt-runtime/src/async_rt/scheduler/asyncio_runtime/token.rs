use crate::BuiltinExc;
use crate::PyToken;
use crate::runtime_flags::{self, RuntimeFlag};
use crate::{obj_from_bits, raise_exception, to_i64};

pub(super) fn asyncio_parse_token_id(_py: &PyToken<'_>, token_bits: u64) -> Result<u64, u64> {
    let Some(token_id) = to_i64(obj_from_bits(token_bits)) else {
        if runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_BAD_ASYNCIO_TOKEN) {
            let value = crate::format_obj_str(_py, obj_from_bits(token_bits));
            if crate::exception_pending(_py) {
                return Err(crate::MoltObject::none().bits());
            }
            eprintln!(
                "molt bad asyncio token type={} value={}",
                crate::type_name(_py, obj_from_bits(token_bits)),
                value
            );
        }
        return Err(raise_exception::<u64>(
            _py,
            BuiltinExc::TypeError,
            "token_id must be int",
        ));
    };
    if token_id < 0 {
        return Err(raise_exception::<u64>(
            _py,
            BuiltinExc::ValueError,
            "token_id must be >= 0",
        ));
    }
    Ok(token_id as u64)
}
