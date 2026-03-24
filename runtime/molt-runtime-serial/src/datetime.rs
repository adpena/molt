#![allow(dead_code, unused_imports)]
//! Rust intrinsics for the Python `datetime` module.
//!
//! All functions are C-callable via `#[unsafe(no_mangle)]` and accept/return
//! NaN-boxed `u64` values following the standard Molt object model.
//!
//! Calendar math uses Howard Hinnant's civil_from_days / days_from_civil
//! algorithms.  Time and hash functions are pure Rust, with
//! `cfg(target_arch = "wasm32")` stubs for platform-specific syscalls.

use std::fmt::Write as _;

use molt_runtime_core::prelude::*;
use crate::bridge::*;

// ---------------------------------------------------------------------------
// Calendar helper types
// ---------------------------------------------------------------------------

/// Proleptic Gregorian ordinal where day 1 = 0001-01-01.
type Ordinal = i64;

/// Parsed datetime components:
/// (year, month, day, hour, minute, second, microsecond, utc_offset_seconds).
type DateTimeParts = (i32, i32, i32, i32, i32, i32, i32, Option<i64>);

/// Local time components: (year, month, day, hour, minute, second, utc_offset_secs).
type LocalTimeParts = (i32, i32, i32, i32, i32, i32, i64);

/// Datetime fields passed to `strftime_impl`.
struct StrftimeArgs<'a> {
    y: i32,
    m: i32,
    d: i32,
    h: i32,
    mi: i32,
    sec: i32,
    us: i32,
    utcoff: Option<i64>,
    fmt: &'a str,
}

// ---------------------------------------------------------------------------
// Howard Hinnant civil/days algorithms
// ---------------------------------------------------------------------------

/// Convert a proleptic Gregorian *rata die* (days since 0001-01-01, 1-based
/// ordinal) to (year, month, day).  Accepts the Python convention where ordinal
/// 1 → 0001-01-01.
fn ordinal_to_ymd(ordinal: i64) -> (i32, i32, i32) {
    // This formulation uses Hinnant's March-based day index where
    // 0001-01-01 corresponds to day 306. CPython ordinals are 1-based, so the
    // conversion is a fixed +305 shift.
    let z = ordinal + 305;
    let era: i64 = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let mut y = (yoe + era * 400) as i32;
    let doy = (doe - (365 * yoe + yoe / 4 - yoe / 100)) as i32; // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = mp + if mp < 10 { 3 } else { -9 }; // [1, 12]
    if m <= 2 {
        y += 1;
    }
    (y, m, d)
}

/// Convert (year, month, day) to a proleptic Gregorian ordinal (1 = 0001-01-01).
fn ymd_to_ordinal(y: i32, m: i32, d: i32) -> i64 {
    let y = if m <= 2 { y as i64 - 1 } else { y as i64 };
    let m = m as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d as i64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 305
}

fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

/// Days in month m of year y (1-indexed month, 1-indexed year).
fn days_in_month_impl(year: i32, month: i32) -> i32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// Mon=0 … Sun=6 weekday from (y, m, d).
fn weekday_impl(y: i32, m: i32, d: i32) -> i32 {
    // Ordinal day 0001-01-01 is a Monday (weekday 0).
    let ord = ymd_to_ordinal(y, m, d);
    ((ord - 1).rem_euclid(7)) as i32
}

/// Day-of-year (1-based) for (y, m, d).
fn day_of_year(y: i32, m: i32, d: i32) -> i32 {
    const DAYS_BEFORE: [i32; 13] = [0, 0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let leap = if is_leap(y) && m > 2 { 1 } else { 0 };
    DAYS_BEFORE[m.clamp(1, 12) as usize] + d + leap
}

/// ISO (year, week, weekday) where weekday Mon=1 … Sun=7.
fn isocalendar_impl(y: i32, m: i32, d: i32) -> (i32, i32, i32) {
    let ord = ymd_to_ordinal(y, m, d);
    // ISO week 1 contains the first Thursday of the year.
    // Thu = weekday 3 (Mon=0).
    let wday = ((ord - 1).rem_euclid(7)) as i32; // Mon=0
    let iso_wday = wday + 1; // Mon=1..Sun=7
    // week number: days from nearest Monday
    let week = ((ord - 1 - wday as i64 + 10) / 7) as i32;
    if week < 1 {
        // belongs to last week of previous year
        let iso_y = y - 1;
        let iso_w = iso_weeks_in_year(iso_y);
        return (iso_y, iso_w, iso_wday);
    }
    let max_week = iso_weeks_in_year(y);
    if week > max_week {
        return (y + 1, 1, iso_wday);
    }
    (y, week, iso_wday)
}

fn iso_weeks_in_year(y: i32) -> i32 {
    // A year has 53 ISO weeks if Jan 1 or Dec 31 is Thursday.
    let jan1_wday = weekday_impl(y, 1, 1); // Mon=0..Sun=6
    if jan1_wday == 3 {
        return 53;
    }
    if is_leap(y) && jan1_wday == 2 {
        return 53;
    }
    52
}

// ---------------------------------------------------------------------------
// Helper: unpack i64 from bits (MoltObject int)
// ---------------------------------------------------------------------------

#[inline]
fn unpack_i64(_py: &PyToken, bits: u64, label: &str) -> Result<i64, u64> {
    let obj = obj_from_bits(bits);
    if let Some(i) = to_i64(obj) {
        return Ok(i);
    }
    if let Some(f) = obj.as_float() {
        return Ok(f as i64);
    }
    let msg = format!("datetime: expected int for {label}");
    Err(raise_exception::<u64>(_py, "TypeError", &msg))
}

#[inline]
fn unpack_f64(_py: &PyToken, bits: u64, label: &str) -> Result<f64, u64> {
    let obj = obj_from_bits(bits);
    if let Some(f) = to_f64(obj) {
        return Ok(f);
    }
    if let Some(i) = to_i64(obj) {
        return Ok(i as f64);
    }
    let msg = format!("datetime: expected number for {label}");
    Err(raise_exception::<u64>(_py, "TypeError", &msg))
}

// ---------------------------------------------------------------------------
// Helper: alloc tuple returning u64 bits
// ---------------------------------------------------------------------------

fn tuple_bits(_py: &PyToken, elems: &[u64]) -> u64 {
    let ptr = alloc_tuple(_py, elems);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

fn string_bits(_py: &PyToken, s: &str) -> u64 {
    let ptr = alloc_string(_py, s.as_bytes());
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

// ---------------------------------------------------------------------------
// Macro: unpack a MoltObject string to an owned String
// ---------------------------------------------------------------------------

macro_rules! unpack_str {
    ($py:expr, $bits:expr, $label:expr) => {{
        let _obj = obj_from_bits($bits);
        match string_obj_to_owned(_obj) {
            Some(s) => s,
            None => {
                let msg = format!("datetime: expected str for {}", $label);
                return raise_exception::<u64>($py, "TypeError", &msg);
            }
        }
    }};
}

// ===========================================================================
// 1. Calendar math
// ===========================================================================

#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_days_in_month(year_bits: u64, month_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let year = match unpack_i64(_py, year_bits, "year") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };
        let month = match unpack_i64(_py, month_bits, "month") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };
        if !(1..=12).contains(&month) {
            let msg = format!("month must be in 1..12, not {month}");
            return raise_exception::<u64>(_py, "ValueError", &msg);
        }
        MoltObject::from_int(days_in_month_impl(year, month) as i64).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_is_leap(year_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let year = match unpack_i64(_py, year_bits, "year") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };
        MoltObject::from_bool(is_leap(year)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_ymd_to_ordinal(
    year_bits: u64,
    month_bits: u64,
    day_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let y = match unpack_i64(_py, year_bits, "year") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };
        let m = match unpack_i64(_py, month_bits, "month") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };
        let d = match unpack_i64(_py, day_bits, "day") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };
        MoltObject::from_int(ymd_to_ordinal(y, m, d)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_ordinal_to_ymd(ordinal_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ordinal = match unpack_i64(_py, ordinal_bits, "ordinal") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let (y, m, d) = ordinal_to_ymd(ordinal);
        let elems = [
            MoltObject::from_int(y as i64).bits(),
            MoltObject::from_int(m as i64).bits(),
            MoltObject::from_int(d as i64).bits(),
        ];
        tuple_bits(_py, &elems)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_weekday(year_bits: u64, month_bits: u64, day_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let y = match unpack_i64(_py, year_bits, "year") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };
        let m = match unpack_i64(_py, month_bits, "month") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };
        let d = match unpack_i64(_py, day_bits, "day") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };
        MoltObject::from_int(weekday_impl(y, m, d) as i64).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_isoweekday(year_bits: u64, month_bits: u64, day_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let y = match unpack_i64(_py, year_bits, "year") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };
        let m = match unpack_i64(_py, month_bits, "month") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };
        let d = match unpack_i64(_py, day_bits, "day") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };
        // Mon=1..Sun=7
        MoltObject::from_int((weekday_impl(y, m, d) + 1) as i64).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_isocalendar(year_bits: u64, month_bits: u64, day_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let y = match unpack_i64(_py, year_bits, "year") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };
        let m = match unpack_i64(_py, month_bits, "month") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };
        let d = match unpack_i64(_py, day_bits, "day") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };
        let (iso_y, iso_w, iso_wd) = isocalendar_impl(y, m, d);
        let elems = [
            MoltObject::from_int(iso_y as i64).bits(),
            MoltObject::from_int(iso_w as i64).bits(),
            MoltObject::from_int(iso_wd as i64).bits(),
        ];
        tuple_bits(_py, &elems)
    })
}

// ===========================================================================
// 2. timedelta
// ===========================================================================

/// Normalize 7 timedelta components to canonical (days, seconds, microseconds).
/// All inputs are i64 Molt integers.
#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_td_normalize(
    days_bits: u64,
    seconds_bits: u64,
    us_bits: u64,
    ms_bits: u64,
    minutes_bits: u64,
    hours_bits: u64,
    weeks_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let days = match unpack_f64(_py, days_bits, "days") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let secs = match unpack_f64(_py, seconds_bits, "seconds") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let us = match unpack_f64(_py, us_bits, "microseconds") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let ms = match unpack_f64(_py, ms_bits, "milliseconds") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let minutes = match unpack_f64(_py, minutes_bits, "minutes") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let hours = match unpack_f64(_py, hours_bits, "hours") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let weeks = match unpack_f64(_py, weeks_bits, "weeks") {
            Ok(v) => v,
            Err(e) => return e,
        };

        // Match CPython timedelta construction by accepting ints/floats and
        // rounding the aggregate duration to the nearest microsecond.
        let total_us_f = us
            + ms * 1_000.0
            + secs * 1_000_000.0
            + minutes * 60.0 * 1_000_000.0
            + hours * 3_600.0 * 1_000_000.0
            + (days + weeks * 7.0) * 86_400.0 * 1_000_000.0;
        let mut total_us = total_us_f.round() as i128;

        let us_per_day: i128 = 86_400 * 1_000_000;
        let us_per_sec: i128 = 1_000_000;

        let out_days = total_us.div_euclid(us_per_day);
        total_us = total_us.rem_euclid(us_per_day);
        let out_secs = total_us.div_euclid(us_per_sec);
        let out_us = total_us.rem_euclid(us_per_sec);

        // Check CPython bounds: timedelta.max.days = 999999999
        if !(-999_999_999..=999_999_999).contains(&out_days) {
            return raise_exception::<u64>(
                _py,
                "OverflowError",
                "timedelta # of days is too large: timedelta.max = timedelta(999999999, 86399, 999999)",
            );
        }

        let elems = [
            MoltObject::from_int(out_days as i64).bits(),
            MoltObject::from_int(out_secs as i64).bits(),
            MoltObject::from_int(out_us as i64).bits(),
        ];
        tuple_bits(_py, &elems)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_td_total_seconds(
    days_bits: u64,
    seconds_bits: u64,
    us_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let days = match unpack_i64(_py, days_bits, "days") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let secs = match unpack_i64(_py, seconds_bits, "seconds") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let us = match unpack_i64(_py, us_bits, "microseconds") {
            Ok(v) => v,
            Err(e) => return e,
        };
        // CPython: days * 86400 + seconds + microseconds / 1e6
        let total = days as f64 * 86_400.0 + secs as f64 + us as f64 / 1_000_000.0;
        MoltObject::from_float(total).bits()
    })
}

// ===========================================================================
// 3. ISO 8601 Parsing
// ===========================================================================

/// Parse "YYYY-MM-DD" → (y, m, d).
fn parse_isodate_str(s: &str) -> Result<(i32, i32, i32), String> {
    let b = s.as_bytes();
    // Minimum: YYYYMMDD (8 chars) or YYYY-MM-DD (10 chars)
    if b.len() < 8 {
        return Err(format!("Invalid isoformat string: {s:?}"));
    }
    // Try YYYY-MM-DD
    if b.len() >= 10 && b[4] == b'-' && b[7] == b'-' {
        let y = parse_digits_n(b, 0, 4)?;
        let m = parse_digits_n(b, 5, 2)?;
        let d = parse_digits_n(b, 8, 2)?;
        return Ok((y, m, d));
    }
    // Try YYYYMMDD
    if b.len() >= 8 {
        let y = parse_digits_n(b, 0, 4)?;
        let m = parse_digits_n(b, 4, 2)?;
        let d = parse_digits_n(b, 6, 2)?;
        return Ok((y, m, d));
    }
    Err(format!("Invalid isoformat string: {s:?}"))
}

/// Parse digits at byte offset `off` of length `len`.
fn parse_digits_n(b: &[u8], off: usize, len: usize) -> Result<i32, String> {
    if off + len > b.len() {
        return Err("unexpected end of datetime string".to_string());
    }
    let mut acc: i32 = 0;
    for &byte in &b[off..off + len] {
        if !byte.is_ascii_digit() {
            return Err(format!("invalid digit: {}", byte as char));
        }
        acc = acc * 10 + (byte - b'0') as i32;
    }
    Ok(acc)
}

/// Parse a UTC offset substring like "+HH:MM", "-HH:MM", "+HHMM", "-HHMM",
/// "+HH", "-HH", or "Z".  Returns total offset in seconds.
fn parse_utc_offset(b: &[u8]) -> Result<i64, String> {
    if b.is_empty() {
        return Err("empty utcoffset".to_string());
    }
    if b == b"Z" {
        return Ok(0);
    }
    let sign: i64 = match b[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return Err(format!("invalid utcoffset sign: {}", b[0] as char)),
    };
    let rest = &b[1..];
    // rest is HH:MM[:SS[.ffffff]], HHMM, or HH
    let (h, m, s) = match rest.len() {
        2 => {
            let h = parse_digits_n(rest, 0, 2)? as i64;
            (h, 0i64, 0i64)
        }
        4 => {
            let h = parse_digits_n(rest, 0, 2)? as i64;
            let m = parse_digits_n(rest, 2, 2)? as i64;
            (h, m, 0i64)
        }
        5 if rest[2] == b':' => {
            let h = parse_digits_n(rest, 0, 2)? as i64;
            let m = parse_digits_n(rest, 3, 2)? as i64;
            (h, m, 0i64)
        }
        6 => {
            let h = parse_digits_n(rest, 0, 2)? as i64;
            let m = parse_digits_n(rest, 2, 2)? as i64;
            let sec = parse_digits_n(rest, 4, 2)? as i64;
            (h, m, sec)
        }
        8 if rest[2] == b':' && rest[5] == b':' => {
            let h = parse_digits_n(rest, 0, 2)? as i64;
            let m = parse_digits_n(rest, 3, 2)? as i64;
            let sec = parse_digits_n(rest, 6, 2)? as i64;
            (h, m, sec)
        }
        _ => {
            // Try HH:MM with possible seconds
            if rest.len() >= 5 && rest[2] == b':' {
                let h = parse_digits_n(rest, 0, 2)? as i64;
                let m = parse_digits_n(rest, 3, 2)? as i64;
                (h, m, 0i64)
            } else {
                return Err(format!("invalid utcoffset length: {}", rest.len()));
            }
        }
    };
    Ok(sign * (h * 3_600 + m * 60 + s))
}

/// Parse a time string of the form HH[:MM[:SS[.ffffff]]]+[utcoffset|Z]
/// Returns (h, min, s, us, utcoff_secs or i64::MIN if naive).
fn parse_isotime_str(s: &str) -> Result<(i32, i32, i32, i32, i64), String> {
    let b = s.as_bytes();
    if b.len() < 2 {
        return Err(format!("Invalid time string: {s:?}"));
    }
    let h = parse_digits_n(b, 0, 2)? as i32;
    let mut pos = 2usize;
    let (mi, sec, us) = if pos < b.len() && (b[pos] == b':' || b[pos].is_ascii_digit()) {
        let has_colon = b[pos] == b':';
        if has_colon {
            pos += 1;
        }
        if pos + 2 > b.len() {
            return Err(format!("Invalid time string: {s:?}"));
        }
        let mi = parse_digits_n(b, pos, 2)? as i32;
        pos += 2;
        let (sec, us) =
            if pos < b.len() && (b[pos] == b':' || (!has_colon && b[pos].is_ascii_digit())) {
                if b[pos] == b':' {
                    pos += 1;
                }
                if pos + 2 > b.len() {
                    return Err(format!("Invalid time string: {s:?}"));
                }
                let sec = parse_digits_n(b, pos, 2)? as i32;
                pos += 2;
                let us = if pos < b.len() && b[pos] == b'.' {
                    pos += 1;
                    let start = pos;
                    while pos < b.len() && b[pos].is_ascii_digit() {
                        pos += 1;
                    }
                    let frac_len = pos - start;
                    let mut us_acc: i32 = 0;
                    let frac_bytes = &b[start..start + frac_len.min(6)];
                    for (i, &byte) in frac_bytes.iter().enumerate() {
                        let pow = 10i32.pow((5 - i as u32).min(5));
                        us_acc += (byte - b'0') as i32 * pow;
                    }
                    // Pad if < 6 digits
                    if frac_len < 6 {
                        // already handled above by pow
                    }
                    us_acc
                } else {
                    0
                };
                (sec, us)
            } else {
                (0, 0)
            };
        (mi, sec, us)
    } else {
        (0, 0, 0)
    };

    // Parse optional UTC offset
    let utcoff = if pos < b.len() {
        let off_bytes = &b[pos..];
        parse_utc_offset(off_bytes)?
    } else {
        i64::MIN // sentinel: naive
    };

    Ok((h, mi, sec, us, utcoff))
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_parse_isoformat_date(text_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let s = unpack_str!(_py, text_bits, "date string");
        match parse_isodate_str(&s) {
            Ok((y, m, d)) => {
                let elems = [
                    MoltObject::from_int(y as i64).bits(),
                    MoltObject::from_int(m as i64).bits(),
                    MoltObject::from_int(d as i64).bits(),
                ];
                tuple_bits(_py, &elems)
            }
            Err(msg) => raise_exception::<u64>(_py, "ValueError", &msg),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_parse_isoformat_time(text_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let s = unpack_str!(_py, text_bits, "time string");
        match parse_isotime_str(&s) {
            Ok((h, mi, sec, us, utcoff)) => {
                let utcoff_bits = if utcoff == i64::MIN {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_int(utcoff).bits()
                };
                let elems = [
                    MoltObject::from_int(h as i64).bits(),
                    MoltObject::from_int(mi as i64).bits(),
                    MoltObject::from_int(sec as i64).bits(),
                    MoltObject::from_int(us as i64).bits(),
                    utcoff_bits,
                ];
                // dec_ref the None placeholder only if it was a ptr (it isn't, None is inline)
                tuple_bits(_py, &elems)
            }
            Err(msg) => raise_exception::<u64>(_py, "ValueError", &msg),
        }
    })
}

/// Parse a full ISO 8601 datetime string: YYYY-MM-DDTHH:MM:SS[.ffffff][Z|+HH:MM]
/// Returns tuple (y, m, d, h, min, s, us, utcoff_secs_or_none).
#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_parse_isoformat(text_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let s = unpack_str!(_py, text_bits, "datetime string");
        let b = s.as_bytes();

        // Find separator between date and time parts.
        // Accepted separators: T, t, space
        let sep_pos = b.iter().position(|&c| c == b'T' || c == b't' || c == b' ');

        let (date_str, time_str) = if let Some(pos) = sep_pos {
            (
                std::str::from_utf8(&b[..pos]).unwrap_or(""),
                std::str::from_utf8(&b[pos + 1..]).unwrap_or(""),
            )
        } else {
            // Date-only
            (s.as_str(), "")
        };

        let (y, m, d) = match parse_isodate_str(date_str) {
            Ok(v) => v,
            Err(msg) => return raise_exception::<u64>(_py, "ValueError", &msg),
        };

        let (h, mi, sec, us, utcoff) = if time_str.is_empty() {
            (0, 0, 0, 0, i64::MIN)
        } else {
            match parse_isotime_str(time_str) {
                Ok(v) => v,
                Err(msg) => return raise_exception::<u64>(_py, "ValueError", &msg),
            }
        };

        let utcoff_bits = if utcoff == i64::MIN {
            MoltObject::none().bits()
        } else {
            MoltObject::from_int(utcoff).bits()
        };

        let elems = [
            MoltObject::from_int(y as i64).bits(),
            MoltObject::from_int(m as i64).bits(),
            MoltObject::from_int(d as i64).bits(),
            MoltObject::from_int(h as i64).bits(),
            MoltObject::from_int(mi as i64).bits(),
            MoltObject::from_int(sec as i64).bits(),
            MoltObject::from_int(us as i64).bits(),
            utcoff_bits,
        ];
        tuple_bits(_py, &elems)
    })
}

/// strptime: parse text with format string.  Supported directives:
/// %Y %m %d %H %M %S %f %z %Z %% and literal characters.
/// Returns tuple (y, m, d, h, min, s, us).
#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_strptime(text_bits: u64, fmt_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let text = unpack_str!(_py, text_bits, "text");
        let fmt = unpack_str!(_py, fmt_bits, "format");

        match strptime_impl(&text, &fmt) {
            Ok((y, m, d, h, mi, s, us, utc_offset_secs)) => {
                let elems = [
                    MoltObject::from_int(y as i64).bits(),
                    MoltObject::from_int(m as i64).bits(),
                    MoltObject::from_int(d as i64).bits(),
                    MoltObject::from_int(h as i64).bits(),
                    MoltObject::from_int(mi as i64).bits(),
                    MoltObject::from_int(s as i64).bits(),
                    MoltObject::from_int(us as i64).bits(),
                    utc_offset_secs
                        .map(MoltObject::from_int)
                        .unwrap_or_else(MoltObject::none)
                        .bits(),
                ];
                tuple_bits(_py, &elems)
            }
            Err(msg) => raise_exception::<u64>(_py, "ValueError", &msg),
        }
    })
}

fn strptime_impl(text: &str, fmt: &str) -> Result<DateTimeParts, String> {
    let tb = text.as_bytes();
    let fb = fmt.as_bytes();
    let mut ti = 0usize;
    let mut fi = 0usize;

    let mut year: i32 = 1900;
    let mut month: i32 = 1;
    let mut day: i32 = 1;
    let mut hour: i32 = 0;
    let mut minute: i32 = 0;
    let mut second: i32 = 0;
    let mut us: i32 = 0;
    let mut utc_offset_secs: Option<i64> = None;

    while fi < fb.len() {
        if fb[fi] != b'%' {
            if ti >= tb.len() || tb[ti] != fb[fi] {
                return Err(format!(
                    "time data {:?} does not match format {:?}",
                    text, fmt
                ));
            }
            ti += 1;
            fi += 1;
            continue;
        }
        fi += 1;
        if fi >= fb.len() {
            return Err("trailing % in format".to_string());
        }
        let spec = fb[fi];
        fi += 1;

        match spec {
            b'%' => {
                if ti >= tb.len() || tb[ti] != b'%' {
                    return Err(format!(
                        "time data {:?} does not match format {:?}",
                        text, fmt
                    ));
                }
                ti += 1;
            }
            b'Y' => {
                year = read_decimal(tb, &mut ti, 4, "year")?;
            }
            b'm' => {
                month = read_decimal(tb, &mut ti, 2, "month")?;
            }
            b'd' => {
                day = read_decimal(tb, &mut ti, 2, "day")?;
            }
            b'H' => {
                hour = read_decimal(tb, &mut ti, 2, "hour")?;
            }
            b'M' => {
                minute = read_decimal(tb, &mut ti, 2, "minute")?;
            }
            b'S' => {
                second = read_decimal(tb, &mut ti, 2, "second")?;
            }
            b'f' => {
                // Up to 6 digits, left-aligned (leading digits significant)
                let start = ti;
                while ti < tb.len() && ti - start < 6 && tb[ti].is_ascii_digit() {
                    ti += 1;
                }
                let frac_len = ti - start;
                if frac_len == 0 {
                    return Err("expected microseconds digits after %f".to_string());
                }
                let mut acc: i32 = 0;
                for (i, &byte) in tb[start..ti].iter().enumerate() {
                    let pow = 10i32.pow(5 - i as u32);
                    acc += (byte - b'0') as i32 * pow;
                }
                us = acc;
            }
            b'z' => {
                // Parse UTC offset and preserve it so Python can build an aware datetime.
                if ti < tb.len() && (tb[ti] == b'+' || tb[ti] == b'-' || tb[ti] == b'Z') {
                    let start = ti;
                    ti += 1;
                    while ti < tb.len() && (tb[ti].is_ascii_digit() || tb[ti] == b':') {
                        ti += 1;
                    }
                    utc_offset_secs = Some(parse_utc_offset(&tb[start..ti])?);
                } else {
                    return Err(format!(
                        "time data {:?} does not match format {:?}",
                        text, fmt
                    ));
                }
            }
            b'Z' => {
                // Accept a narrow CPython-compatible subset. Unknown names should
                // fail rather than silently producing a naive datetime.
                let start = ti;
                while ti < tb.len() && (tb[ti].is_ascii_alphabetic() || tb[ti] == b'/') {
                    ti += 1;
                }
                let zone = std::str::from_utf8(&tb[start..ti]).unwrap_or("");
                if zone.eq_ignore_ascii_case("UTC") || zone.eq_ignore_ascii_case("GMT") {
                    utc_offset_secs = Some(0);
                } else {
                    return Err(format!(
                        "time data {:?} does not match format {:?}",
                        text, fmt
                    ));
                }
            }
            b'j' => {
                // Day of year — parse but ignore (we use y/m/d)
                read_decimal(tb, &mut ti, 3, "day-of-year")?;
            }
            b'p' => {
                // AM/PM
                if ti + 2 <= tb.len() {
                    let ampm = &tb[ti..ti + 2];
                    if ampm == b"AM" || ampm == b"am" {
                        ti += 2;
                        if hour == 12 {
                            hour = 0;
                        }
                    } else if ampm == b"PM" || ampm == b"pm" {
                        ti += 2;
                        if hour != 12 {
                            hour += 12;
                        }
                    }
                }
            }
            b'I' => {
                // 12-hour clock
                hour = read_decimal(tb, &mut ti, 2, "12-hour")?;
            }
            b'a' | b'A' => {
                // Abbreviated/full weekday name — skip word
                while ti < tb.len() && tb[ti].is_ascii_alphabetic() {
                    ti += 1;
                }
            }
            b'b' | b'B' | b'h' => {
                // Abbreviated/full month name
                // Try to map it to a month number
                let start = ti;
                while ti < tb.len() && tb[ti].is_ascii_alphabetic() {
                    ti += 1;
                }
                let word = std::str::from_utf8(&tb[start..ti])
                    .unwrap_or("")
                    .to_lowercase();
                month = month_name_to_num(&word)?;
            }
            b'c' => {
                // Locale's date and time: "Thu Jan  1 00:00:00 2000"
                // Parse manually: weekday SP month SP day SP time SP year
                // skip weekday
                while ti < tb.len() && tb[ti].is_ascii_alphabetic() {
                    ti += 1;
                }
                if ti < tb.len() && tb[ti] == b' ' {
                    ti += 1;
                }
                // month name
                let start = ti;
                while ti < tb.len() && tb[ti].is_ascii_alphabetic() {
                    ti += 1;
                }
                let word = std::str::from_utf8(&tb[start..ti])
                    .unwrap_or("")
                    .to_lowercase();
                month = month_name_to_num(&word)?;
                // spaces + day
                while ti < tb.len() && tb[ti] == b' ' {
                    ti += 1;
                }
                day = read_decimal_flexible(tb, &mut ti, "day")?;
                if ti < tb.len() && tb[ti] == b' ' {
                    ti += 1;
                }
                // HH:MM:SS
                hour = read_decimal(tb, &mut ti, 2, "hour")?;
                if ti < tb.len() && tb[ti] == b':' {
                    ti += 1;
                }
                minute = read_decimal(tb, &mut ti, 2, "minute")?;
                if ti < tb.len() && tb[ti] == b':' {
                    ti += 1;
                }
                second = read_decimal(tb, &mut ti, 2, "second")?;
                if ti < tb.len() && tb[ti] == b' ' {
                    ti += 1;
                }
                year = read_decimal(tb, &mut ti, 4, "year")?;
            }
            b'x' => {
                // MM/DD/YY
                month = read_decimal(tb, &mut ti, 2, "month")?;
                if ti < tb.len() && tb[ti] == b'/' {
                    ti += 1;
                }
                day = read_decimal(tb, &mut ti, 2, "day")?;
                if ti < tb.len() && tb[ti] == b'/' {
                    ti += 1;
                }
                let yy = read_decimal(tb, &mut ti, 2, "year")?;
                year = if yy >= 69 { 1900 + yy } else { 2000 + yy };
            }
            b'X' => {
                // HH:MM:SS
                hour = read_decimal(tb, &mut ti, 2, "hour")?;
                if ti < tb.len() && tb[ti] == b':' {
                    ti += 1;
                }
                minute = read_decimal(tb, &mut ti, 2, "minute")?;
                if ti < tb.len() && tb[ti] == b':' {
                    ti += 1;
                }
                second = read_decimal(tb, &mut ti, 2, "second")?;
            }
            b'y' => {
                let yy = read_decimal(tb, &mut ti, 2, "year")?;
                year = if yy >= 69 { 1900 + yy } else { 2000 + yy };
            }
            other => {
                return Err(format!(
                    "unsupported strptime directive: %{}",
                    other as char
                ));
            }
        }
    }

    if ti < tb.len() {
        return Err(format!(
            "unconverted data remains: {:?}",
            std::str::from_utf8(&tb[ti..]).unwrap_or("?")
        ));
    }

    Ok((year, month, day, hour, minute, second, us, utc_offset_secs))
}

/// Read up to `max_digits` decimal digits from `b[*pos..]`, advancing `*pos`.
fn read_decimal(b: &[u8], pos: &mut usize, max_digits: usize, _label: &str) -> Result<i32, String> {
    // Skip leading space (for space-padded values like day-of-month)
    while *pos < b.len() && b[*pos] == b' ' {
        *pos += 1;
    }
    let start = *pos;
    let mut count = 0;
    while *pos < b.len() && count < max_digits && b[*pos].is_ascii_digit() {
        *pos += 1;
        count += 1;
    }
    if count == 0 {
        return Err(format!("expected {max_digits} digit(s) for {_label}"));
    }
    parse_digits_n(b, start, *pos - start)
}

/// Like read_decimal but accepts 1 or 2 digits (for space-padded day).
fn read_decimal_flexible(b: &[u8], pos: &mut usize, label: &str) -> Result<i32, String> {
    while *pos < b.len() && b[*pos] == b' ' {
        *pos += 1;
    }
    read_decimal(b, pos, 2, label)
}

fn month_name_to_num(lower: &str) -> Result<i32, String> {
    match lower {
        "jan" | "january" => Ok(1),
        "feb" | "february" => Ok(2),
        "mar" | "march" => Ok(3),
        "apr" | "april" => Ok(4),
        "may" => Ok(5),
        "jun" | "june" => Ok(6),
        "jul" | "july" => Ok(7),
        "aug" | "august" => Ok(8),
        "sep" | "september" => Ok(9),
        "oct" | "october" => Ok(10),
        "nov" | "november" => Ok(11),
        "dec" | "december" => Ok(12),
        other => Err(format!("unknown month name: {other:?}")),
    }
}

// ===========================================================================
// 4. Formatting
// ===========================================================================

/// "YYYY-MM-DD"
#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_format_isodate(y_bits: u64, m_bits: u64, d_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let y = match unpack_i64(_py, y_bits, "year") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let m = match unpack_i64(_py, m_bits, "month") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let d = match unpack_i64(_py, d_bits, "day") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let s = format!("{y:04}-{m:02}-{d:02}");
        string_bits(_py, &s)
    })
}

/// Format ISO time string.
/// `timespec_bits` is a string: "auto", "hours", "minutes", "seconds",
/// "milliseconds", "microseconds".
#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_format_isotime(
    h_bits: u64,
    min_bits: u64,
    s_bits: u64,
    us_bits: u64,
    timespec_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let h = match unpack_i64(_py, h_bits, "hour") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let mi = match unpack_i64(_py, min_bits, "minute") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let sec = match unpack_i64(_py, s_bits, "second") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let us = match unpack_i64(_py, us_bits, "microsecond") {
            Ok(v) => v,
            Err(e) => return e,
        };

        let timespec =
            string_obj_to_owned(obj_from_bits(timespec_bits)).unwrap_or_else(|| "auto".to_string());

        let s = format_isotime_impl(h, mi, sec, us, None, &timespec);
        match s {
            Ok(text) => string_bits(_py, &text),
            Err(msg) => raise_exception::<u64>(_py, "ValueError", &msg),
        }
    })
}

fn format_isotime_impl(
    h: i64,
    mi: i64,
    sec: i64,
    us: i64,
    utcoff: Option<i64>,
    timespec: &str,
) -> Result<String, String> {
    let mut out = format!("{h:02}:{mi:02}");
    match timespec {
        "hours" => {}
        "minutes" => {}
        "seconds" => {
            let _ = write!(out, ":{sec:02}");
        }
        "milliseconds" => {
            let ms = us / 1_000;
            let _ = write!(out, ":{sec:02}.{ms:03}");
        }
        "microseconds" => {
            let _ = write!(out, ":{sec:02}.{us:06}");
        }
        "auto" | "" => {
            if us != 0 {
                let _ = write!(out, ":{sec:02}.{us:06}");
            } else {
                let _ = write!(out, ":{sec:02}");
            }
        }
        other => {
            return Err(format!("Unknown timespec: {other:?}"));
        }
    }
    if let Some(off) = utcoff {
        if off == 0 {
            out.push('+');
            out.push_str("00:00");
        } else {
            let sign = if off < 0 { '-' } else { '+' };
            let off = off.abs();
            let oh = off / 3600;
            let om = (off % 3600) / 60;
            let os = off % 60;
            out.push(sign);
            if os != 0 {
                let _ = write!(out, "{oh:02}:{om:02}:{os:02}");
            } else {
                let _ = write!(out, "{oh:02}:{om:02}");
            }
        }
    }
    Ok(out)
}

/// Format a full ISO datetime string.
/// Parameters: y, m, d, h, min, s, us, sep (str), timespec (str), tz_str (str)
#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_format_isodatetime(
    y_bits: u64,
    m_bits: u64,
    d_bits: u64,
    h_bits: u64,
    min_bits: u64,
    s_bits: u64,
    us_bits: u64,
    sep_bits: u64,
    timespec_bits: u64,
    tz_str_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let y = match unpack_i64(_py, y_bits, "year") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let m = match unpack_i64(_py, m_bits, "month") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let d = match unpack_i64(_py, d_bits, "day") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let h = match unpack_i64(_py, h_bits, "hour") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let mi = match unpack_i64(_py, min_bits, "minute") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let sec = match unpack_i64(_py, s_bits, "second") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let us = match unpack_i64(_py, us_bits, "microsecond") {
            Ok(v) => v,
            Err(e) => return e,
        };

        let sep = string_obj_to_owned(obj_from_bits(sep_bits)).unwrap_or_else(|| "T".to_string());
        let timespec =
            string_obj_to_owned(obj_from_bits(timespec_bits)).unwrap_or_else(|| "auto".to_string());
        let tz_str = string_obj_to_owned(obj_from_bits(tz_str_bits)).unwrap_or_default();
        let utcoff = match parse_iso_offset_str(&tz_str) {
            Ok(value) => value,
            Err(msg) => return raise_exception::<u64>(_py, "ValueError", &msg),
        };

        let date_str = format!("{y:04}-{m:02}-{d:02}");
        let time_str = match format_isotime_impl(h, mi, sec, us, utcoff, &timespec) {
            Ok(s) => s,
            Err(msg) => return raise_exception::<u64>(_py, "ValueError", &msg),
        };

        let sep_char = sep.chars().next().unwrap_or('T');
        let result = format!("{date_str}{sep_char}{time_str}");
        string_bits(_py, &result)
    })
}

fn parse_iso_offset_str(value: &str) -> Result<Option<i64>, String> {
    if value.is_empty() {
        return Ok(None);
    }
    let sign = match value.as_bytes().first().copied() {
        Some(b'+') => 1_i64,
        Some(b'-') => -1_i64,
        _ => return Err(format!("Invalid UTC offset: {:?}", value)),
    };
    let body = &value[1..];
    let parts: Vec<&str> = body.split(':').collect();
    if parts.len() < 2 || parts.len() > 3 {
        return Err(format!("Invalid UTC offset: {:?}", value));
    }
    let hours = parts[0]
        .parse::<i64>()
        .map_err(|_| format!("Invalid UTC offset: {:?}", value))?;
    let minutes = parts[1]
        .parse::<i64>()
        .map_err(|_| format!("Invalid UTC offset: {:?}", value))?;
    let seconds = if parts.len() == 3 {
        parts[2]
            .parse::<i64>()
            .map_err(|_| format!("Invalid UTC offset: {:?}", value))?
    } else {
        0
    };
    if !(0..=23).contains(&hours) || !(0..=59).contains(&minutes) || !(0..=59).contains(&seconds) {
        return Err(format!("Invalid UTC offset: {:?}", value));
    }
    Ok(Some(sign * (hours * 3600 + minutes * 60 + seconds)))
}

// ---------------------------------------------------------------------------
// strftime implementation
// ---------------------------------------------------------------------------

const WEEKDAY_SHORT: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const WEEKDAY_LONG: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];
const MONTH_SHORT: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const MONTH_LONG: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

fn strftime_impl(args: &StrftimeArgs<'_>) -> Result<String, String> {
    let StrftimeArgs {
        y,
        m,
        d,
        h,
        mi,
        sec,
        us,
        utcoff,
        fmt,
    } = *args;
    // Derived fields
    let wday_mon0 = weekday_impl(y, m, d); // Mon=0..Sun=6
    let yday = day_of_year(y, m, d); // 1-based
    let wday_sun0 = (wday_mon0 + 1).rem_euclid(7); // Sun=0..Sat=6

    fn push_num(out: &mut String, val: i32, width: usize, pad: char) {
        let s = if val < 0 {
            format!("-{:0>width$}", -val, width = width.saturating_sub(1))
        } else {
            format!("{val:0>width$}", width = width)
        };
        if pad == ' ' {
            // right-align with spaces for the given width
            let raw = format!("{val}");
            if raw.len() < width {
                for _ in 0..(width - raw.len()) {
                    out.push(' ');
                }
            }
            out.push_str(&raw);
        } else {
            out.push_str(&s);
        }
    }

    fn week_number_sun(yday: i32, wday_sun0: i32) -> i32 {
        // First Sunday of year: yday where (wday_sun0 of Jan1 + (yday-1)) % 7 == 0
        // jan1_sun0 = (wday_sun0 - (yday - 1)).rem_euclid(7)
        let jan1_sun0 = (wday_sun0 - (yday - 1)).rem_euclid(7);
        let first_sun = 1 + (7 - jan1_sun0).rem_euclid(7);
        if yday < first_sun {
            0
        } else {
            1 + (yday - first_sun) / 7
        }
    }

    fn week_number_mon(yday: i32, wday_mon0: i32) -> i32 {
        let jan1_mon0 = (wday_mon0 - (yday - 1)).rem_euclid(7);
        let first_mon = 1 + (7 - jan1_mon0).rem_euclid(7);
        if yday < first_mon {
            0
        } else {
            1 + (yday - first_mon) / 7
        }
    }

    fn iso_week_date(y: i32, yday: i32, wday_mon0: i32) -> (i32, i32, i32) {
        let iso_wday = wday_mon0 + 1; // Mon=1
        let mut week = (yday - iso_wday + 10) / 7;
        let jan1_mon0 = (wday_mon0 - (yday - 1)).rem_euclid(7);
        let mut iso_year = y;
        // max weeks in year
        let max_weeks = if jan1_mon0 == 3 || (is_leap(y) && jan1_mon0 == 2) {
            53
        } else {
            52
        };
        if week < 1 {
            iso_year -= 1;
            let prev_days = if is_leap(iso_year) { 366 } else { 365 };
            let prev_jan1 = (jan1_mon0 - (prev_days % 7)).rem_euclid(7);
            week = if prev_jan1 == 3 || (is_leap(iso_year) && prev_jan1 == 2) {
                53
            } else {
                52
            };
        } else if week > max_weeks {
            iso_year += 1;
            week = 1;
        }
        (iso_year, week, iso_wday)
    }

    fn utcoff_str(utcoff: Option<i64>) -> String {
        match utcoff {
            None => String::new(),
            Some(off) => {
                let sign = if off < 0 { '-' } else { '+' };
                let off = off.abs();
                let oh = off / 3600;
                let om = (off % 3600) / 60;
                format!("{sign}{oh:02}{om:02}")
            }
        }
    }

    let mut out = String::with_capacity(fmt.len() + 16);
    let mut chars = fmt.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch != '%' {
            out.push(ch);
            continue;
        }
        let Some(spec) = chars.next() else {
            out.push('%');
            break;
        };
        match spec {
            '%' => out.push('%'),
            'Y' => {
                let _ = write!(out, "{y:04}");
            }
            'y' => {
                let yy = y.rem_euclid(100);
                let _ = write!(out, "{yy:02}");
            }
            'm' => {
                let _ = write!(out, "{m:02}");
            }
            'd' => {
                let _ = write!(out, "{d:02}");
            }
            'e' => {
                let _ = write!(out, "{d:>2}");
            }
            'H' => {
                let _ = write!(out, "{h:02}");
            }
            'I' => {
                let h12 = if h % 12 == 0 { 12 } else { h % 12 };
                let _ = write!(out, "{h12:02}");
            }
            'M' => {
                let _ = write!(out, "{mi:02}");
            }
            'S' => {
                let _ = write!(out, "{sec:02}");
            }
            'f' => {
                let _ = write!(out, "{us:06}");
            }
            'p' => {
                out.push_str(if h < 12 { "AM" } else { "PM" });
            }
            'a' => {
                out.push_str(WEEKDAY_SHORT[wday_mon0 as usize]);
            }
            'A' => {
                out.push_str(WEEKDAY_LONG[wday_mon0 as usize]);
            }
            'b' | 'h' => {
                out.push_str(MONTH_SHORT[(m - 1) as usize]);
            }
            'B' => {
                out.push_str(MONTH_LONG[(m - 1) as usize]);
            }
            'j' => {
                let _ = write!(out, "{yday:03}");
            }
            'U' => {
                let w = week_number_sun(yday, wday_sun0);
                let _ = write!(out, "{w:02}");
            }
            'W' => {
                let w = week_number_mon(yday, wday_mon0);
                let _ = write!(out, "{w:02}");
            }
            'w' => {
                let _ = write!(out, "{wday_sun0}");
            }
            'u' => {
                let u = wday_mon0 + 1;
                let _ = write!(out, "{u}");
            }
            'C' => {
                let century = y.div_euclid(100);
                let _ = write!(out, "{century:02}");
            }
            'G' => {
                let (iso_y, _, _) = iso_week_date(y, yday, wday_mon0);
                let _ = write!(out, "{iso_y:04}");
            }
            'g' => {
                let (iso_y, _, _) = iso_week_date(y, yday, wday_mon0);
                let yy = iso_y.rem_euclid(100);
                let _ = write!(out, "{yy:02}");
            }
            'V' => {
                let (_, iso_w, _) = iso_week_date(y, yday, wday_mon0);
                let _ = write!(out, "{iso_w:02}");
            }
            'z' => {
                out.push_str(&utcoff_str(utcoff));
            }
            'Z' => {
                if let Some(off) = utcoff {
                    if off == 0 {
                        out.push_str("UTC");
                    } else {
                        out.push_str(&utcoff_str(Some(off)));
                    }
                }
            }
            'x' => {
                let yy = y.rem_euclid(100);
                let _ = write!(out, "{m:02}/{d:02}/{yy:02}");
            }
            'X' => {
                let _ = write!(out, "{h:02}:{mi:02}:{sec:02}");
            }
            'c' => {
                // "Thu Jan  1 00:00:00 2000"
                let wd = WEEKDAY_SHORT[wday_mon0 as usize];
                let mo = MONTH_SHORT[(m - 1) as usize];
                let _ = write!(out, "{wd} {mo} {d:>2} {h:02}:{mi:02}:{sec:02} {y:04}");
            }
            'D' => {
                let yy = y.rem_euclid(100);
                let _ = write!(out, "{m:02}/{d:02}/{yy:02}");
            }
            'F' => {
                let _ = write!(out, "{y:04}-{m:02}-{d:02}");
            }
            'R' => {
                let _ = write!(out, "{h:02}:{mi:02}");
            }
            'T' => {
                let _ = write!(out, "{h:02}:{mi:02}:{sec:02}");
            }
            'r' => {
                let h12 = if h % 12 == 0 { 12 } else { h % 12 };
                let ampm = if h < 12 { "AM" } else { "PM" };
                let _ = write!(out, "{h12:02}:{mi:02}:{sec:02} {ampm}");
            }
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'k' => {
                let _ = write!(out, "{h:>2}");
            }
            'l' => {
                let h12 = if h % 12 == 0 { 12 } else { h % 12 };
                let _ = write!(out, "{h12:>2}");
            }
            other => {
                return Err(format!("unsupported strftime directive: %{other}"));
            }
        }
    }
    Ok(out)
}

/// `strftime(y, m, d, h, min, s, us, utcoff, fmt)` — 9 parameters.
#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_strftime(
    y_bits: u64,
    m_bits: u64,
    d_bits: u64,
    h_bits: u64,
    min_bits: u64,
    s_bits: u64,
    us_bits: u64,
    utcoff_bits: u64,
    fmt_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let y = match unpack_i64(_py, y_bits, "year") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };
        let m = match unpack_i64(_py, m_bits, "month") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };
        let d = match unpack_i64(_py, d_bits, "day") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };
        let h = match unpack_i64(_py, h_bits, "hour") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };
        let mi = match unpack_i64(_py, min_bits, "minute") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };
        let sec = match unpack_i64(_py, s_bits, "second") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };
        let us = match unpack_i64(_py, us_bits, "microsecond") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };

        let utcoff_obj = obj_from_bits(utcoff_bits);
        let utcoff: Option<i64> = if utcoff_obj.is_none() {
            None
        } else {
            to_i64(utcoff_obj)
        };

        let fmt = unpack_str!(_py, fmt_bits, "format");

        match strftime_impl(&StrftimeArgs {
            y,
            m,
            d,
            h,
            mi,
            sec,
            us,
            utcoff,
            fmt: &fmt,
        }) {
            Ok(s) => string_bits(_py, &s),
            Err(msg) => raise_exception::<u64>(_py, "ValueError", &msg),
        }
    })
}

/// `ctime(y, m, d, h, min, s)` → "Thu Jan  1 00:00:00 2000"
#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_ctime(
    y_bits: u64,
    m_bits: u64,
    d_bits: u64,
    h_bits: u64,
    min_bits: u64,
    s_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let y = match unpack_i64(_py, y_bits, "year") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };
        let m = match unpack_i64(_py, m_bits, "month") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };
        let d = match unpack_i64(_py, d_bits, "day") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };
        let h = match unpack_i64(_py, h_bits, "hour") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };
        let mi = match unpack_i64(_py, min_bits, "minute") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };
        let sec = match unpack_i64(_py, s_bits, "second") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };

        let wday = weekday_impl(y, m, d) as usize;
        let month_idx = (m - 1).clamp(0, 11) as usize;
        let text = format!(
            "{} {} {:>2} {:02}:{:02}:{:02} {:04}",
            WEEKDAY_SHORT[wday], MONTH_SHORT[month_idx], d, h, mi, sec, y
        );
        string_bits(_py, &text)
    })
}

// ===========================================================================
// 5. Timestamp conversions
// ===========================================================================

/// Decompose a UNIX epoch into (y, m, d, h, min, s, us) in UTC.
fn epoch_to_utc_parts(secs: i64, subsec_us: i64) -> (i32, i32, i32, i32, i32, i32, i32) {
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let h = (rem / 3600) as i32;
    let mi = ((rem % 3600) / 60) as i32;
    let sec = (rem % 60) as i32;
    // Convert Unix epoch (1970-01-01) to proleptic Gregorian ordinal.
    // 1970-01-01 ordinal = ymd_to_ordinal(1970, 1, 1) = 719_163
    let ordinal = days + 719_163;
    let (y, m, d) = ordinal_to_ymd(ordinal);
    let us = subsec_us.clamp(0, 999_999) as i32;
    (y, m, d, h, mi, sec, us)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_now_utc() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let secs = now.as_secs() as i64;
        let us = (now.subsec_micros()) as i64;
        let (y, m, d, h, mi, sec, us_out) = epoch_to_utc_parts(secs, us);
        let elems = [
            MoltObject::from_int(y as i64).bits(),
            MoltObject::from_int(m as i64).bits(),
            MoltObject::from_int(d as i64).bits(),
            MoltObject::from_int(h as i64).bits(),
            MoltObject::from_int(mi as i64).bits(),
            MoltObject::from_int(sec as i64).bits(),
            MoltObject::from_int(us_out as i64).bits(),
        ];
        tuple_bits(_py, &elems)
    })
}

/// Get local time as (y, m, d, h, min, s, us, utcoff_secs).
#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_now_local() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default();
            let secs = now.as_secs() as i64;
            let us = now.subsec_micros() as i64;
            match localtime_to_parts(secs as libc::time_t) {
                Ok((y, m, d, h, mi, sec, utcoff)) => {
                    let elems = [
                        MoltObject::from_int(y as i64).bits(),
                        MoltObject::from_int(m as i64).bits(),
                        MoltObject::from_int(d as i64).bits(),
                        MoltObject::from_int(h as i64).bits(),
                        MoltObject::from_int(mi as i64).bits(),
                        MoltObject::from_int(sec as i64).bits(),
                        MoltObject::from_int(us).bits(),
                        MoltObject::from_int(utcoff).bits(),
                    ];
                    tuple_bits(_py, &elems)
                }
                Err(msg) => raise_exception::<u64>(_py, "OSError", &msg),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            // On WASM, fall back to UTC — the host must inject local offset.
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default();
            let secs = now.as_secs() as i64;
            let us = now.subsec_micros() as i64;
            let offset_west = unsafe { crate::molt_time_local_offset_host(secs) };
            if offset_west == i64::MIN {
                return raise_exception::<u64>(
                    _py,
                    "OSError",
                    "localtime failed: timezone information unavailable",
                );
            }
            let local_secs = secs.saturating_sub(offset_west);
            let (y, m, d, h, mi, sec, us_out) = epoch_to_utc_parts(local_secs, us);
            let utcoff_secs: i64 = -offset_west;
            let _py_ref = _py; // satisfy borrow checker
            let elems = [
                MoltObject::from_int(y as i64).bits(),
                MoltObject::from_int(m as i64).bits(),
                MoltObject::from_int(d as i64).bits(),
                MoltObject::from_int(h as i64).bits(),
                MoltObject::from_int(mi as i64).bits(),
                MoltObject::from_int(sec as i64).bits(),
                MoltObject::from_int(us_out as i64).bits(),
                MoltObject::from_int(utcoff_secs).bits(),
            ];
            tuple_bits(_py_ref, &elems)
        }
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn localtime_to_parts(secs: libc::time_t) -> Result<LocalTimeParts, String> {
    #[cfg(unix)]
    unsafe {
        let mut tm = std::mem::zeroed::<libc::tm>();
        if libc::localtime_r(&secs, &mut tm).is_null() {
            return Err("localtime_r failed".to_string());
        }
        let y = tm.tm_year + 1900;
        let m = tm.tm_mon + 1;
        let d = tm.tm_mday;
        let h = tm.tm_hour;
        let mi = tm.tm_min;
        let sec = tm.tm_sec;
        // UTC offset in seconds east of UTC
        let utcoff = tm.tm_gmtoff;
        Ok((y, m, d, h, mi, sec, utcoff))
    }
    #[cfg(windows)]
    unsafe {
        let mut tm = std::mem::zeroed::<libc::tm>();
        let rc = libc::localtime_s(&mut tm, &secs);
        if rc != 0 {
            return Err("localtime_s failed".to_string());
        }
        let y = tm.tm_year + 1900;
        let m = tm.tm_mon + 1;
        let d = tm.tm_mday;
        let h = tm.tm_hour;
        let mi = tm.tm_min;
        let sec = tm.tm_sec;
        // Windows libc::tm doesn't have tm_gmtoff; compute from timezone global
        let utcoff = -(unsafe { libc::timezone } as i64);
        Ok((y, m, d, h, mi, sec, utcoff))
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err("localtime unsupported on this platform".to_string())
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_fromtimestamp_utc(ts_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ts = match unpack_f64(_py, ts_bits, "timestamp") {
            Ok(v) => v,
            Err(e) => return e,
        };
        if !ts.is_finite() {
            return raise_exception::<u64>(_py, "OverflowError", "timestamp out of range");
        }
        let secs = ts.trunc() as i64;
        let us = ((ts.fract().abs()) * 1_000_000.0).round() as i64;
        let (y, m, d, h, mi, sec, us_out) = epoch_to_utc_parts(secs, us);
        let elems = [
            MoltObject::from_int(y as i64).bits(),
            MoltObject::from_int(m as i64).bits(),
            MoltObject::from_int(d as i64).bits(),
            MoltObject::from_int(h as i64).bits(),
            MoltObject::from_int(mi as i64).bits(),
            MoltObject::from_int(sec as i64).bits(),
            MoltObject::from_int(us_out as i64).bits(),
        ];
        tuple_bits(_py, &elems)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_fromtimestamp_local(ts_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let ts = match unpack_f64(_py, ts_bits, "timestamp") {
            Ok(v) => v,
            Err(e) => return e,
        };
        if !ts.is_finite() {
            return raise_exception::<u64>(_py, "OverflowError", "timestamp out of range");
        }
        let secs = ts.trunc() as i64;
        let us = ((ts.fract().abs()) * 1_000_000.0).round() as i64;

        #[cfg(not(target_arch = "wasm32"))]
        {
            match localtime_to_parts(secs as libc::time_t) {
                Ok((y, m, d, h, mi, sec, utcoff)) => {
                    let elems = [
                        MoltObject::from_int(y as i64).bits(),
                        MoltObject::from_int(m as i64).bits(),
                        MoltObject::from_int(d as i64).bits(),
                        MoltObject::from_int(h as i64).bits(),
                        MoltObject::from_int(mi as i64).bits(),
                        MoltObject::from_int(sec as i64).bits(),
                        MoltObject::from_int(us).bits(),
                        MoltObject::from_int(utcoff).bits(),
                    ];
                    tuple_bits(_py, &elems)
                }
                Err(msg) => raise_exception::<u64>(_py, "OSError", &msg),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let offset_west = unsafe { crate::molt_time_local_offset_host(secs) };
            if offset_west == i64::MIN {
                return raise_exception::<u64>(
                    _py,
                    "OSError",
                    "localtime failed: timezone information unavailable",
                );
            }
            let local_secs = secs.saturating_sub(offset_west);
            let (y, m, d, h, mi, sec, us_out) = epoch_to_utc_parts(local_secs, us);
            let utcoff_secs: i64 = -offset_west;
            let elems = [
                MoltObject::from_int(y as i64).bits(),
                MoltObject::from_int(m as i64).bits(),
                MoltObject::from_int(d as i64).bits(),
                MoltObject::from_int(h as i64).bits(),
                MoltObject::from_int(mi as i64).bits(),
                MoltObject::from_int(sec as i64).bits(),
                MoltObject::from_int(us_out as i64).bits(),
                MoltObject::from_int(utcoff_secs).bits(),
            ];
            tuple_bits(_py, &elems)
        }
    })
}

/// Convert (y, m, d, h, min, s, us, utcoff_secs_or_none) → UNIX timestamp float.
/// `utcoff_bits` is int (seconds east) or None (treat as local time).
#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_to_timestamp(
    y_bits: u64,
    m_bits: u64,
    d_bits: u64,
    h_bits: u64,
    min_bits: u64,
    s_bits: u64,
    us_bits: u64,
    utcoff_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let y = match unpack_i64(_py, y_bits, "year") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };
        let m = match unpack_i64(_py, m_bits, "month") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };
        let d = match unpack_i64(_py, d_bits, "day") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };
        let h = match unpack_i64(_py, h_bits, "hour") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let mi = match unpack_i64(_py, min_bits, "minute") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let sec = match unpack_i64(_py, s_bits, "second") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let us = match unpack_i64(_py, us_bits, "microsecond") {
            Ok(v) => v,
            Err(e) => return e,
        };

        let utcoff_obj = obj_from_bits(utcoff_bits);

        // Compute ordinal → epoch seconds in UTC.
        let ord = ymd_to_ordinal(y, m, d);
        // Unix epoch (1970-01-01) ordinal = 719_163
        let days_since_epoch = ord - 719_163;
        let day_secs: i64 = days_since_epoch * 86_400 + h * 3_600 + mi * 60 + sec;

        let epoch_secs: i64 = if utcoff_obj.is_none() {
            // Local time: subtract local UTC offset
            #[cfg(not(target_arch = "wasm32"))]
            {
                match localtime_to_epoch_from_local(y, m, d, h as i32, mi as i32, sec as i32) {
                    Ok(v) => v,
                    Err(msg) => return raise_exception::<u64>(_py, "OSError", &msg),
                }
            }
            #[cfg(target_arch = "wasm32")]
            {
                // Approximate: use host offset for the naive timestamp
                let approx = day_secs;
                let offset_west = unsafe { crate::molt_time_local_offset_host(approx) };
                if offset_west == i64::MIN {
                    return raise_exception::<u64>(
                        _py,
                        "OSError",
                        "localtime failed: timezone information unavailable",
                    );
                }
                day_secs + offset_west
            }
        } else if let Some(off) = to_i64(utcoff_obj) {
            // Aware: subtract UTC offset to get UTC epoch
            day_secs - off
        } else {
            day_secs
        };

        let ts = epoch_secs as f64 + us as f64 / 1_000_000.0;
        MoltObject::from_float(ts).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn localtime_to_epoch_from_local(
    y: i32,
    m: i32,
    d: i32,
    h: i32,
    mi: i32,
    sec: i32,
) -> Result<i64, String> {
    #[cfg(unix)]
    unsafe {
        let mut tm = std::mem::zeroed::<libc::tm>();
        tm.tm_year = y - 1900;
        tm.tm_mon = m - 1;
        tm.tm_mday = d;
        tm.tm_hour = h;
        tm.tm_min = mi;
        tm.tm_sec = sec;
        tm.tm_isdst = -1; // let mktime determine DST
        let t = libc::mktime(&mut tm);
        if t == -1 {
            return Err("mktime failed: date out of range".to_string());
        }
        Ok(t as i64)
    }
    #[cfg(windows)]
    unsafe {
        let mut tm = std::mem::zeroed::<libc::tm>();
        tm.tm_year = y - 1900;
        tm.tm_mon = m - 1;
        tm.tm_mday = d;
        tm.tm_hour = h;
        tm.tm_min = mi;
        tm.tm_sec = sec;
        tm.tm_isdst = -1;
        let t = libc::mktime(&mut tm);
        if t == -1 {
            return Err("mktime failed: date out of range".to_string());
        }
        Ok(t as i64)
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err("mktime unsupported on this platform".to_string())
    }
}

/// Return the current local UTC offset in seconds (east of UTC).
#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_local_utcoffset() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            match localtime_to_parts(now as libc::time_t) {
                Ok((_, _, _, _, _, _, utcoff)) => MoltObject::from_int(utcoff).bits(),
                Err(_) => MoltObject::from_int(0).bits(),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            let offset_west = unsafe { crate::molt_time_local_offset_host(now) };
            if offset_west == i64::MIN {
                return raise_exception::<u64>(
                    _py,
                    "OSError",
                    "localtime failed: timezone information unavailable",
                );
            }
            let utcoff = -offset_west;
            MoltObject::from_int(utcoff).bits()
        }
    })
}

// ===========================================================================
// 6. Hashing — deterministic, matches CPython datetime hash behaviour
//
// CPython uses Python's built-in hash() on the tuple of components for
// most datetime types, but for compatibility we implement a simple
// FNV-1a–inspired mix that is stable and deterministic.
//
// CPython datetime hash algorithm (simplified):
//   date.__hash__  = hash(ymd ordinal)
//   time.__hash__  = hash((h, mi, s, us, utcoff_minutes))
//   datetime.__hash__ = combined
//   timedelta.__hash__ = hash(total_seconds * 1e6)
//
// We reproduce CPython's exact integer hash behaviour using the same
// formulae to ensure differential test correctness.
// ===========================================================================

/// CPython hash for a single Python int n.
///
/// CPython uses the integer value modulo `sys.hash_info.modulus`
/// (which is `2^61 - 1` on 64-bit) with the special case -1 → -2.
const HASH_MOD: i64 = 2_305_843_009_213_693_951; // 2^61 - 1

fn py_hash_int(n: i64) -> i64 {
    let h = n.rem_euclid(HASH_MOD);
    if h == -1 { -2 } else { h }
}

fn py_hash_i128(n: i128) -> i64 {
    let m = HASH_MOD as i128;
    let h = n.rem_euclid(m) as i64;
    if h == -1 { -2 } else { h }
}

/// Combine hashes the way CPython combines a tuple of ints:
/// uses `hash_tuple` from CPython which mixes with xxHash-like operations.
///
/// CPython tuple hash (simplified for small tuples):
///   acc = 0x27D4EB2F165667C5 ^ (len * multiplier)
///   for each item:
///       acc = acc * multiplier ^ lane_hash(item)
fn cpython_tuple_hash(values: &[i64]) -> i64 {
    // Port of CPython's tuplehash from Objects/tupleobject.c
    // xxHash constants
    const XXPRIME_1: u64 = 11400714785074694791;
    const XXPRIME_2: u64 = 14029467366897019727;
    const XXPRIME_5: u64 = 2870177450012600261;

    let n = values.len() as u64;
    let mut acc: u64 = XXPRIME_5.wrapping_add(n.wrapping_mul(XXPRIME_1));

    for &v in values {
        // CPython lane hash for int: same as py_hash_int converted to u64
        let vh = py_hash_int(v);
        let lane = vh as u64;
        acc = acc.wrapping_add(lane.wrapping_mul(XXPRIME_2));
        acc = acc.rotate_left(31);
        acc = acc.wrapping_mul(XXPRIME_1);
    }

    acc = acc.wrapping_add(n ^ XXPRIME_5 ^ 3_527_539);
    let result = acc as i64;
    if result == -1 { -2 } else { result }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_hash_date(y_bits: u64, m_bits: u64, d_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let y = match unpack_i64(_py, y_bits, "year") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let m = match unpack_i64(_py, m_bits, "month") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let d = match unpack_i64(_py, d_bits, "day") {
            Ok(v) => v,
            Err(e) => return e,
        };
        // CPython: date.__hash__ = hash(ordinal)
        let ord = ymd_to_ordinal(y as i32, m as i32, d as i32);
        MoltObject::from_int(py_hash_int(ord)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_hash_time(
    h_bits: u64,
    min_bits: u64,
    s_bits: u64,
    us_bits: u64,
    utcoff_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let h = match unpack_i64(_py, h_bits, "hour") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let mi = match unpack_i64(_py, min_bits, "minute") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let sec = match unpack_i64(_py, s_bits, "second") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let us = match unpack_i64(_py, us_bits, "microsecond") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let utcoff_obj = obj_from_bits(utcoff_bits);
        // CPython time.__hash__: hash a combined seconds+us value adjusted for tzinfo.
        // Total microseconds from midnight, then adjust for UTC offset.
        let total_us: i64 = (h * 3600 + mi * 60 + sec) * 1_000_000 + us;
        let utcoff_us: i64 = if utcoff_obj.is_none() {
            0
        } else {
            to_i64(utcoff_obj).unwrap_or(0) * 1_000_000
        };
        let adjusted = total_us - utcoff_us;
        // Hash as if it's the integer value of total microseconds
        let h_val = py_hash_i128(adjusted as i128);
        MoltObject::from_int(h_val).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_hash_datetime(
    y_bits: u64,
    m_bits: u64,
    d_bits: u64,
    h_bits: u64,
    min_bits: u64,
    s_bits: u64,
    us_bits: u64,
    utcoff_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let y = match unpack_i64(_py, y_bits, "year") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };
        let m = match unpack_i64(_py, m_bits, "month") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };
        let d = match unpack_i64(_py, d_bits, "day") {
            Ok(v) => v as i32,
            Err(e) => return e,
        };
        let h = match unpack_i64(_py, h_bits, "hour") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let mi = match unpack_i64(_py, min_bits, "minute") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let sec = match unpack_i64(_py, s_bits, "second") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let us = match unpack_i64(_py, us_bits, "microsecond") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let utcoff_obj = obj_from_bits(utcoff_bits);

        // Convert (y, m, d) to ordinal then compute total microseconds since
        // the proleptic Gregorian epoch.
        let ord = ymd_to_ordinal(y, m, d);
        // Days since epoch (0001-01-01 ordinal 1), then total us
        let day_us: i128 = (ord - 1) as i128 * 86_400 * 1_000_000;
        let time_us: i128 = (h * 3_600 + mi * 60 + sec) as i128 * 1_000_000 + us as i128;
        let total_us: i128 = day_us + time_us;

        let utcoff_us: i128 = if utcoff_obj.is_none() {
            0
        } else {
            to_i64(utcoff_obj).unwrap_or(0) as i128 * 1_000_000
        };
        let adjusted = total_us - utcoff_us;
        let h_val = py_hash_i128(adjusted);
        MoltObject::from_int(h_val).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_hash_timedelta(
    days_bits: u64,
    secs_bits: u64,
    us_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let days = match unpack_i64(_py, days_bits, "days") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let secs = match unpack_i64(_py, secs_bits, "seconds") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let us = match unpack_i64(_py, us_bits, "microseconds") {
            Ok(v) => v,
            Err(e) => return e,
        };
        // CPython: timedelta.__hash__ = hash(total_seconds * 1e6) — but
        // more precisely it hashes the tuple (days, secs, us) after normalization.
        let total_us: i128 =
            days as i128 * 86_400 * 1_000_000 + secs as i128 * 1_000_000 + us as i128;
        MoltObject::from_int(py_hash_i128(total_us)).bits()
    })
}

// ===========================================================================
// 7. Validation
// ===========================================================================

#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_validate_date(y_bits: u64, m_bits: u64, d_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let y = match unpack_i64(_py, y_bits, "year") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let m = match unpack_i64(_py, m_bits, "month") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let d = match unpack_i64(_py, d_bits, "day") {
            Ok(v) => v,
            Err(e) => return e,
        };
        if !(1..=9999).contains(&y) {
            let msg = format!("year must be in 1..9999, not {y}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        if !(1..=12).contains(&m) {
            let msg = format!("month must be in 1..12, not {m}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        let max_day = days_in_month_impl(y as i32, m as i32) as i64;
        if !(1..=max_day).contains(&d) {
            let msg = format!("day {d} must be in range 1..{max_day} for month {m} in year {y}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        MoltObject::from_bool(true).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_validate_time(
    h_bits: u64,
    min_bits: u64,
    s_bits: u64,
    us_bits: u64,
    fold_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let h = match unpack_i64(_py, h_bits, "hour") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let mi = match unpack_i64(_py, min_bits, "minute") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let sec = match unpack_i64(_py, s_bits, "second") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let us = match unpack_i64(_py, us_bits, "microsecond") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let fold = match unpack_i64(_py, fold_bits, "fold") {
            Ok(v) => v,
            Err(e) => return e,
        };
        if !(0..24).contains(&h) {
            let msg = format!("hour must be in 0..23, not {h}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        if !(0..60).contains(&mi) {
            let msg = format!("minute must be in 0..59, not {mi}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        if !(0..60).contains(&sec) {
            let msg = format!("second must be in 0..59, not {sec}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        if !(0..1_000_000).contains(&us) {
            let msg = format!("microsecond must be in 0..999999, not {us}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        if !(0..=1).contains(&fold) {
            let msg = format!("fold must be either 0 or 1, not {fold}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        MoltObject::from_bool(true).bits()
    })
}

// ─── datetime.combine ───────────────────────────────────────────────────────

/// combine(year, month, day, hour, minute, second, microsecond, fold)
/// Returns validated 8-tuple for Python wrapper to unpack.
#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_combine(
    y_bits: u64,
    m_bits: u64,
    d_bits: u64,
    h_bits: u64,
    mi_bits: u64,
    sec_bits: u64,
    us_bits: u64,
    fold_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let y = match unpack_i64(_py, y_bits, "year") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let m = match unpack_i64(_py, m_bits, "month") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let d = match unpack_i64(_py, d_bits, "day") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let h = match unpack_i64(_py, h_bits, "hour") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let mi = match unpack_i64(_py, mi_bits, "minute") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let sec = match unpack_i64(_py, sec_bits, "second") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let us = match unpack_i64(_py, us_bits, "microsecond") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let fold = match unpack_i64(_py, fold_bits, "fold") {
            Ok(v) => v,
            Err(e) => return e,
        };
        if !(1..=9999).contains(&y) || !(1..=12).contains(&m) {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                &format!("year {y} or month {m} is out of range"),
            );
        }
        let max_day = days_in_month_impl(y as i32, m as i32) as i64;
        if d < 1 || d > max_day {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                &format!("day {d} is out of range for month"),
            );
        }
        if !(0..24).contains(&h)
            || !(0..60).contains(&mi)
            || !(0..60).contains(&sec)
            || !(0..1_000_000).contains(&us)
        {
            return raise_exception::<u64>(_py, "ValueError", "time component out of range");
        }
        let elems = [
            MoltObject::from_int(y).bits(),
            MoltObject::from_int(m).bits(),
            MoltObject::from_int(d).bits(),
            MoltObject::from_int(h).bits(),
            MoltObject::from_int(mi).bits(),
            MoltObject::from_int(sec).bits(),
            MoltObject::from_int(us).bits(),
            MoltObject::from_int(fold).bits(),
        ];
        let tuple_ptr = alloc_tuple(_py, &elems);
        for &e in &elems {
            dec_ref_bits(_py, e);
        }
        if tuple_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

// ─── date.fromisocalendar ───────────────────────────────────────────────────

/// fromisocalendar(year, week, day) -> (year, month, day) tuple
#[unsafe(no_mangle)]
pub extern "C" fn molt_date_fromisocalendar(
    iso_year_bits: u64,
    iso_week_bits: u64,
    iso_day_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let iso_year = match unpack_i64(_py, iso_year_bits, "year") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let iso_week = match unpack_i64(_py, iso_week_bits, "week") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let iso_day = match unpack_i64(_py, iso_day_bits, "day") {
            Ok(v) => v,
            Err(e) => return e,
        };
        if !(1..=7).contains(&iso_day) {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                &format!("Invalid day: {iso_day} (range is [1, 7])"),
            );
        }
        let max_week = iso_weeks_in_year(iso_year as i32) as i64;
        if iso_week < 1 || iso_week > max_week {
            return raise_exception::<u64>(_py, "ValueError", &format!("Invalid week: {iso_week}"));
        }
        // January 4 is always in ISO week 1
        let jan4_ordinal = ymd_to_ordinal(iso_year as i32, 1, 4);
        let jan4_weekday = (jan4_ordinal - 1) % 7; // 0=Mon
        let week1_monday = jan4_ordinal - jan4_weekday;
        let ordinal = week1_monday + (iso_week - 1) * 7 + (iso_day - 1);
        let (y, m, d) = ordinal_to_ymd(ordinal);
        let elems = [
            MoltObject::from_int(y as i64).bits(),
            MoltObject::from_int(m as i64).bits(),
            MoltObject::from_int(d as i64).bits(),
        ];
        let tuple_ptr = alloc_tuple(_py, &elems);
        for &e in &elems {
            dec_ref_bits(_py, e);
        }
        if tuple_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

// ─── timedelta arithmetic ───────────────────────────────────────────────────

/// timedelta / scalar (int or float) -> timedelta (days, seconds, us) tuple
#[unsafe(no_mangle)]
pub extern "C" fn molt_timedelta_truediv_scalar(
    days_bits: u64,
    secs_bits: u64,
    us_bits: u64,
    divisor_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let days = match unpack_i64(_py, days_bits, "days") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let secs = match unpack_i64(_py, secs_bits, "seconds") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let us = match unpack_i64(_py, us_bits, "microseconds") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let divisor = to_f64(obj_from_bits(divisor_bits)).unwrap_or(0.0);
        if divisor == 0.0 {
            return raise_exception::<u64>(_py, "ZeroDivisionError", "division by zero");
        }
        let total_us = (days as f64) * 86_400_000_000.0 + (secs as f64) * 1_000_000.0 + (us as f64);
        let result_us = (total_us / divisor).round() as i64;
        let (rd, rs, ru) = normalize_timedelta_us(result_us);
        timedelta_tuple(_py, rd, rs, ru)
    })
}

/// timedelta / timedelta -> float
#[unsafe(no_mangle)]
pub extern "C" fn molt_timedelta_truediv_td(
    a_days: u64,
    a_secs: u64,
    a_us: u64,
    b_days: u64,
    b_secs: u64,
    b_us: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let a_total = td_total_us(a_days, a_secs, a_us);
        let b_total = td_total_us(b_days, b_secs, b_us);
        if b_total == 0.0 {
            return raise_exception::<u64>(_py, "ZeroDivisionError", "division by zero");
        }
        MoltObject::from_float(a_total / b_total).bits()
    })
}

/// timedelta // timedelta -> int
#[unsafe(no_mangle)]
pub extern "C" fn molt_timedelta_floordiv_td(
    a_days: u64,
    a_secs: u64,
    a_us: u64,
    b_days: u64,
    b_secs: u64,
    b_us: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let a_total = td_total_us(a_days, a_secs, a_us) as i64;
        let b_total = td_total_us(b_days, b_secs, b_us) as i64;
        if b_total == 0 {
            return raise_exception::<u64>(_py, "ZeroDivisionError", "integer division by zero");
        }
        MoltObject::from_int(a_total.div_euclid(b_total)).bits()
    })
}

/// timedelta % timedelta -> timedelta (days, seconds, us) tuple
#[unsafe(no_mangle)]
pub extern "C" fn molt_timedelta_mod_td(
    a_days: u64,
    a_secs: u64,
    a_us: u64,
    b_days: u64,
    b_secs: u64,
    b_us: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let a_total = td_total_us(a_days, a_secs, a_us) as i64;
        let b_total = td_total_us(b_days, b_secs, b_us) as i64;
        if b_total == 0 {
            return raise_exception::<u64>(
                _py,
                "ZeroDivisionError",
                "integer division or modulo by zero",
            );
        }
        let rem = a_total.rem_euclid(b_total);
        let (rd, rs, ru) = normalize_timedelta_us(rem);
        timedelta_tuple(_py, rd, rs, ru)
    })
}

/// timedelta // int -> timedelta (days, seconds, us) tuple
#[unsafe(no_mangle)]
pub extern "C" fn molt_timedelta_floordiv_scalar(
    days_bits: u64,
    secs_bits: u64,
    us_bits: u64,
    divisor_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let days = match unpack_i64(_py, days_bits, "days") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let secs = match unpack_i64(_py, secs_bits, "seconds") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let us = match unpack_i64(_py, us_bits, "microseconds") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let divisor = obj_from_bits(divisor_bits).as_int().unwrap_or(0);
        if divisor == 0 {
            return raise_exception::<u64>(_py, "ZeroDivisionError", "integer division by zero");
        }
        let total_us = days * 86_400_000_000 + secs * 1_000_000 + us;
        let result_us = total_us.div_euclid(divisor);
        let (rd, rs, ru) = normalize_timedelta_us(result_us);
        timedelta_tuple(_py, rd, rs, ru)
    })
}

/// abs(timedelta) -> timedelta (days, seconds, us) tuple
#[unsafe(no_mangle)]
pub extern "C" fn molt_timedelta_abs(days_bits: u64, secs_bits: u64, us_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let days = match unpack_i64(_py, days_bits, "days") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let secs = match unpack_i64(_py, secs_bits, "seconds") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let us = match unpack_i64(_py, us_bits, "microseconds") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let total_us = (days * 86_400_000_000 + secs * 1_000_000 + us).abs();
        let (rd, rs, ru) = normalize_timedelta_us(total_us);
        timedelta_tuple(_py, rd, rs, ru)
    })
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn td_total_us(days_bits: u64, secs_bits: u64, us_bits: u64) -> f64 {
    let days = obj_from_bits(days_bits).as_int().unwrap_or(0);
    let secs = obj_from_bits(secs_bits).as_int().unwrap_or(0);
    let us = obj_from_bits(us_bits).as_int().unwrap_or(0);
    (days as f64) * 86_400_000_000.0 + (secs as f64) * 1_000_000.0 + (us as f64)
}

fn normalize_timedelta_us(total_us: i64) -> (i64, i64, i64) {
    let us = total_us.rem_euclid(1_000_000);
    let total_secs = (total_us - us) / 1_000_000;
    let secs = total_secs.rem_euclid(86_400);
    let days = (total_secs - secs) / 86_400;
    (days, secs, us)
}

fn timedelta_tuple(_py: &PyToken, days: i64, secs: i64, us: i64) -> u64 {
    let elems = [
        MoltObject::from_int(days).bits(),
        MoltObject::from_int(secs).bits(),
        MoltObject::from_int(us).bits(),
    ];
    let tuple_ptr = alloc_tuple(_py, &elems);
    for &e in &elems {
        dec_ref_bits(_py, e);
    }
    if tuple_ptr.is_null() {
        return raise_exception::<u64>(_py, "MemoryError", "out of memory");
    }
    MoltObject::from_ptr(tuple_ptr).bits()
}

// ===========================================================================
// New intrinsics: _as_int, _format_time, timedelta repr/str, timezone,
// date/time/datetime repr, timetuple, datetime.__repr__
// ===========================================================================

/// Coerce a value to int: bools are promoted, ints pass through, else TypeError.
#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_as_int(value_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let obj = obj_from_bits(value_bits);
        // Accept bools (which MoltObject may represent as bool tag).
        if let Some(b) = obj.as_bool() {
            return MoltObject::from_int(if b { 1 } else { 0 }).bits();
        }
        if let Some(i) = to_i64(obj) {
            return MoltObject::from_int(i).bits();
        }
        raise_exception::<u64>(_py, "TypeError", "integer argument expected")
    })
}

/// Format time components into a string for a given timespec.
#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_format_time(
    hour_bits: u64,
    minute_bits: u64,
    second_bits: u64,
    microsecond_bits: u64,
    timespec_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let h = match unpack_i64(_py, hour_bits, "hour") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let m = match unpack_i64(_py, minute_bits, "minute") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let s = match unpack_i64(_py, second_bits, "second") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let us = match unpack_i64(_py, microsecond_bits, "microsecond") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let spec = unpack_str!(_py, timespec_bits, "timespec");
        let base = format!("{:02}:{:02}:{:02}", h, m, s);
        let result = match spec.as_str() {
            "auto" => {
                if us != 0 {
                    format!("{}.{:06}", base, us)
                } else {
                    base
                }
            }
            "seconds" => base,
            "milliseconds" => format!("{}.{:03}", base, us / 1000),
            "microseconds" => format!("{}.{:06}", base, us),
            "minutes" => format!("{:02}:{:02}", h, m),
            "hours" => format!("{:02}", h),
            _ => return raise_exception::<u64>(_py, "ValueError", "Unknown timespec value"),
        };
        string_bits(_py, &result)
    })
}

/// timedelta.__repr__()
#[unsafe(no_mangle)]
pub extern "C" fn molt_timedelta_repr(days_bits: u64, secs_bits: u64, us_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let days = match unpack_i64(_py, days_bits, "days") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let secs = match unpack_i64(_py, secs_bits, "seconds") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let us = match unpack_i64(_py, us_bits, "microseconds") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let s = format!(
            "timedelta(days={}, seconds={}, microseconds={})",
            days, secs, us
        );
        string_bits(_py, &s)
    })
}

/// timedelta.__str__()
#[unsafe(no_mangle)]
pub extern "C" fn molt_timedelta_str(days_bits: u64, secs_bits: u64, us_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let days = match unpack_i64(_py, days_bits, "days") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let secs = match unpack_i64(_py, secs_bits, "seconds") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let us = match unpack_i64(_py, us_bits, "microseconds") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let hours = secs / 3600;
        let rem = secs % 3600;
        let minutes = rem / 60;
        let seconds = rem % 60;
        let prefix = if days != 0 {
            let word = if days.abs() == 1 { "day" } else { "days" };
            format!("{} {}, ", days, word)
        } else {
            String::new()
        };
        let result = if us != 0 {
            format!(
                "{}{}:{:02}:{:02}.{:06}",
                prefix, hours, minutes, seconds, us
            )
        } else {
            format!("{}{}:{:02}:{:02}", prefix, hours, minutes, seconds)
        };
        string_bits(_py, &result)
    })
}

/// timezone.__init__ validation: return offset total seconds or raise.
#[unsafe(no_mangle)]
pub extern "C" fn molt_timezone_validate(days_bits: u64, secs_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let days = match unpack_i64(_py, days_bits, "days") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let secs = match unpack_i64(_py, secs_bits, "seconds") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let total = days * 86400 + secs;
        if !(-86400 < total && total < 86400) {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "offset must be strictly between -24h and +24h",
            );
        }
        MoltObject::from_int(total).bits()
    })
}

/// timezone.tzname() formatting
#[unsafe(no_mangle)]
pub extern "C" fn molt_timezone_tzname(days_bits: u64, secs_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let days = match unpack_i64(_py, days_bits, "days") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let secs = match unpack_i64(_py, secs_bits, "seconds") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let total = days * 86400 + secs;
        if total == 0 {
            return string_bits(_py, "UTC");
        }
        let sign = if total >= 0 { '+' } else { '-' };
        let abs_total = total.unsigned_abs();
        let hh = abs_total / 3600;
        let mm = (abs_total % 3600) / 60;
        let result = format!("UTC{}{:02}:{:02}", sign, hh, mm);
        string_bits(_py, &result)
    })
}

/// date.__repr__()
#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_date_repr(y_bits: u64, m_bits: u64, d_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let y = match unpack_i64(_py, y_bits, "year") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let m = match unpack_i64(_py, m_bits, "month") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let d = match unpack_i64(_py, d_bits, "day") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let s = format!("datetime.date({}, {}, {})", y, m, d);
        string_bits(_py, &s)
    })
}

/// time.__repr__()
#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_time_repr(
    h_bits: u64,
    m_bits: u64,
    s_bits: u64,
    us_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let h = match unpack_i64(_py, h_bits, "hour") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let m = match unpack_i64(_py, m_bits, "minute") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let s = match unpack_i64(_py, s_bits, "second") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let us = match unpack_i64(_py, us_bits, "microsecond") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let result = if us != 0 {
            format!("datetime.time({}, {}, {}, {})", h, m, s, us)
        } else if s != 0 {
            format!("datetime.time({}, {}, {})", h, m, s)
        } else {
            format!("datetime.time({}, {})", h, m)
        };
        string_bits(_py, &result)
    })
}

/// datetime.__repr__()
#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_datetime_repr(
    y_bits: u64,
    mo_bits: u64,
    d_bits: u64,
    h_bits: u64,
    mi_bits: u64,
    s_bits: u64,
    us_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let y = match unpack_i64(_py, y_bits, "year") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let mo = match unpack_i64(_py, mo_bits, "month") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let d = match unpack_i64(_py, d_bits, "day") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let h = match unpack_i64(_py, h_bits, "hour") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let mi = match unpack_i64(_py, mi_bits, "minute") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let s = match unpack_i64(_py, s_bits, "second") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let us = match unpack_i64(_py, us_bits, "microsecond") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let result = if us != 0 {
            format!(
                "datetime.datetime({}, {}, {}, {}, {}, {}, {})",
                y, mo, d, h, mi, s, us
            )
        } else if s != 0 {
            format!(
                "datetime.datetime({}, {}, {}, {}, {}, {})",
                y, mo, d, h, mi, s
            )
        } else if mi != 0 {
            format!("datetime.datetime({}, {}, {}, {}, {})", y, mo, d, h, mi)
        } else if h != 0 {
            format!("datetime.datetime({}, {}, {}, {})", y, mo, d, h)
        } else {
            format!("datetime.datetime({}, {}, {})", y, mo, d)
        };
        string_bits(_py, &result)
    })
}

/// datetime.timetuple() -> 9-tuple of ints
#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_timetuple(
    y_bits: u64,
    mo_bits: u64,
    d_bits: u64,
    h_bits: u64,
    mi_bits: u64,
    s_bits: u64,
    weekday_bits: u64,
    yday_bits: u64,
    dst_flag_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let y = match unpack_i64(_py, y_bits, "year") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let mo = match unpack_i64(_py, mo_bits, "month") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let d = match unpack_i64(_py, d_bits, "day") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let h = match unpack_i64(_py, h_bits, "hour") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let mi = match unpack_i64(_py, mi_bits, "minute") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let s = match unpack_i64(_py, s_bits, "second") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let wd = match unpack_i64(_py, weekday_bits, "weekday") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let yday = match unpack_i64(_py, yday_bits, "yday") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let dst = match unpack_i64(_py, dst_flag_bits, "dst") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let elems = [
            MoltObject::from_int(y).bits(),
            MoltObject::from_int(mo).bits(),
            MoltObject::from_int(d).bits(),
            MoltObject::from_int(h).bits(),
            MoltObject::from_int(mi).bits(),
            MoltObject::from_int(s).bits(),
            MoltObject::from_int(wd).bits(),
            MoltObject::from_int(yday).bits(),
            MoltObject::from_int(dst).bits(),
        ];
        tuple_bits(_py, &elems)
    })
}
