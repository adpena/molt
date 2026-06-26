use super::*;

#[cfg(all(not(target_arch = "wasm32"), unix))]
pub(crate) fn process_time_duration() -> Result<std::time::Duration, String> {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut ts) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if ts.tv_sec < 0 || ts.tv_nsec < 0 {
        return Err("process time before epoch".to_string());
    }
    Ok(std::time::Duration::new(
        ts.tv_sec as u64,
        ts.tv_nsec as u32,
    ))
}

#[cfg(all(not(target_arch = "wasm32"), windows))]
pub(crate) fn process_time_duration() -> Result<std::time::Duration, String> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    let mut creation = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut exit = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut kernel = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut user = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let handle = unsafe { GetCurrentProcess() };
    let ok = unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) };
    if ok == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let kernel_100ns = ((kernel.dwHighDateTime as u64) << 32) | kernel.dwLowDateTime as u64;
    let user_100ns = ((user.dwHighDateTime as u64) << 32) | user.dwLowDateTime as u64;
    let total_100ns = kernel_100ns.saturating_add(user_100ns);
    let secs = total_100ns / 10_000_000;
    let nanos = (total_100ns % 10_000_000) * 100;
    Ok(std::time::Duration::new(secs, nanos as u32))
}

#[cfg(any(target_arch = "wasm32", not(any(unix, windows))))]
pub(crate) fn process_time_duration() -> Result<std::time::Duration, String> {
    Err("process_time unavailable".to_string())
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TimeParts {
    pub(crate) year: i32,
    pub(crate) month: i32,
    pub(crate) day: i32,
    pub(crate) hour: i32,
    pub(crate) minute: i32,
    pub(crate) second: i32,
    pub(crate) wday: i32,
    pub(crate) yday: i32,
    pub(crate) isdst: i32,
}

pub(crate) fn time_parts_to_tuple(_py: &PyToken<'_>, parts: TimeParts) -> u64 {
    let elems = [
        MoltObject::from_int(parts.year as i64).bits(),
        MoltObject::from_int(parts.month as i64).bits(),
        MoltObject::from_int(parts.day as i64).bits(),
        MoltObject::from_int(parts.hour as i64).bits(),
        MoltObject::from_int(parts.minute as i64).bits(),
        MoltObject::from_int(parts.second as i64).bits(),
        MoltObject::from_int(parts.wday as i64).bits(),
        MoltObject::from_int(parts.yday as i64).bits(),
        MoltObject::from_int(parts.isdst as i64).bits(),
    ];
    let tuple_ptr = alloc_tuple(_py, &elems);
    if tuple_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(tuple_ptr).bits()
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn time_parts_from_tm(tm: &libc::tm) -> TimeParts {
    let wday = (tm.tm_wday + 6).rem_euclid(7);
    TimeParts {
        year: tm.tm_year + 1900,
        month: tm.tm_mon + 1,
        day: tm.tm_mday,
        hour: tm.tm_hour,
        minute: tm.tm_min,
        second: tm.tm_sec,
        wday,
        yday: tm.tm_yday + 1,
        isdst: tm.tm_isdst,
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn tm_from_time_parts(_py: &PyToken<'_>, parts: TimeParts) -> Result<libc::tm, u64> {
    let mut tm = unsafe { std::mem::zeroed::<libc::tm>() };
    tm.tm_sec = parts.second;
    tm.tm_min = parts.minute;
    tm.tm_hour = parts.hour;
    tm.tm_mday = parts.day;
    tm.tm_mon = parts.month - 1;
    tm.tm_year = parts.year - 1900;
    tm.tm_wday = (parts.wday + 1).rem_euclid(7);
    tm.tm_yday = parts.yday - 1;
    tm.tm_isdst = parts.isdst;
    if tm.tm_mon < 0 || tm.tm_mon > 11 {
        return Err(raise_exception::<_>(
            _py,
            "ValueError",
            "strftime() argument 2 out of range",
        ));
    }
    Ok(tm)
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn day_of_year(year: i32, month: i32, day: i32) -> i32 {
    const DAYS_BEFORE_MONTH: [[i32; 13]; 2] = [
        [0, 0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334],
        [0, 0, 31, 60, 91, 121, 152, 182, 213, 244, 274, 305, 335],
    ];
    let leap = if is_leap_year(year) { 1 } else { 0 };
    let m = month.clamp(1, 12) as usize;
    DAYS_BEFORE_MONTH[leap][m] + day
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn civil_from_days(days: i64) -> (i32, i32, i32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut y = (yoe + era * 400) as i32;
    let doy = (doe - (365 * yoe + yoe / 4 - yoe / 100)) as i32;
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1);
    let m = (mp + if mp < 10 { 3 } else { -9 });
    if m <= 2 {
        y += 1;
    }
    (y, m, d)
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn time_parts_from_epoch_utc(secs: i64) -> TimeParts {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let hour = (rem / 3600) as i32;
    let minute = ((rem % 3600) / 60) as i32;
    let second = (rem % 60) as i32;
    let (year, month, day) = civil_from_days(days);
    let yday = day_of_year(year, month, day);
    let wday = ((days + 3).rem_euclid(7)) as i32;
    TimeParts {
        year,
        month,
        day,
        hour,
        minute,
        second,
        wday,
        yday,
        isdst: 0,
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn timezone_west_wasm() -> Result<i64, String> {
    let offset = unsafe { crate::molt_time_timezone_host() };
    if offset == i64::MIN {
        return Err("timezone unavailable".to_string());
    }
    Ok(offset)
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn local_offset_west_wasm(secs: i64) -> Result<i64, String> {
    let offset = unsafe { crate::molt_time_local_offset_host(secs) };
    if offset == i64::MIN {
        return Err("localtime failed".to_string());
    }
    Ok(offset)
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn tzname_label_wasm(which: i32) -> Result<String, String> {
    let mut buf = vec![0u8; 256];
    let mut out_len: u32 = 0;
    let status = unsafe {
        crate::molt_time_tzname_host(
            which,
            buf.as_mut_ptr() as u32,
            buf.len() as u32,
            (&mut out_len as *mut u32) as u32,
        )
    };
    if status != 0 {
        return Err("tzname unavailable".to_string());
    }
    let out_len = usize::try_from(out_len).map_err(|_| "tzname unavailable".to_string())?;
    if out_len > buf.len() {
        return Err("tzname unavailable".to_string());
    }
    buf.truncate(out_len);
    String::from_utf8(buf).map_err(|_| "tzname unavailable".to_string())
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn tzname_wasm() -> Result<(String, String), String> {
    let std_name = tzname_label_wasm(0)?;
    let dst_name = tzname_label_wasm(1)?;
    Ok((std_name, dst_name))
}

#[cfg(any(unix, target_arch = "wasm32"))]
pub(crate) fn current_epoch_secs_i64() -> Result<i64, String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "system time before epoch".to_string())?;
    Ok(i64::try_from(now.as_secs()).unwrap_or(i64::MAX))
}

pub(crate) fn parse_time_seconds(_py: &PyToken<'_>, secs_bits: u64) -> Result<i64, u64> {
    let obj = obj_from_bits(secs_bits);
    if obj.is_none() {
        let now = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(now) => now,
            Err(_) => {
                return Err(raise_exception::<_>(
                    _py,
                    "OSError",
                    "system time before epoch",
                ));
            }
        };
        let secs = now.as_secs();
        let secs = i64::try_from(secs).unwrap_or(i64::MAX);
        return Ok(secs);
    }
    let Some(val) = to_f64(obj) else {
        let type_name = class_name_for_error(type_of_bits(_py, secs_bits));
        let msg = format!("an integer is required (got type {type_name})");
        return Err(raise_exception::<_>(_py, "TypeError", &msg));
    };
    if !val.is_finite() {
        return Err(raise_exception::<_>(
            _py,
            "OverflowError",
            "timestamp out of range for platform time_t",
        ));
    }
    let secs = val.trunc();
    let (min, max) = time_t_bounds();
    if secs < min as f64 || secs > max as f64 {
        return Err(raise_exception::<_>(
            _py,
            "OverflowError",
            "timestamp out of range for platform time_t",
        ));
    }
    Ok(secs as i64)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn time_t_bounds() -> (i128, i128) {
    let size = std::mem::size_of::<libc::time_t>();
    if size == 4 {
        (i32::MIN as i128, i32::MAX as i128)
    } else {
        (i64::MIN as i128, i64::MAX as i128)
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn time_t_bounds() -> (i128, i128) {
    (i64::MIN as i128, i64::MAX as i128)
}

pub(crate) fn days_from_civil(year: i32, month: i32, day: i32) -> i64 {
    let mut y = year as i64;
    let m = month as i64;
    let d = day as i64;
    y -= if m <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = m + if m > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(unix)]
pub(crate) fn tm_to_epoch_seconds(tm: &libc::tm) -> i64 {
    let year = tm.tm_year + 1900;
    let month = tm.tm_mon + 1;
    let day = tm.tm_mday;
    let days = days_from_civil(year, month, day);
    let seconds = (tm.tm_hour as i64) * 3600 + (tm.tm_min as i64) * 60 + (tm.tm_sec as i64);
    days.saturating_mul(86_400).saturating_add(seconds)
}

#[cfg(unix)]
pub(crate) fn offset_west_from_secs(secs: i64) -> Result<i64, String> {
    let secs = secs as libc::time_t;
    let local_tm = localtime_tm(secs)?;
    let utc_tm = gmtime_tm(secs)?;
    let local_secs = tm_to_epoch_seconds(&local_tm);
    let utc_secs = tm_to_epoch_seconds(&utc_tm);
    Ok(utc_secs.saturating_sub(local_secs))
}

pub(crate) fn parse_time_tuple(_py: &PyToken<'_>, tuple_bits: u64) -> Result<TimeParts, u64> {
    let obj = obj_from_bits(tuple_bits);
    let Some(ptr) = obj.as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "strftime() argument 2 must be tuple",
        ));
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_TUPLE {
            let type_name = class_name_for_error(type_of_bits(_py, tuple_bits));
            let msg = format!("strftime() argument 2 must be tuple, not {type_name}");
            return Err(raise_exception::<_>(_py, "TypeError", &msg));
        }
        let elems = seq_vec_ref(ptr);
        if elems.len() != 9 {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "time tuple must have exactly 9 elements",
            ));
        }
        let mut vals = [0i64; 9];
        for (idx, slot) in vals.iter_mut().enumerate() {
            let bits = elems[idx];
            let Some(val) = to_i64(obj_from_bits(bits)) else {
                let type_name = class_name_for_error(type_of_bits(_py, bits));
                let msg = format!("an integer is required (got type {type_name})");
                return Err(raise_exception::<_>(_py, "TypeError", &msg));
            };
            if val < i32::MIN as i64 || val > i32::MAX as i64 {
                return Err(raise_exception::<_>(
                    _py,
                    "ValueError",
                    "strftime() argument 2 out of range",
                ));
            }
            *slot = val;
        }
        let year = vals[0] as i32;
        let month = vals[1] as i32;
        let day = vals[2] as i32;
        let hour = vals[3] as i32;
        let minute = vals[4] as i32;
        let second = vals[5] as i32;
        let wday = vals[6] as i32;
        let yday = vals[7] as i32;
        let isdst = vals[8] as i32;
        if !(1..=12).contains(&month)
            || !(1..=31).contains(&day)
            || !(0..=23).contains(&hour)
            || !(0..=59).contains(&minute)
            || !(0..=60).contains(&second)
            || !(0..=6).contains(&wday)
            || !(1..=366).contains(&yday)
        {
            return Err(raise_exception::<_>(
                _py,
                "ValueError",
                "strftime() argument 2 out of range",
            ));
        }
        if ![-1, 0, 1].contains(&isdst) {
            return Err(raise_exception::<_>(
                _py,
                "ValueError",
                "strftime() argument 2 out of range",
            ));
        }
        Ok(TimeParts {
            year,
            month,
            day,
            hour,
            minute,
            second,
            wday,
            yday,
            isdst,
        })
    }
}

pub(crate) fn asctime_from_parts(parts: TimeParts) -> Result<String, String> {
    const WEEKDAY_ABBR: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    const MONTH_ABBR: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    if !(0..=6).contains(&parts.wday)
        || !(1..=12).contains(&parts.month)
        || !(1..=31).contains(&parts.day)
    {
        return Err("time tuple elements out of range".to_string());
    }
    let wday = WEEKDAY_ABBR[parts.wday as usize];
    let month = MONTH_ABBR[(parts.month - 1) as usize];
    Ok(format!(
        "{wday} {month} {:2} {:02}:{:02}:{:02} {:04}",
        parts.day, parts.hour, parts.minute, parts.second, parts.year
    ))
}

pub(crate) fn parse_mktime_tuple(_py: &PyToken<'_>, tuple_bits: u64) -> Result<TimeParts, u64> {
    let obj = obj_from_bits(tuple_bits);
    let Some(ptr) = obj.as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "Tuple or struct_time argument required",
        ));
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_TUPLE {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "Tuple or struct_time argument required",
            ));
        }
        let elems = seq_vec_ref(ptr);
        if elems.len() != 9 {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "mktime(): illegal time tuple argument",
            ));
        }
        let mut vals = [0i64; 9];
        for (idx, slot) in vals.iter_mut().enumerate() {
            let bits = elems[idx];
            let Some(val) = to_i64(obj_from_bits(bits)) else {
                let type_name = class_name_for_error(type_of_bits(_py, bits));
                let msg = format!("an integer is required (got type {type_name})");
                return Err(raise_exception::<_>(_py, "TypeError", &msg));
            };
            if val < i32::MIN as i64 || val > i32::MAX as i64 {
                return Err(raise_exception::<_>(
                    _py,
                    "OverflowError",
                    "mktime(): argument out of range",
                ));
            }
            *slot = val;
        }
        Ok(TimeParts {
            year: vals[0] as i32,
            month: vals[1] as i32,
            day: vals[2] as i32,
            hour: vals[3] as i32,
            minute: vals[4] as i32,
            second: vals[5] as i32,
            wday: vals[6] as i32,
            yday: vals[7] as i32,
            isdst: vals[8] as i32,
        })
    }
}

pub(crate) fn parse_timegm_tuple(
    _py: &PyToken<'_>,
    tuple_bits: u64,
) -> Result<(i32, i32, i32, i32, i32, i32), u64> {
    let obj = obj_from_bits(tuple_bits);
    let Some(ptr) = obj.as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "Tuple or struct_time argument required",
        ));
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_TUPLE {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "Tuple or struct_time argument required",
            ));
        }
        let elems = seq_vec_ref(ptr);
        if elems.len() < 6 {
            let msg = format!(
                "not enough values to unpack (expected 6, got {})",
                elems.len()
            );
            return Err(raise_exception::<_>(_py, "ValueError", &msg));
        }
        let mut vals = [0i64; 6];
        for (idx, slot) in vals.iter_mut().enumerate() {
            let bits = elems[idx];
            let Some(val) = to_i64(obj_from_bits(bits)) else {
                let type_name = class_name_for_error(type_of_bits(_py, bits));
                let msg = format!("an integer is required (got type {type_name})");
                return Err(raise_exception::<_>(_py, "TypeError", &msg));
            };
            if val < i32::MIN as i64 || val > i32::MAX as i64 {
                return Err(raise_exception::<_>(
                    _py,
                    "OverflowError",
                    "timegm(): argument out of range",
                ));
            }
            *slot = val;
        }
        Ok((
            vals[0] as i32,
            vals[1] as i32,
            vals[2] as i32,
            vals[3] as i32,
            vals[4] as i32,
            vals[5] as i32,
        ))
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn localtime_tm(secs: libc::time_t) -> Result<libc::tm, String> {
    #[cfg(unix)]
    unsafe {
        let mut out = std::mem::zeroed::<libc::tm>();
        if libc::localtime_r(&secs as *const libc::time_t, &mut out).is_null() {
            return Err("localtime failed".to_string());
        }
        Ok(out)
    }
    #[cfg(windows)]
    unsafe {
        let mut out = std::mem::zeroed::<libc::tm>();
        let rc = libc::localtime_s(&mut out as *mut libc::tm, &secs as *const libc::time_t);
        if rc != 0 {
            return Err("localtime failed".to_string());
        }
        Ok(out)
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn gmtime_tm(secs: libc::time_t) -> Result<libc::tm, String> {
    #[cfg(unix)]
    unsafe {
        let mut out = std::mem::zeroed::<libc::tm>();
        if libc::gmtime_r(&secs as *const libc::time_t, &mut out).is_null() {
            return Err("gmtime failed".to_string());
        }
        Ok(out)
    }
    #[cfg(windows)]
    unsafe {
        let mut out = std::mem::zeroed::<libc::tm>();
        let rc = libc::gmtime_s(&mut out as *mut libc::tm, &secs as *const libc::time_t);
        if rc != 0 {
            return Err("gmtime failed".to_string());
        }
        Ok(out)
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn strftime_wasm(format: &str, parts: TimeParts) -> Result<String, String> {
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

    fn push_num(out: &mut String, val: i32, width: usize, pad: char) {
        let mut buf = [pad as u8; 12];
        let mut idx = buf.len();
        let mut n = val.unsigned_abs();
        if n == 0 {
            idx -= 1;
            buf[idx] = b'0';
        } else {
            while n > 0 {
                let digit = (n % 10) as u8;
                idx -= 1;
                buf[idx] = b'0' + digit;
                n /= 10;
            }
        }
        let len = buf.len() - idx;
        let needed = width.saturating_sub(len + if val < 0 { 1 } else { 0 });
        for _ in 0..needed {
            out.push(pad);
        }
        if val < 0 {
            out.push('-');
        }
        out.push_str(std::str::from_utf8(&buf[idx..]).unwrap_or("0"));
    }

    fn jan1_wday_mon0(yday: i32, wday_mon0: i32) -> i32 {
        let offset = (yday - 1).rem_euclid(7);
        (wday_mon0 - offset).rem_euclid(7)
    }

    fn week_number_sun(yday: i32, jan1_wday_mon0: i32) -> i32 {
        let jan1_sun0 = (jan1_wday_mon0 + 1).rem_euclid(7);
        let first_sunday = 1 + (7 - jan1_sun0).rem_euclid(7);
        if yday < first_sunday {
            0
        } else {
            1 + (yday - first_sunday) / 7
        }
    }

    fn week_number_mon(yday: i32, jan1_wday_mon0: i32) -> i32 {
        let first_monday = 1 + (7 - jan1_wday_mon0).rem_euclid(7);
        if yday < first_monday {
            0
        } else {
            1 + (yday - first_monday) / 7
        }
    }

    fn weeks_in_year(year: i32, jan1_wday_mon0: i32) -> i32 {
        let jan1_mon1 = jan1_wday_mon0 + 1;
        if jan1_mon1 == 4 || (is_leap_year(year) && jan1_mon1 == 3) {
            53
        } else {
            52
        }
    }

    fn iso_week_date(year: i32, yday: i32, wday_mon0: i32) -> (i32, i32, i32) {
        let weekday = wday_mon0 + 1;
        let mut week = (yday - weekday + 10) / 7;
        let jan1_wday = jan1_wday_mon0(yday, wday_mon0);
        let mut iso_year = year;
        let max_week = weeks_in_year(year, jan1_wday);
        if week < 1 {
            iso_year -= 1;
            let prev_days = if is_leap_year(iso_year) { 366 } else { 365 };
            let prev_jan1 = (jan1_wday - (prev_days % 7)).rem_euclid(7);
            week = weeks_in_year(iso_year, prev_jan1);
        } else if week > max_week {
            iso_year += 1;
            week = 1;
        }
        (iso_year, week, weekday)
    }

    let mut out = String::with_capacity(format.len() + 16);
    let mut iter = format.chars();
    while let Some(ch) = iter.next() {
        if ch != '%' {
            out.push(ch);
            continue;
        }
        let Some(spec) = iter.next() else {
            out.push('%');
            break;
        };
        match spec {
            '%' => out.push('%'),
            'a' => out.push_str(WEEKDAY_SHORT[parts.wday as usize]),
            'A' => out.push_str(WEEKDAY_LONG[parts.wday as usize]),
            'b' | 'h' => out.push_str(MONTH_SHORT[(parts.month - 1) as usize]),
            'B' => out.push_str(MONTH_LONG[(parts.month - 1) as usize]),
            'C' => {
                let century = parts.year.div_euclid(100);
                push_num(&mut out, century, 2, '0');
            }
            'd' => push_num(&mut out, parts.day, 2, '0'),
            'e' => push_num(&mut out, parts.day, 2, ' '),
            'H' => push_num(&mut out, parts.hour, 2, '0'),
            'I' => {
                let mut hour = parts.hour % 12;
                if hour == 0 {
                    hour = 12;
                }
                push_num(&mut out, hour, 2, '0');
            }
            'k' => push_num(&mut out, parts.hour, 2, ' '),
            'l' => {
                let mut hour = parts.hour % 12;
                if hour == 0 {
                    hour = 12;
                }
                push_num(&mut out, hour, 2, ' ');
            }
            'j' => push_num(&mut out, parts.yday, 3, '0'),
            'm' => push_num(&mut out, parts.month, 2, '0'),
            'M' => push_num(&mut out, parts.minute, 2, '0'),
            'p' => out.push_str(if parts.hour < 12 { "AM" } else { "PM" }),
            'S' => push_num(&mut out, parts.second, 2, '0'),
            'U' => {
                let jan1 = jan1_wday_mon0(parts.yday, parts.wday);
                let week = week_number_sun(parts.yday, jan1);
                push_num(&mut out, week, 2, '0');
            }
            'W' => {
                let jan1 = jan1_wday_mon0(parts.yday, parts.wday);
                let week = week_number_mon(parts.yday, jan1);
                push_num(&mut out, week, 2, '0');
            }
            'w' => {
                let wday_sun0 = (parts.wday + 1).rem_euclid(7);
                push_num(&mut out, wday_sun0, 1, '0');
            }
            'u' => {
                let wday_mon1 = parts.wday + 1;
                push_num(&mut out, wday_mon1, 1, '0');
            }
            'x' => {
                push_num(&mut out, parts.month, 2, '0');
                out.push('/');
                push_num(&mut out, parts.day, 2, '0');
                out.push('/');
                let yy = parts.year.rem_euclid(100);
                push_num(&mut out, yy, 2, '0');
            }
            'X' => {
                push_num(&mut out, parts.hour, 2, '0');
                out.push(':');
                push_num(&mut out, parts.minute, 2, '0');
                out.push(':');
                push_num(&mut out, parts.second, 2, '0');
            }
            'y' => {
                let yy = parts.year.rem_euclid(100);
                push_num(&mut out, yy, 2, '0');
            }
            'Y' => push_num(&mut out, parts.year, 4, '0'),
            'Z' => out.push_str("UTC"),
            'z' => out.push_str("+0000"),
            'c' => {
                out.push_str(WEEKDAY_SHORT[parts.wday as usize]);
                out.push(' ');
                out.push_str(MONTH_SHORT[(parts.month - 1) as usize]);
                out.push(' ');
                push_num(&mut out, parts.day, 2, ' ');
                out.push(' ');
                push_num(&mut out, parts.hour, 2, '0');
                out.push(':');
                push_num(&mut out, parts.minute, 2, '0');
                out.push(':');
                push_num(&mut out, parts.second, 2, '0');
                out.push(' ');
                push_num(&mut out, parts.year, 4, '0');
            }
            'D' => {
                push_num(&mut out, parts.month, 2, '0');
                out.push('/');
                push_num(&mut out, parts.day, 2, '0');
                out.push('/');
                let yy = parts.year.rem_euclid(100);
                push_num(&mut out, yy, 2, '0');
            }
            'F' => {
                push_num(&mut out, parts.year, 4, '0');
                out.push('-');
                push_num(&mut out, parts.month, 2, '0');
                out.push('-');
                push_num(&mut out, parts.day, 2, '0');
            }
            'R' => {
                push_num(&mut out, parts.hour, 2, '0');
                out.push(':');
                push_num(&mut out, parts.minute, 2, '0');
            }
            'r' => {
                let mut hour = parts.hour % 12;
                if hour == 0 {
                    hour = 12;
                }
                push_num(&mut out, hour, 2, '0');
                out.push(':');
                push_num(&mut out, parts.minute, 2, '0');
                out.push(':');
                push_num(&mut out, parts.second, 2, '0');
                out.push(' ');
                out.push_str(if parts.hour < 12 { "AM" } else { "PM" });
            }
            'T' => {
                push_num(&mut out, parts.hour, 2, '0');
                out.push(':');
                push_num(&mut out, parts.minute, 2, '0');
                out.push(':');
                push_num(&mut out, parts.second, 2, '0');
            }
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'G' | 'g' | 'V' => {
                let (iso_year, iso_week, _) = iso_week_date(parts.year, parts.yday, parts.wday);
                match spec {
                    'G' => push_num(&mut out, iso_year, 4, '0'),
                    'g' => {
                        let yy = iso_year.rem_euclid(100);
                        push_num(&mut out, yy, 2, '0');
                    }
                    _ => push_num(&mut out, iso_week, 2, '0'),
                }
            }
            _ => {
                return Err(format!("unsupported strftime directive %{spec}"));
            }
        }
    }
    Ok(out)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn tzname_native() -> Result<(String, String), String> {
    #[cfg(unix)]
    unsafe {
        unsafe extern "C" {
            fn tzset();
            static mut tzname: [*mut libc::c_char; 2];
        }
        tzset();
        let std_ptr = tzname[0];
        let dst_ptr = tzname[1];
        if std_ptr.is_null() || dst_ptr.is_null() {
            return Err("tzname unavailable".to_string());
        }
        let std_name = CStr::from_ptr(std_ptr).to_string_lossy().into_owned();
        let dst_name = CStr::from_ptr(dst_ptr).to_string_lossy().into_owned();
        Ok((std_name, dst_name))
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Time::{
            GetTimeZoneInformation, TIME_ZONE_ID_INVALID, TIME_ZONE_INFORMATION,
        };
        let mut info = TIME_ZONE_INFORMATION {
            Bias: 0,
            StandardName: [0u16; 32],
            StandardDate: std::mem::zeroed(),
            StandardBias: 0,
            DaylightName: [0u16; 32],
            DaylightDate: std::mem::zeroed(),
            DaylightBias: 0,
        };
        let status = GetTimeZoneInformation(&mut info as *mut TIME_ZONE_INFORMATION);
        if status == TIME_ZONE_ID_INVALID {
            return Err("tzname unavailable".to_string());
        }
        let std_len = info
            .StandardName
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(info.StandardName.len());
        let dst_len = info
            .DaylightName
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(info.DaylightName.len());
        let std_name = String::from_utf16_lossy(&info.StandardName[..std_len]);
        let dst_name = String::from_utf16_lossy(&info.DaylightName[..dst_len]);
        Ok((std_name, dst_name))
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn timezone_native() -> Result<i64, String> {
    #[cfg(unix)]
    unsafe {
        unsafe extern "C" {
            fn tzset();
            static mut timezone: libc::c_long;
        }
        tzset();
        Ok(timezone)
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Time::{
            GetTimeZoneInformation, TIME_ZONE_ID_INVALID, TIME_ZONE_INFORMATION,
        };
        let mut info = TIME_ZONE_INFORMATION {
            Bias: 0,
            StandardName: [0u16; 32],
            StandardDate: std::mem::zeroed(),
            StandardBias: 0,
            DaylightName: [0u16; 32],
            DaylightDate: std::mem::zeroed(),
            DaylightBias: 0,
        };
        let status = GetTimeZoneInformation(&mut info as *mut TIME_ZONE_INFORMATION);
        if status == TIME_ZONE_ID_INVALID {
            return Err("timezone unavailable".to_string());
        }
        let bias = info.Bias + info.StandardBias;
        Ok((bias as i64) * 60)
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn daylight_native() -> Result<i64, String> {
    #[cfg(unix)]
    unsafe {
        unsafe extern "C" {
            fn tzset();
            static mut daylight: libc::c_int;
        }
        tzset();
        Ok(if daylight != 0 { 1 } else { 0 })
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Time::{
            GetTimeZoneInformation, TIME_ZONE_ID_INVALID, TIME_ZONE_INFORMATION,
        };
        let mut info = TIME_ZONE_INFORMATION {
            Bias: 0,
            StandardName: [0u16; 32],
            StandardDate: std::mem::zeroed(),
            StandardBias: 0,
            DaylightName: [0u16; 32],
            DaylightDate: std::mem::zeroed(),
            DaylightBias: 0,
        };
        let status = GetTimeZoneInformation(&mut info as *mut TIME_ZONE_INFORMATION);
        if status == TIME_ZONE_ID_INVALID {
            return Err("daylight unavailable".to_string());
        }
        Ok(if info.DaylightDate.wMonth != 0 { 1 } else { 0 })
    }
}

#[cfg(unix)]
pub(crate) fn sample_offset_west_native(year: i32, month: i32, day: i32) -> Result<i64, String> {
    let days = days_from_civil(year, month, day);
    let secs = days.saturating_mul(86_400).saturating_add(12 * 3600);
    offset_west_from_secs(secs)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn altzone_native() -> Result<i64, String> {
    let std_offset = timezone_native()?;
    if daylight_native()? == 0 {
        return Ok(std_offset);
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Time::{
            GetTimeZoneInformation, TIME_ZONE_ID_INVALID, TIME_ZONE_INFORMATION,
        };
        let mut info = TIME_ZONE_INFORMATION {
            Bias: 0,
            StandardName: [0u16; 32],
            StandardDate: std::mem::zeroed(),
            StandardBias: 0,
            DaylightName: [0u16; 32],
            DaylightDate: std::mem::zeroed(),
            DaylightBias: 0,
        };
        let status = GetTimeZoneInformation(&mut info as *mut TIME_ZONE_INFORMATION);
        if status == TIME_ZONE_ID_INVALID {
            return Err("altzone unavailable".to_string());
        }
        let bias = info.Bias + info.DaylightBias;
        Ok((bias as i64) * 60)
    }
    #[cfg(unix)]
    {
        let now = current_epoch_secs_i64()?;
        let local_tm = localtime_tm(now as libc::time_t)?;
        let year = local_tm.tm_year + 1900;
        let jan = sample_offset_west_native(year, 1, 1).unwrap_or(std_offset);
        let jul = sample_offset_west_native(year, 7, 1).unwrap_or(std_offset);
        if jan != std_offset && jul == std_offset {
            return Ok(jan);
        }
        if jul != std_offset && jan == std_offset {
            return Ok(jul);
        }
        if jan != jul {
            return Ok(std::cmp::min(jan, jul));
        }
        Ok(jan)
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn sample_offset_west_wasm(year: i32, month: i32, day: i32) -> Result<i64, String> {
    let days = days_from_civil(year, month, day);
    let secs = days.saturating_mul(86_400).saturating_add(12 * 3600);
    local_offset_west_wasm(secs)
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn daylight_wasm() -> Result<i64, String> {
    let year = time_parts_from_epoch_utc(current_epoch_secs_i64()?).year;
    let jan = sample_offset_west_wasm(year, 1, 1)?;
    let jul = sample_offset_west_wasm(year, 7, 1)?;
    Ok(if jan != jul { 1 } else { 0 })
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn altzone_wasm() -> Result<i64, String> {
    let std_offset = timezone_west_wasm()?;
    if daylight_wasm()? == 0 {
        return Ok(std_offset);
    }
    let year = time_parts_from_epoch_utc(current_epoch_secs_i64()?).year;
    let jan = sample_offset_west_wasm(year, 1, 1).unwrap_or(std_offset);
    let jul = sample_offset_west_wasm(year, 7, 1).unwrap_or(std_offset);
    if jan != std_offset && jul == std_offset {
        return Ok(jan);
    }
    if jul != std_offset && jan == std_offset {
        return Ok(jul);
    }
    if jan != jul {
        return Ok(std::cmp::min(jan, jul));
    }
    Ok(jan)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn mktime_native(parts: TimeParts) -> f64 {
    let mut tm = unsafe { std::mem::zeroed::<libc::tm>() };
    tm.tm_sec = parts.second;
    tm.tm_min = parts.minute;
    tm.tm_hour = parts.hour;
    tm.tm_mday = parts.day;
    tm.tm_mon = parts.month - 1;
    tm.tm_year = parts.year - 1900;
    tm.tm_wday = (parts.wday + 1).rem_euclid(7);
    tm.tm_yday = parts.yday - 1;
    tm.tm_isdst = parts.isdst;
    #[cfg(windows)]
    let out = unsafe { crate::windows_abi::mktime64(&mut tm as *mut libc::tm) };
    #[cfg(not(windows))]
    let out = unsafe { libc::mktime(&mut tm as *mut libc::tm) };
    out as f64
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn mktime_wasm(parts: TimeParts) -> Result<f64, String> {
    let days = days_from_civil(parts.year, parts.month, parts.day);
    let local_secs = days
        .saturating_mul(86_400)
        .saturating_add((parts.hour as i64).saturating_mul(3600))
        .saturating_add((parts.minute as i64).saturating_mul(60))
        .saturating_add(parts.second as i64);
    let std_offset = timezone_west_wasm()?;
    let utc_secs = if parts.isdst > 0 {
        let dst_offset = altzone_wasm().unwrap_or(std_offset);
        local_secs.saturating_add(dst_offset)
    } else if parts.isdst == 0 {
        local_secs.saturating_add(std_offset)
    } else {
        let mut guess = local_secs.saturating_add(std_offset);
        for _ in 0..3 {
            let offset = local_offset_west_wasm(guess).unwrap_or(std_offset);
            let next = local_secs.saturating_add(offset);
            if next == guess {
                break;
            }
            guess = next;
        }
        guess
    };
    Ok(utc_secs as f64)
}
