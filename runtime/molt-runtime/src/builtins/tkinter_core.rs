//! Re-export bridge: delegates to `molt_runtime_tk::tkinter_core`.
//!
//! The canonical implementation lives in the extracted `molt_runtime_tk` crate.
//! This file provides `#[unsafe(no_mangle)]` entry points so the linker
//! exports them with the expected symbol names.

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_event_build_from_args(_widget_path_bits: u64, args_bits: u64) -> u64 {
    molt_runtime_tk::tkinter_core::molt_tk_event_build_from_args(_widget_path_bits, args_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_event_int(value_bits: u64) -> u64 {
    molt_runtime_tk::tkinter_core::molt_tk_event_int(value_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_event_state_decode(state_bits: u64) -> u64 {
    molt_runtime_tk::tkinter_core::molt_tk_event_state_decode(state_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_splitdict(tcl_str_bits: u64, cut_minus_bits: u64) -> u64 {
    molt_runtime_tk::tkinter_core::molt_tk_splitdict(tcl_str_bits, cut_minus_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_flatten_args(args_bits: u64) -> u64 {
    molt_runtime_tk::tkinter_core::molt_tk_flatten_args(args_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_cnfmerge(cnf_bits: u64, kw_bits: u64) -> u64 {
    molt_runtime_tk::tkinter_core::molt_tk_cnfmerge(cnf_bits, kw_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_normalize_option(name_bits: u64) -> u64 {
    molt_runtime_tk::tkinter_core::molt_tk_normalize_option(name_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_hex_to_rgb(color_bits: u64) -> u64 {
    molt_runtime_tk::tkinter_core::molt_tk_hex_to_rgb(color_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_normalize_delay_ms(delay_bits: u64) -> u64 {
    molt_runtime_tk::tkinter_core::molt_tk_normalize_delay_ms(delay_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_convert_stringval(text_bits: u64) -> u64 {
    molt_runtime_tk::tkinter_core::molt_tk_convert_stringval(text_bits)
}
