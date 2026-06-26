use molt_runtime_core::prelude::*;

use super::common::{alloc_tuple_result, bits_to_string};

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
