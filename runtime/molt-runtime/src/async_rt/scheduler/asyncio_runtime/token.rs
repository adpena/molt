use crate::PyToken;
use crate::{obj_from_bits, raise_exception, to_i64};

pub(super) fn asyncio_parse_token_id(_py: &PyToken<'_>, token_bits: u64) -> Result<u64, u64> {
    let Some(token_id) = to_i64(obj_from_bits(token_bits)) else {
        if matches!(
            std::env::var("MOLT_TRACE_BAD_ASYNCIO_TOKEN")
                .ok()
                .as_deref(),
            Some("1")
        ) {
            eprintln!(
                "molt bad asyncio token type={} value={}",
                crate::type_name(_py, obj_from_bits(token_bits)),
                crate::format_obj_str(_py, obj_from_bits(token_bits))
            );
        }
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "token_id must be int",
        ));
    };
    if token_id < 0 {
        return Err(raise_exception::<u64>(
            _py,
            "ValueError",
            "token_id must be >= 0",
        ));
    }
    Ok(token_id as u64)
}
