#![allow(dead_code, unused_imports)]
// === FILE: runtime/molt-runtime/src/builtins/zoneinfo.rs ===
//! `zoneinfo` module intrinsics for Molt.
//!
//! Reads IANA TZif v1/v2/v3 binary data from `/usr/share/zoneinfo` and exposes
//! timezone key, UTC offset, tzname, and DST offset for a given datetime.
//!
//! The `dt_components_bits` argument is a tuple of 6 or 7 integers:
//!   (year, month, day, hour, minute, second[, fold])
//! representing a naive local datetime for which to compute the offset.
//!
//! ABI: NaN-boxed u64 in/out.

use crate::builtins::numbers::int_bits_from_i64;
use crate::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicI64, Ordering};

// ── Handle-id counter ─────────────────────────────────────────────────────

static NEXT_ZONE_ID: AtomicI64 = AtomicI64::new(1);

fn next_zone_id() -> i64 {
    NEXT_ZONE_ID.fetch_add(1, Ordering::Relaxed)
}

// ── TZif binary format constants ──────────────────────────────────────────

const TZIF_MAGIC: &[u8; 4] = b"TZif";

// ── Parsed timezone data ───────────────────────────────────────────────────

/// A single transition: wall-clock time (Unix timestamp) at which the offset
/// changes, the new UTC offset in seconds, a DST flag, and the abbreviation.
#[derive(Clone, Debug)]
struct Transition {
    /// Transition time as Unix seconds.
    at: i64,
    /// UTC offset after this transition in seconds (west negative).
    utoff: i32,
    /// Nonzero when daylight saving is in effect.
    is_dst: u8,
    /// Abbreviation index into `abbr_string` buffer.
    abbr_idx: usize,
}

#[derive(Clone, Debug)]
struct ZoneData {
    key: String,
    transitions: Vec<Transition>,
    abbr_strings: Vec<u8>,
    /// Default offset when no transitions exist.
    default_utoff: i32,
    default_abbr: String,
    default_is_dst: bool,
}

impl ZoneData {
    /// Compute the UTC offset in seconds for a naive datetime expressed as
    /// Unix timestamp in UTC (approximate: year/mon/day/h/m/s treated as UTC).
    fn utoff_at(&self, unix_ts: i64) -> (i32, bool, &str) {
        if self.transitions.is_empty() {
            return (self.default_utoff, self.default_is_dst, &self.default_abbr);
        }
        // Binary search: find last transition at or before unix_ts.
        let idx = match self.transitions.binary_search_by_key(&unix_ts, |t| t.at) {
            Ok(i) => i,
            Err(0) => 0,
            Err(i) => i - 1,
        };
        let t = &self.transitions[idx];
        let abbr = abbr_from_bytes(&self.abbr_strings, t.abbr_idx);
        (t.utoff, t.is_dst != 0, abbr)
    }

    fn local_mapping_at(&self, local_unix_ts: i64, fold: i64) -> (i32, bool, &str) {
        let mut candidates: Vec<(i64, i32, bool, &str)> = Vec::new();
        candidates.push((
            local_unix_ts - self.default_utoff as i64,
            self.default_utoff,
            self.default_is_dst,
            &self.default_abbr,
        ));
        for transition in &self.transitions {
            let candidate_utc = local_unix_ts - transition.utoff as i64;
            let (resolved_off, is_dst, abbr) = self.utoff_at(candidate_utc);
            if resolved_off == transition.utoff {
                candidates.push((candidate_utc, resolved_off, is_dst, abbr));
            }
        }
        candidates.sort_by_key(|entry| entry.0);
        candidates.dedup_by(|left, right| left.0 == right.0 && left.1 == right.1);
        match candidates.len() {
            0 => self.utoff_at(local_unix_ts),
            1 => {
                let (_, utoff, is_dst, abbr) = candidates[0];
                (utoff, is_dst, abbr)
            }
            _ => {
                let index = if fold > 0 { candidates.len() - 1 } else { 0 };
                let (_, utoff, is_dst, abbr) = candidates[index];
                (utoff, is_dst, abbr)
            }
        }
    }
}

fn abbr_from_bytes(buf: &[u8], idx: usize) -> &str {
    if idx >= buf.len() {
        return "UTC";
    }
    let end = buf[idx..]
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(buf.len() - idx);
    std::str::from_utf8(&buf[idx..idx + end]).unwrap_or("UTC")
}

// ── TZif parser ──────────────────────────────────────────────────────────

fn parse_tzif(data: &[u8], key: &str) -> Result<ZoneData, String> {
    if data.len() < 44 {
        return Err(format!("TZif too short ({} bytes)", data.len()));
    }
    if &data[0..4] != TZIF_MAGIC {
        return Err("not a TZif file".to_string());
    }
    let version = data[4];

    // Try v2/v3 data block first (starts with a second header after the v1 block).
    // v1 block sizes are determined from the v1 header counts.
    let parse_block = |offset: usize, ptr_size: usize, data: &[u8]| -> Result<ZoneData, String> {
        if offset + 44 > data.len() {
            return Err("TZif header truncated".to_string());
        }
        let buf = &data[offset..];
        // Counts from header bytes 20-43 (6 × u32 BE).
        let read_u32 = |pos: usize| -> usize {
            u32::from_be_bytes(buf[pos..pos + 4].try_into().unwrap_or([0; 4])) as usize
        };
        let ttisgmtcnt = read_u32(20);
        let ttisstdcnt = read_u32(24);
        let leapcnt = read_u32(28);
        let timecnt = read_u32(32);
        let typecnt = read_u32(36);
        let charcnt = read_u32(40);

        let header_end = 44;
        let trans_times_sz = timecnt * ptr_size;
        let trans_types_sz = timecnt;
        let ttinfo_sz = typecnt * 6; // each ttinfo is 6 bytes
        let abbr_sz = charcnt;
        let leap_sz = leapcnt * (ptr_size + 4);
        let isstd_sz = ttisstdcnt;
        let isgmt_sz = ttisgmtcnt;

        let needed = header_end
            + trans_times_sz
            + trans_types_sz
            + ttinfo_sz
            + abbr_sz
            + leap_sz
            + isstd_sz
            + isgmt_sz;
        if offset + needed > data.len() {
            return Err(format!("TZif block truncated (need {needed})"));
        }

        let mut pos = offset + header_end;

        // Read transition times.
        let mut trans_times = Vec::with_capacity(timecnt);
        for _ in 0..timecnt {
            let t = if ptr_size == 4 {
                i32::from_be_bytes(
                    buf[pos - offset..pos - offset + 4]
                        .try_into()
                        .unwrap_or([0; 4]),
                ) as i64
            } else {
                i64::from_be_bytes(
                    buf[pos - offset..pos - offset + 8]
                        .try_into()
                        .unwrap_or([0; 8]),
                )
            };
            trans_times.push(t);
            pos += ptr_size;
        }

        // Read transition type indices.
        let mut trans_type_idxs = Vec::with_capacity(timecnt);
        for i in 0..timecnt {
            trans_type_idxs.push(buf[pos - offset + i] as usize);
        }
        pos += trans_types_sz;

        // Read ttinfo structs (6 bytes each: i32 utoff, u8 is_dst, u8 abbr_idx).
        let mut ttinfos: Vec<(i32, u8, usize)> = Vec::with_capacity(typecnt);
        for _ in 0..typecnt {
            let utoff = i32::from_be_bytes(
                buf[pos - offset..pos - offset + 4]
                    .try_into()
                    .unwrap_or([0; 4]),
            );
            let is_dst = buf[pos - offset + 4];
            let abbr_idx = buf[pos - offset + 5] as usize;
            ttinfos.push((utoff, is_dst, abbr_idx));
            pos += 6;
        }

        // Read abbreviation strings.
        let abbr_strings = buf[pos - offset..pos - offset + abbr_sz].to_vec();
        pos += abbr_sz;
        // Skip leapcnt, isstd, isgmt — we don't use them.
        let _ = pos;

        // Build transitions.
        let mut transitions = Vec::with_capacity(timecnt);
        for i in 0..timecnt {
            let type_idx = if trans_type_idxs[i] < ttinfos.len() {
                trans_type_idxs[i]
            } else {
                0
            };
            let (utoff, is_dst, abbr_idx) = ttinfos[type_idx];
            transitions.push(Transition {
                at: trans_times[i],
                utoff,
                is_dst,
                abbr_idx,
            });
        }

        let (default_utoff, default_is_dst, default_abbr_raw) = if let Some(first) = ttinfos.first()
        {
            let abbr = abbr_from_bytes(&abbr_strings, first.2).to_string();
            (first.0, first.1 != 0, abbr)
        } else {
            (0, false, "UTC".to_string())
        };

        Ok(ZoneData {
            key: key.to_string(),
            transitions,
            abbr_strings,
            default_utoff,
            default_abbr: default_abbr_raw,
            default_is_dst,
        })
    };

    // Compute v1 block size so we can jump to v2 block.
    let v1_size: usize = {
        let read_u32 = |pos: usize| -> usize {
            u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap_or([0; 4])) as usize
        };
        let ttisgmtcnt = read_u32(20);
        let ttisstdcnt = read_u32(24);
        let leapcnt = read_u32(28);
        let timecnt = read_u32(32);
        let typecnt = read_u32(36);
        let charcnt = read_u32(40);
        44 + timecnt * 4 + timecnt + typecnt * 6 + charcnt + leapcnt * 8 + ttisstdcnt + ttisgmtcnt
    };

    if (version == b'2' || version == b'3') && v1_size + 44 <= data.len() {
        parse_block(v1_size, 8, data)
    } else {
        parse_block(0, 4, data)
    }
}

// ── Zone data loader ──────────────────────────────────────────────────────

const ZONEINFO_DIRS: &[&str] = &[
    "/usr/share/zoneinfo",
    "/usr/lib/zoneinfo",
    "/usr/share/lib/zoneinfo",
    "/etc/zoneinfo",
    "/usr/share/zoneinfo.default",
];

fn load_zone(key: &str) -> Result<ZoneData, String> {
    // Validate key: no .. traversal.
    if key.contains("..") || key.starts_with('/') {
        return Err(format!("invalid timezone key: {key}"));
    }
    for dir in ZONEINFO_DIRS {
        let path = format!("{dir}/{key}");
        if let Ok(data) = std::fs::read(&path) {
            return parse_tzif(&data, key);
        }
    }
    Err(format!("timezone '{key}' not found"))
}

// ── Available timezone names ──────────────────────────────────────────────

fn available_timezones_set() -> Vec<String> {
    let mut names = Vec::new();
    for dir in ZONEINFO_DIRS {
        if collect_zone_names(dir, "", &mut names).is_ok() && !names.is_empty() {
            break;
        }
    }
    names
}

fn collect_zone_names(base: &str, prefix: &str, out: &mut Vec<String>) -> Result<(), ()> {
    let dir_path = if prefix.is_empty() {
        base.to_string()
    } else {
        format!("{base}/{prefix}")
    };
    let entries = std::fs::read_dir(&dir_path).map_err(|_| ())?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        // Skip metadata files.
        if name_str.starts_with('+')
            || name_str == "leap-seconds.list"
            || name_str == "posixrules"
            || name_str == "localtime"
            || name_str == "zone.tab"
            || name_str == "zone1970.tab"
            || name_str == "iso3166.tab"
            || name_str.ends_with(".list")
            || name_str.ends_with(".tab")
            || name_str.ends_with(".tz")
        {
            continue;
        }
        let full_key = if prefix.is_empty() {
            name_str.to_string()
        } else {
            format!("{prefix}/{name_str}")
        };
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            let _ = collect_zone_names(base, &full_key, out);
        } else {
            // Quick check: readable as TZif.
            let path = format!("{base}/{full_key}");
            if let Ok(data) = std::fs::read(&path)
                && data.len() >= 4
                && &data[0..4] == TZIF_MAGIC
            {
                out.push(full_key);
            }
        }
    }
    Ok(())
}

// ── Thread-local handle map ───────────────────────────────────────────────

thread_local! {
    static ZONE_MAP: RefCell<HashMap<i64, ZoneData>> = RefCell::new(HashMap::new());
}

// ── datetime components → approximate Unix timestamp ─────────────────────

fn dt_components_to_unix(components: &[i64]) -> i64 {
    if components.len() < 3 {
        return 0;
    }
    let year = components[0];
    let month = components[1].clamp(1, 12);
    let day = components[2].clamp(1, 31);
    let hour = components.get(3).copied().unwrap_or(0);
    let minute = components.get(4).copied().unwrap_or(0);
    let second = components.get(5).copied().unwrap_or(0);

    // Simplified Gregorian calendar → Unix timestamp (ignoring timezone).
    let days_from_epoch = {
        let y = year - 1970;
        let leap_days = y / 4 - y / 100 + y / 400;
        const MONTH_DAYS: [i64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        let day_of_year: i64 = MONTH_DAYS[..((month - 1) as usize)].iter().sum::<i64>() + day - 1;
        y * 365 + leap_days + day_of_year
    };
    days_from_epoch * 86400 + hour * 3600 + minute * 60 + second
}

fn dt_components_fold(components: &[i64]) -> i64 {
    components.get(6).copied().unwrap_or(0)
}

/// Extract i64 tuple/list into a Vec<i64>.
fn extract_dt_components(_py: &PyToken<'_>, bits: u64) -> Option<Vec<i64>> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return Some(vec![1970, 1, 1, 0, 0, 0]);
    }
    let ptr = obj.as_ptr()?;
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id != TYPE_ID_TUPLE && type_id != TYPE_ID_LIST {
            return None;
        }
        let elems = seq_vec_ref(ptr);
        let result: Vec<i64> = elems
            .iter()
            .filter_map(|&b| to_i64(obj_from_bits(b)))
            .collect();
        Some(result)
    }
}

// ── Public intrinsics ─────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_zoneinfo_new(key_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let key = match string_obj_to_owned(obj_from_bits(key_bits)) {
            Some(k) => k,
            None => return raise_exception::<u64>(_py, "TypeError", "timezone key must be a str"),
        };
        match load_zone(&key) {
            Ok(zone) => {
                let id = next_zone_id();
                ZONE_MAP.with(|m| m.borrow_mut().insert(id, zone));
                int_bits_from_i64(_py, id)
            }
            Err(msg) => raise_exception::<u64>(_py, "zoneinfo.ZoneInfoNotFoundError", &msg),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zoneinfo_key(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "zoneinfo handle must be int"),
        };
        let key = ZONE_MAP.with(|m| m.borrow().get(&id).map(|z| z.key.clone()));
        match key {
            None => raise_exception::<u64>(_py, "ValueError", "invalid zoneinfo handle"),
            Some(k) => {
                let ptr = alloc_string(_py, k.as_bytes());
                if ptr.is_null() {
                    raise_exception::<u64>(_py, "MemoryError", "out of memory")
                } else {
                    MoltObject::from_ptr(ptr).bits()
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zoneinfo_utcoffset(handle_bits: u64, dt_components_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "zoneinfo handle must be int"),
        };
        let comps = match extract_dt_components(_py, dt_components_bits) {
            Some(c) => c,
            None => {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "dt_components must be a tuple of ints",
                );
            }
        };
        let local_unix_ts = dt_components_to_unix(&comps);
        let fold = dt_components_fold(&comps);
        let utoff = ZONE_MAP.with(|m| {
            m.borrow()
                .get(&id)
                .map(|z| z.local_mapping_at(local_unix_ts, fold).0)
        });
        match utoff {
            None => raise_exception::<u64>(_py, "ValueError", "invalid zoneinfo handle"),
            Some(secs) => int_bits_from_i64(_py, secs as i64),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zoneinfo_tzname(handle_bits: u64, dt_components_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "zoneinfo handle must be int"),
        };
        let comps = match extract_dt_components(_py, dt_components_bits) {
            Some(c) => c,
            None => {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "dt_components must be a tuple of ints",
                );
            }
        };
        let local_unix_ts = dt_components_to_unix(&comps);
        let fold = dt_components_fold(&comps);
        let name_opt: Option<String> = ZONE_MAP.with(|m| {
            let map = m.borrow();
            let zone = map.get(&id)?;
            let (utoff, _is_dst, abbr) = zone.local_mapping_at(local_unix_ts, fold);
            // Produce a CPython-compatible tzname: abbreviation or "UTC±HH:MM".
            if !abbr.is_empty() && abbr != "UTC" {
                Some(abbr.to_string())
            } else {
                let sign = if utoff >= 0 { '+' } else { '-' };
                let abs = utoff.unsigned_abs();
                let h = abs / 3600;
                let m = (abs % 3600) / 60;
                Some(format!("UTC{sign}{h:02}:{m:02}"))
            }
        });
        match name_opt {
            None => raise_exception::<u64>(_py, "ValueError", "invalid zoneinfo handle"),
            Some(name) => {
                let ptr = alloc_string(_py, name.as_bytes());
                if ptr.is_null() {
                    raise_exception::<u64>(_py, "MemoryError", "out of memory")
                } else {
                    MoltObject::from_ptr(ptr).bits()
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zoneinfo_dst(handle_bits: u64, dt_components_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "TypeError", "zoneinfo handle must be int"),
        };
        let comps = match extract_dt_components(_py, dt_components_bits) {
            Some(c) => c,
            None => {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "dt_components must be a tuple of ints",
                );
            }
        };
        let local_unix_ts = dt_components_to_unix(&comps);
        let fold = dt_components_fold(&comps);
        let dst_secs: Option<i64> = ZONE_MAP.with(|m| {
            let map = m.borrow();
            let zone = map.get(&id)?;
            let (_utoff, is_dst, _abbr) = zone.local_mapping_at(local_unix_ts, fold);
            // CPython returns the DST offset (difference between standard and DST)
            // as a timedelta in seconds.  If no DST, returns 0.
            if is_dst { Some(3600) } else { Some(0) }
        });
        match dst_secs {
            None => raise_exception::<u64>(_py, "ValueError", "invalid zoneinfo handle"),
            Some(secs) => int_bits_from_i64(_py, secs),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zoneinfo_available_timezones() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let names = available_timezones_set();
        let mut str_bits = Vec::with_capacity(names.len());
        for name in &names {
            let ptr = alloc_string(_py, name.as_bytes());
            if ptr.is_null() {
                return raise_exception::<u64>(_py, "MemoryError", "out of memory");
            }
            str_bits.push(MoltObject::from_ptr(ptr).bits());
        }
        let set_ptr = alloc_set_with_entries(_py, &str_bits);
        if set_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(set_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zoneinfo_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Some(id) = to_i64(obj_from_bits(handle_bits)) {
            ZONE_MAP.with(|m| m.borrow_mut().remove(&id));
        }
        MoltObject::none().bits()
    })
}
