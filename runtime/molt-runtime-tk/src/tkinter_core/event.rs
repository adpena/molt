use molt_runtime_core::prelude::*;

use crate::bridge::decode_value_list_bits;

use super::common::{
    alloc_list_bits, alloc_str_bits, bits_as_f64, bits_as_i64, bits_is_empty_or_none,
    bits_to_string,
};

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
    if let Some(i) = bits_as_i64(bits) {
        return MoltObject::from_bool(i != 0).bits();
    }
    if let Some(text) = bits_to_string(bits)
        && let Some(b) = parse_tcl_bool(&text)
    {
        return MoltObject::from_bool(b).bits();
    }
    if let Some(f) = bits_as_f64(bits) {
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
        let Some(elems) = decode_value_list_bits(args_bits) else {
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
    molt_runtime_core::with_gil_entry!(_py, event_int_convert(value_bits))
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
