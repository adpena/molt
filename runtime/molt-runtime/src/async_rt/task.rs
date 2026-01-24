use crate::{obj_from_bits, resolve_ptr};

pub(crate) fn resolve_task_ptr(bits: u64) -> Option<*mut u8> {
    let obj = obj_from_bits(bits);
    if let Some(ptr) = obj.as_ptr() {
        return Some(ptr);
    }
    if !obj.is_float() {
        return None;
    }
    let high = bits >> 48;
    if high == 0 || high == 0xffff {
        let addr = bits as usize;
        if addr < 4096 || (addr & 0x7) != 0 {
            return None;
        }
        return resolve_ptr(bits);
    }
    None
}
