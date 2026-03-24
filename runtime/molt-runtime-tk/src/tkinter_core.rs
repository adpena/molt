//! Tkinter core intrinsics — event parsing, Tcl list/dict parsing, option
//! normalization, color parsing, and value conversion.
//!
//! These intrinsics move hot-path tkinter helper logic from Python shims into
//! Rust, eliminating per-event allocation churn and string re-parsing overhead
//! in bind callbacks, widget configuration, and Tcl response decoding.
//!
//! All public functions follow the Molt intrinsic ABI:
//!   `#[unsafe(no_mangle)] pub extern "C" fn name(args: u64...) -> u64`
//! where all values are NaN-boxed u64 bits (MoltObject).
//!
//! WASM compatibility: All intrinsics in this module are **parsing-only** —
//! pure string/numeric operations with no I/O, no display server interaction,
//! and no platform-specific syscalls. They compile and run correctly on all
//! targets including wasm32-wasi and wasm32-unknown-unknown.
//!
//! Note: tkinter as a whole is NOT available on WASM (no display server / Tcl/Tk
//! runtime). These parsing intrinsics are safe on WASM, but the actual Tk widget
//! operations (which live in the Python shim layer and communicate with a Tcl
//! interpreter) are gated at the Python import level — `import tkinter` raises
//! `ImportError` on WASM targets. This module does not need `#[cfg]` gating
//! because it never touches the display server or Tcl interpreter directly.

use molt_runtime_core::prelude::*;

// ---------------------------------------------------------------------------
// Compatibility shims: map the old pub(crate) runtime API to the new
// extern-"C" FFI wrappers provided by molt-runtime-core::prelude.
//
// The original tkinter_core.rs used functions like alloc_string(_py, bytes),
// raise_exception::<u64>(_py, kind, msg), to_i64(obj), etc.
// Below we provide equivalent thin wrappers so the rest of the file
// requires minimal edits.
// ---------------------------------------------------------------------------

/// Read a string value as an owned Rust String.
fn string_obj_to_owned(obj: MoltObject) -> Option<String> {
    rt_string_as_bytes(obj.bits())
        .map(|b| String::from_utf8_lossy(b).into_owned())
}

/// Extract i64 from a MoltObject.
fn to_i64(obj: MoltObject) -> Option<i64> {
    obj.as_int()
}

/// Extract f64 from a MoltObject.
fn to_f64(obj: MoltObject) -> Option<f64> {
    obj.as_float()
}

// ─── Internal helpers ────────────────────────────────────────────────────────

/// Allocate a Molt string from a Rust `&str` and return its NaN-boxed bits.
/// Returns `Err(bits)` on allocation failure (MemoryError raised).
fn alloc_str_bits(value: &str) -> Result<u64, u64> {
    let bits = rt_string_from(value);
    if bits == 0 || rt_exception_pending() {
        return Err(rt_raise_str("MemoryError", "failed to allocate tkinter_core string"));
    }
    Ok(bits)
}

/// Allocate a Molt list from a slice of owned bits and return its NaN-boxed bits.
/// The elements are inc-ref'd by `rt_list`.
/// Returns `Err(bits)` on allocation failure.
fn alloc_list_bits(elems: &[u64]) -> Result<u64, u64> {
    let bits = rt_list(elems);
    if bits == 0 || rt_exception_pending() {
        return Err(rt_raise_str("MemoryError", "failed to allocate tkinter_core list"));
    }
    Ok(bits)
}

/// Allocate a Molt tuple from a slice of owned bits and return its NaN-boxed bits.
/// The elements are inc-ref'd by `rt_tuple`.
/// Returns `Err(bits)` on allocation failure.
fn alloc_tuple_result(elems: &[u64]) -> Result<u64, u64> {
    let bits = rt_tuple(elems);
    if bits == 0 || rt_exception_pending() {
        return Err(rt_raise_str("MemoryError", "failed to allocate tkinter_core tuple"));
    }
    Ok(bits)
}

/// Extract a Rust `String` from NaN-boxed bits that should be a Molt string.
/// Returns `None` if the value is not a string (could be int, None, etc.).
fn bits_to_string(bits: u64) -> Option<String> {
    string_obj_to_owned(obj_from_bits(bits))
}

/// Read the elements of a list or tuple pointer into a `Vec<u64>`.
/// Returns `None` if the bits don't refer to a list/tuple pointer.
fn read_seq_elements(bits: u64) -> Option<Vec<u64>> {
    let obj = obj_from_bits(bits);
    let ptr = obj.as_ptr()?;
    let type_id = object_type_id(ptr);
    if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
        return None;
    }
    let vec = seq_vec_ref(ptr);
    Some(vec.to_vec())
}

// Low-level object layout access — resolved at link time from molt-runtime.
unsafe extern "C" {
    fn molt_rt_object_type_id(ptr: *mut u8) -> u32;
    fn molt_rt_seq_vec_ref(ptr: *mut u8, out_ptr: *mut *const u64, out_len: *mut usize);
    fn molt_rt_dict_order(ptr: *mut u8, out_ptr: *mut *const u64, out_len: *mut usize);
}

fn object_type_id(ptr: *mut u8) -> u32 {
    unsafe { molt_rt_object_type_id(ptr) }
}

fn seq_vec_ref(ptr: *mut u8) -> &'static [u64] {
    let mut out_ptr: *const u64 = std::ptr::null();
    let mut out_len: usize = 0;
    unsafe {
        molt_rt_seq_vec_ref(ptr, &mut out_ptr, &mut out_len);
        std::slice::from_raw_parts(out_ptr, out_len)
    }
}

fn dict_order(ptr: *mut u8) -> Vec<u64> {
    let mut out_ptr: *const u64 = std::ptr::null();
    let mut out_len: usize = 0;
    unsafe {
        molt_rt_dict_order(ptr, &mut out_ptr, &mut out_len);
        std::slice::from_raw_parts(out_ptr, out_len).to_vec()
    }
}

/// Try to interpret bits as an integer.  Handles int, bool, and int-subclass.
fn bits_as_i64(bits: u64) -> Option<i64> {
    to_i64(obj_from_bits(bits))
}

/// Try to interpret bits as a float.
fn bits_as_f64(bits: u64) -> Option<f64> {
    to_f64(obj_from_bits(bits))
}

/// Check if the value is None.
fn bits_is_none(bits: u64) -> bool {
    obj_from_bits(bits).is_none()
}

/// Check if the value is an empty string.
fn bits_is_empty_string(bits: u64) -> bool {
    bits_to_string(bits).is_some_and(|s| s.is_empty())
}

/// Check if the value is None or an empty string.
fn bits_is_empty_or_none(bits: u64) -> bool {
    bits_is_none(bits) || bits_is_empty_string(bits)
}

// ─── Event parsing internals ─────────────────────────────────────────────────

/// The 19 substitution fields in tkinter event binding:
///   %#  %b  %f  %h  %k  %s  %t  %w  %x  %y  %A  %E  %K  %N  %W  %T  %X  %Y  %D
///
/// Field indices:
///   0: serial (%#)       — int
///   1: num (%b)          — int (button number)
///   2: focus (%f)        — bool
///   3: height (%h)       — int
///   4: keycode (%k)      — int
///   5: state (%s)        — int (modifier bitmask)
///   6: time (%t)         — int (timestamp ms)
///   7: width (%w)        — int
///   8: x (%x)            — int
///   9: y (%y)            — int
///  10: char (%A)         — string
///  11: send_event (%E)   — bool
///  12: keysym (%K)       — string
///  13: keysym_num (%N)   — int
///  14: widget_path (%W)  — string
///  15: type (%T)         — string
///  16: x_root (%X)       — int
///  17: y_root (%Y)       — int
///  18: delta (%D)        — int (0 if empty)
const EVENT_FIELD_COUNT: usize = 19;

/// Try to parse a string value as an integer for event field conversion.
/// Mirrors `_event_int`: if the value is already an int, return it; if it's a
/// string of digits (possibly negative), parse it; otherwise return the
/// original value unchanged.
fn event_int_convert(bits: u64) -> u64 {
    // Already an integer?
    if let Some(i) = bits_as_i64(bits) {
        return MoltObject::from_int(i).bits();
    }
    // String of digits (possibly with leading minus)?
    if let Some(text) = bits_to_string(bits) {
        let trimmed = text.trim();
        if !trimmed.is_empty()
            && trimmed
                .trim_start_matches('-')
                .chars()
                .all(|c| c.is_ascii_digit())
            && let Ok(i) = trimmed.parse::<i64>()
        {
            return MoltObject::from_int(i).bits();
        }
    }
    // Return original value unchanged
    bits
}

/// Parse a bool from string text (Tcl-style boolean values).
fn parse_tcl_bool(text: &str) -> Option<bool> {
    let lowered = text.trim().to_ascii_lowercase();
    match lowered.as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => {
            // Prefix matching
            for truthy in &["true", "yes", "on"] {
                if truthy.starts_with(&lowered) && !lowered.is_empty() {
                    return Some(true);
                }
            }
            for falsy in &["false", "no", "off"] {
                if falsy.starts_with(&lowered) && !lowered.is_empty() {
                    return Some(false);
                }
            }
            None
        }
    }
}

/// Convert a substitution field to a boolean value.
fn event_bool_convert(bits: u64) -> u64 {
    let obj = obj_from_bits(bits);
    if obj.is_bool() {
        return bits;
    }
    if let Some(i) = to_i64(obj) {
        return MoltObject::from_bool(i != 0).bits();
    }
    if let Some(text) = string_obj_to_owned(obj)
        && let Some(b) = parse_tcl_bool(&text)
    {
        return MoltObject::from_bool(b).bits();
    }
    if let Some(f) = to_f64(obj) {
        return MoltObject::from_bool(f != 0.0).bits();
    }
    // Cannot parse — return None
    MoltObject::none().bits()
}

// ─── Modifier name table for state bitmask decoding ──────────────────────────

const MODIFIER_NAMES: &[&str] = &[
    "Shift", "Lock", "Control", "Mod1", "Mod2", "Mod3", "Mod4", "Mod5", "Button1", "Button2",
    "Button3", "Button4", "Button5",
];

// ─── Public intrinsics ───────────────────────────────────────────────────────

/// Parse a 19-element list/tuple of event substitution args into a list of
/// typed values (ints, bools, strings).
///
/// Takes `widget_path` (string, the widget's Tcl path name) and `args` (list
/// of 19 string elements from Tk's bind substitution).
/// Returns a list of 19 typed values:
///   ints: serial, num, height, keycode, state, time, width, x, y, x_root, y_root, delta
///   bools: focus, send_event
///   strings: char, keysym, type, widget_path
///
/// Returns None on malformed input.
#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_event_build_from_args(_widget_path_bits: u64, args_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(elems) = read_seq_elements(args_bits) else {
            return MoltObject::none().bits();
        };
        if elems.len() != EVENT_FIELD_COUNT {
            return MoltObject::none().bits();
        }

        // Build the typed payload:
        //  0: serial   (int)
        //  1: num      (int)
        //  2: focus    (bool)
        //  3: height   (int)
        //  4: keycode  (int)
        //  5: state    (int)
        //  6: time     (int)
        //  7: width    (int)
        //  8: x        (int)
        //  9: y        (int)
        // 10: char     (string, pass-through)
        // 11: send_event (bool)
        // 12: keysym   (string, pass-through)
        // 13: keysym_num (int)
        // 14: widget_path (string, pass-through)
        // 15: type     (string, pass-through)
        // 16: x_root   (int)
        // 17: y_root   (int)
        // 18: delta    (int, default 0 if empty)
        let delta = if bits_is_empty_or_none(elems[18]) {
            MoltObject::from_int(0).bits()
        } else {
            event_int_convert(elems[18])
        };

        let payload = [
            event_int_convert(elems[0]),   // serial
            event_int_convert(elems[1]),   // num
            event_bool_convert(elems[2]),  // focus
            event_int_convert(elems[3]),   // height
            event_int_convert(elems[4]),   // keycode
            event_int_convert(elems[5]),   // state
            event_int_convert(elems[6]),   // time
            event_int_convert(elems[7]),   // width
            event_int_convert(elems[8]),   // x
            event_int_convert(elems[9]),   // y
            elems[10],                     // char (pass-through)
            event_bool_convert(elems[11]), // send_event
            elems[12],                     // keysym (pass-through)
            event_int_convert(elems[13]),  // keysym_num
            elems[14],                     // widget_path (pass-through)
            elems[15],                     // type (pass-through)
            event_int_convert(elems[16]),  // x_root
            event_int_convert(elems[17]),  // y_root
            delta,                         // delta
        ];

        match alloc_list_bits(&payload) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

/// Convert a string value to int if it looks numeric (with optional leading
/// minus). Returns the original value unchanged if conversion fails.
///
/// Mirrors tkinter `_event_int(widget, value)`:
///   - If value is already int, return it.
///   - If value is a string and `value.lstrip("-").isdigit()`, parse as int.
///   - Otherwise return value unchanged.
#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_event_int(value_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, { event_int_convert(value_bits) })
}

/// Decode an event state bitmask into a list of modifier name strings.
///
/// The state value is a bitmask where bits 0-12 correspond to:
///   Shift, Lock, Control, Mod1, Mod2, Mod3, Mod4, Mod5,
///   Button1, Button2, Button3, Button4, Button5
///
/// Any remaining high bits are appended as a hex string.
/// If no modifiers match and no high bits remain, returns a list containing
/// just "0x0".
///
/// Returns a Molt list of strings, or the "|"-joined string representation.
#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_event_state_decode(state_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(state_value) = bits_as_i64(state_bits) else {
            // Not an integer — return None
            return MoltObject::none().bits();
        };

        let mut parts: Vec<u64> = Vec::new();
        let mut remaining = state_value;

        for (index, name) in MODIFIER_NAMES.iter().enumerate() {
            if remaining & (1i64 << index) != 0 {
                match alloc_str_bits(name) {
                    Ok(bits) => parts.push(bits),
                    Err(bits) => return bits,
                }
            }
        }

        // Clear the known modifier bits
        remaining &= !((1i64 << MODIFIER_NAMES.len()) - 1);

        // If there are leftover high bits or no modifiers matched, append hex
        if remaining != 0 || parts.is_empty() {
            let hex = format!("{:#x}", remaining);
            match alloc_str_bits(&hex) {
                Ok(bits) => parts.push(bits),
                Err(bits) => return bits,
            }
        }

        // Return the list of modifier strings
        match alloc_list_bits(&parts) {
            Ok(list_bits) => {
                // Dec-ref the individual strings since alloc_list inc-refs them
                for &part_bits in &parts {
                    rt_dec_ref(part_bits);
                }
                list_bits
            }
            Err(bits) => {
                // Clean up on failure
                for &part_bits in &parts {
                    rt_dec_ref(part_bits);
                }
                bits
            }
        }
    })
}

// ─── Tcl parsing intrinsics ──────────────────────────────────────────────────

/// Split a Tcl dict response string into key-value pairs.
///
/// The input `tcl_str` is a whitespace-delimited string of alternating keys
/// and values (the standard Tcl dict format). If `cut_minus` is truthy,
/// leading "-" is stripped from keys.
///
/// Returns a Molt list of [key, value] pairs (each pair is a 2-element list).
/// Raises RuntimeError if the element count is odd.
#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_splitdict(tcl_str_bits: u64, cut_minus_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(tcl_str) = bits_to_string(tcl_str_bits) else {
            return rt_raise_str("TypeError", "splitdict requires a string argument");
        };

        // Determine cut_minus flag
        let cut_minus = if bits_is_none(cut_minus_bits) {
            true // default
        } else if let Some(b) = obj_from_bits(cut_minus_bits).as_bool() {
            b
        } else if let Some(i) = bits_as_i64(cut_minus_bits) {
            i != 0
        } else {
            true
        };

        // Split by whitespace into tokens, handling Tcl brace quoting
        let items = split_tcl_list(&tcl_str);

        if !items.len().is_multiple_of(2) {
            return rt_raise_str("RuntimeError", "Tcl list representing a dict is expected to contain an even number of elements");
        }

        // Build list of [key, value] pairs
        let mut pairs: Vec<u64> = Vec::with_capacity(items.len() / 2);
        let mut i = 0;
        while i + 1 < items.len() {
            let mut key = items[i].clone();
            if cut_minus && key.starts_with('-') {
                key = key[1..].to_string();
            }
            let value = &items[i + 1];

            let key_bits = match alloc_str_bits(&key) {
                Ok(bits) => bits,
                Err(bits) => {
                    cleanup_list(&pairs);
                    return bits;
                }
            };
            let value_bits = match alloc_str_bits(value) {
                Ok(bits) => bits,
                Err(bits) => {
                    rt_dec_ref(key_bits);
                    cleanup_list(&pairs);
                    return bits;
                }
            };

            // Build a 2-element list [key, value]
            let pair_elems = [key_bits, value_bits];
            let pair_bits = match alloc_list_bits(&pair_elems) {
                Ok(bits) => bits,
                Err(bits) => {
                    rt_dec_ref(key_bits);
                    rt_dec_ref(value_bits);
                    cleanup_list(&pairs);
                    return bits;
                }
            };
            // alloc_list already inc-ref'd key and value; release our local refs
            rt_dec_ref(key_bits);
            rt_dec_ref(value_bits);

            pairs.push(pair_bits);
            i += 2;
        }

        // Build the outer list of pairs
        match alloc_list_bits(&pairs) {
            Ok(list_bits) => {
                for &pair_bits in &pairs {
                    rt_dec_ref(pair_bits);
                }
                list_bits
            }
            Err(bits) => {
                cleanup_list(&pairs);
                bits
            }
        }
    })
}

/// Parse a Tcl list string, handling brace quoting and backslash escapes.
/// This is a simplified parser sufficient for Tcl dict responses.
fn split_tcl_list(input: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut chars = input.chars().peekable();

    loop {
        // Skip whitespace
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }

        let ch = *chars.peek().unwrap();
        if ch == '{' {
            // Brace-quoted word
            chars.next(); // consume '{'
            let mut depth = 1;
            let mut word = String::new();
            while let Some(&c) = chars.peek() {
                chars.next();
                if c == '{' {
                    depth += 1;
                    word.push(c);
                } else if c == '}' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    word.push(c);
                } else {
                    word.push(c);
                }
            }
            items.push(word);
        } else if ch == '"' {
            // Double-quoted word
            chars.next(); // consume '"'
            let mut word = String::new();
            while let Some(&c) = chars.peek() {
                chars.next();
                if c == '"' {
                    break;
                } else if c == '\\' {
                    if let Some(&next) = chars.peek() {
                        chars.next();
                        match next {
                            'n' => word.push('\n'),
                            't' => word.push('\t'),
                            '\\' => word.push('\\'),
                            '"' => word.push('"'),
                            other => {
                                word.push('\\');
                                word.push(other);
                            }
                        }
                    } else {
                        word.push('\\');
                    }
                } else {
                    word.push(c);
                }
            }
            items.push(word);
        } else {
            // Bare word — read until whitespace
            let mut word = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                chars.next();
                if c == '\\' {
                    if let Some(&next) = chars.peek() {
                        chars.next();
                        match next {
                            'n' => word.push('\n'),
                            't' => word.push('\t'),
                            '\\' => word.push('\\'),
                            other => {
                                word.push('\\');
                                word.push(other);
                            }
                        }
                    } else {
                        word.push('\\');
                    }
                } else {
                    word.push(c);
                }
            }
            items.push(word);
        }
    }

    items
}

/// Dec-ref all bits in a vec (cleanup helper for error paths).
fn cleanup_list(items: &[u64]) {
    for &bits in items {
        rt_dec_ref(bits);
    }
}

/// Flatten nested list/tuple args into a flat list for Tcl command building.
///
/// Recursively descends into list/tuple elements, collecting non-None leaf
/// values into a flat list. None values are skipped.
#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_flatten_args(args_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let mut out: Vec<u64> = Vec::new();
        flatten_recursive(args_bits, &mut out);
        if rt_exception_pending() {
            return MoltObject::none().bits();
        }
        match alloc_list_bits(&out) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

/// Recursive helper for flatten_args.
fn flatten_recursive(bits: u64, out: &mut Vec<u64>) {
    if bits_is_none(bits) {
        return;
    }
    if let Some(elems) = read_seq_elements(bits) {
        for elem_bits in elems {
            flatten_recursive(elem_bits, out);
        }
    } else {
        // Leaf value — include as-is
        out.push(bits);
    }
}

/// Merge config dict and keyword dict, filter None values, return flat list
/// of "-key", "value" pairs for Tcl command building.
///
/// `cnf` is either a dict (NaN-boxed pointer to dict) or None.
/// `kw` is either a dict or None.
///
/// For each key-value pair in the merged dict (cnf first, then kw):
///   - Skip pairs where value is None
///   - Normalize key: ensure leading "-", convert underscores to nothing
///   - Append "-key" and value to output list
///
/// Returns a Molt list of alternating "-key", value elements.
#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_cnfmerge(cnf_bits: u64, kw_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let mut out: Vec<u64> = Vec::new();

        // Process cnf dict
        if !bits_is_none(cnf_bits)
            && let Some(ptr) = obj_from_bits(cnf_bits).as_ptr()
        {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_DICT {
                let order_snapshot = dict_order(ptr);
                let mut i = 0;
                while i + 1 < order_snapshot.len() {
                    let key_bits = order_snapshot[i];
                    let value_bits = order_snapshot[i + 1];
                    if !bits_is_none(value_bits)
                        && let Err(bits) = emit_option_pair(key_bits, value_bits, &mut out)
                    {
                        cleanup_list(&out);
                        return bits;
                    }
                    i += 2;
                }
            }
        }

        // Process kw dict
        if !bits_is_none(kw_bits)
            && let Some(ptr) = obj_from_bits(kw_bits).as_ptr()
        {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_DICT {
                let order_snapshot = dict_order(ptr);
                let mut i = 0;
                while i + 1 < order_snapshot.len() {
                    let key_bits = order_snapshot[i];
                    let value_bits = order_snapshot[i + 1];
                    if !bits_is_none(value_bits)
                        && let Err(bits) = emit_option_pair(key_bits, value_bits, &mut out)
                    {
                        cleanup_list(&out);
                        return bits;
                    }
                    i += 2;
                }
            }
        }

        match alloc_list_bits(&out) {
            Ok(list_bits) => {
                // alloc_list inc-refs each element; release our local string refs
                for &elem_bits in &out {
                    rt_dec_ref(elem_bits);
                }
                list_bits
            }
            Err(bits) => {
                cleanup_list(&out);
                bits
            }
        }
    })
}

/// Emit a normalized "-key", value pair into the output vec.
/// The key is converted to string, normalized with leading "-", and
/// underscores in the key name are preserved (matching CPython tkinter behavior).
fn emit_option_pair(
    key_bits: u64,
    value_bits: u64,
    out: &mut Vec<u64>,
) -> Result<(), u64> {
    let key_str = bits_to_string(key_bits).unwrap_or_else(|| {
        // If key is not a string, try int/float rendering
        if let Some(i) = bits_as_i64(key_bits) {
            i.to_string()
        } else if let Some(f) = bits_as_f64(key_bits) {
            f.to_string()
        } else {
            String::from("?")
        }
    });

    let normalized = normalize_option_str(&key_str);
    let key_str_bits = alloc_str_bits(&normalized)?;
    out.push(key_str_bits);
    // Push the value as-is (could be string, int, etc.)
    out.push(value_bits);
    // Inc-ref value since we're going to pass it to alloc_list later which
    // will also inc-ref it, but we dec-ref everything in out after alloc.
    // Actually, the caller pattern is: alloc_list(out) which inc-refs,
    // then we dec-ref out elements. For the key_str_bits, that's correct.
    // For value_bits we need to inc-ref here so the dec-ref after alloc_list
    // doesn't drop the caller's reference.
    rt_inc_ref(value_bits);
    Ok(())
}

/// Normalize a tkinter option name: ensure it has a leading "-".
fn normalize_option_str(name: &str) -> String {
    if name.starts_with('-') {
        name.to_string()
    } else {
        format!("-{name}")
    }
}

/// Normalize a tkinter option name: ensure leading "-", convert underscores.
///
/// Input is a Molt string. Returns a new Molt string with:
///   - Leading "-" prepended if missing
///   - (Underscores preserved — tkinter convention)
#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_normalize_option(name_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(name) = bits_to_string(name_bits) else {
            // Not a string — return as-is
            return name_bits;
        };
        let normalized = normalize_option_str(&name);
        // If already normalized, return the original to avoid allocation
        if normalized == name {
            return name_bits;
        }
        match alloc_str_bits(&normalized) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

// ─── Color parsing ───────────────────────────────────────────────────────────

/// Parse a "#RRGGBB" or "#RGB" hex color string into a (R, G, B) tuple of ints.
///
/// Supports:
///   - "#RGB"       — each component 0-15, scaled to 0-255 (R*17, G*17, B*17)
///   - "#RRGGBB"    — each component 0-255
///   - "#RRRGGGBBB" — each component 0-4095, scaled to 0-255 (component >> 4)
///   - "#RRRRGGGGBBBB" — each component 0-65535, scaled to 0-255 (component >> 8)
///
/// Returns None on failure (not a valid hex color string).
#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_hex_to_rgb(color_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(color) = bits_to_string(color_bits) else {
            return MoltObject::none().bits();
        };
        let trimmed = color.trim();
        if !trimmed.starts_with('#') {
            return MoltObject::none().bits();
        }
        let hex = &trimmed[1..];

        // All characters must be hex digits
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return MoltObject::none().bits();
        }

        let (r, g, b) = match hex.len() {
            3 => {
                // #RGB — 4-bit per channel
                let r = u8::from_str_radix(&hex[0..1], 16).unwrap_or(0);
                let g = u8::from_str_radix(&hex[1..2], 16).unwrap_or(0);
                let b = u8::from_str_radix(&hex[2..3], 16).unwrap_or(0);
                // Scale 0-15 to 0-255
                (i64::from(r * 17), i64::from(g * 17), i64::from(b * 17))
            }
            6 => {
                // #RRGGBB — 8-bit per channel
                let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
                let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
                let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
                (i64::from(r), i64::from(g), i64::from(b))
            }
            9 => {
                // #RRRGGGBBB — 12-bit per channel, scale to 8-bit
                let r = u16::from_str_radix(&hex[0..3], 16).unwrap_or(0);
                let g = u16::from_str_radix(&hex[3..6], 16).unwrap_or(0);
                let b = u16::from_str_radix(&hex[6..9], 16).unwrap_or(0);
                (i64::from(r >> 4), i64::from(g >> 4), i64::from(b >> 4))
            }
            12 => {
                // #RRRRGGGGBBBB — 16-bit per channel, scale to 8-bit
                let r = u16::from_str_radix(&hex[0..4], 16).unwrap_or(0);
                let g = u16::from_str_radix(&hex[4..8], 16).unwrap_or(0);
                let b = u16::from_str_radix(&hex[8..12], 16).unwrap_or(0);
                (i64::from(r >> 8), i64::from(g >> 8), i64::from(b >> 8))
            }
            _ => {
                return MoltObject::none().bits();
            }
        };

        let elems = [
            MoltObject::from_int(r).bits(),
            MoltObject::from_int(g).bits(),
            MoltObject::from_int(b).bits(),
        ];
        match alloc_tuple_result(&elems) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

// ─── Delay/value parsing ─────────────────────────────────────────────────────

/// Convert a delay value to integer milliseconds.
///
/// Accepts:
///   - int: returned as-is
///   - float: truncated to int
///   - string of digits: parsed as int
///
/// Returns None if the value cannot be interpreted as a delay.
#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_normalize_delay_ms(delay_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        // Already an integer?
        if let Some(i) = bits_as_i64(delay_bits) {
            return MoltObject::from_int(i).bits();
        }
        // Float?
        if let Some(f) = bits_as_f64(delay_bits)
            && f.is_finite()
        {
            return MoltObject::from_int(f as i64).bits();
        }
        // String of digits?
        if let Some(text) = bits_to_string(delay_bits) {
            let trimmed = text.trim();
            if !trimmed.is_empty()
                && trimmed.chars().all(|c| c.is_ascii_digit())
                && let Ok(i) = trimmed.parse::<i64>()
            {
                return MoltObject::from_int(i).bits();
            }
            // Also handle negative digit strings
            if trimmed.starts_with('-')
                && trimmed.len() > 1
                && trimmed[1..].chars().all(|c| c.is_ascii_digit())
                && let Ok(i) = trimmed.parse::<i64>()
            {
                return MoltObject::from_int(i).bits();
            }
        }
        // Cannot convert
        MoltObject::none().bits()
    })
}

/// Convert a Tcl string value to the best Python type.
///
/// Conversion priority:
///   1. Try int (with optional sign)
///   2. Try float (if the result is finite)
///   3. Keep as string
///
/// This mirrors the tkinter `_convert_stringval` / `getint`/`getdouble`
/// cascade used when reading widget configuration values from Tcl.
#[unsafe(no_mangle)]
pub extern "C" fn molt_tk_convert_stringval(text_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        // If already int or float, return as-is
        let obj = obj_from_bits(text_bits);
        if obj.is_none() {
            return text_bits;
        }
        if to_i64(obj).is_some() {
            return text_bits;
        }
        if obj.as_float().is_some() {
            return text_bits;
        }

        let Some(text) = string_obj_to_owned(obj) else {
            return text_bits;
        };
        let trimmed = text.trim();

        if trimmed.is_empty() {
            return text_bits;
        }

        // Try integer parsing (with Tcl hex/octal support)
        if let Some(int_val) = try_parse_tcl_int(trimmed) {
            return MoltObject::from_int(int_val).bits();
        }

        // Try float parsing
        if let Ok(float_val) = trimmed.parse::<f64>()
            && float_val.is_finite()
        {
            return MoltObject::from_float(float_val).bits();
        }

        // Keep as original string
        text_bits
    })
}

/// Try to parse a Tcl integer value, supporting:
///   - Decimal: "123", "-456"
///   - Hex: "0x1A", "0X1a"
///   - Octal: "0o17", "0O17"
///   - Binary: "0b1010", "0B1010"
fn try_parse_tcl_int(text: &str) -> Option<i64> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Handle sign
    let (negative, unsigned) = if let Some(rest) = trimmed.strip_prefix('-') {
        (true, rest)
    } else if let Some(rest) = trimmed.strip_prefix('+') {
        (false, rest)
    } else {
        (false, trimmed)
    };

    if unsigned.is_empty() {
        return None;
    }

    let value = if let Some(hex) = unsigned
        .strip_prefix("0x")
        .or_else(|| unsigned.strip_prefix("0X"))
    {
        // Hex
        if hex.is_empty() || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        i64::from_str_radix(hex, 16).ok()?
    } else if let Some(oct) = unsigned
        .strip_prefix("0o")
        .or_else(|| unsigned.strip_prefix("0O"))
    {
        // Octal
        if oct.is_empty() || !oct.chars().all(|c| matches!(c, '0'..='7')) {
            return None;
        }
        i64::from_str_radix(oct, 8).ok()?
    } else if let Some(bin) = unsigned
        .strip_prefix("0b")
        .or_else(|| unsigned.strip_prefix("0B"))
    {
        // Binary
        if bin.is_empty() || !bin.chars().all(|c| c == '0' || c == '1') {
            return None;
        }
        i64::from_str_radix(bin, 2).ok()?
    } else {
        // Decimal
        if !unsigned.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        unsigned.parse::<i64>().ok()?
    };

    Some(if negative { -value } else { value })
}
