use super::*;

#[cfg(unix)]
fn local_tm_for_secs(secs: i64) -> Option<libc::tm> {
    let mut tm = std::mem::MaybeUninit::<libc::tm>::zeroed();
    let ts = secs as libc::time_t;
    let ptr = unsafe { libc::localtime_r(&ts, tm.as_mut_ptr()) };
    if ptr.is_null() {
        return None;
    }
    Some(unsafe { tm.assume_init() })
}

#[cfg(unix)]
fn local_noon_epoch(year: i32, month_zero_based: i32, day: i32) -> Option<i64> {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = year - 1900;
    tm.tm_mon = month_zero_based;
    tm.tm_mday = day;
    tm.tm_hour = 12;
    tm.tm_min = 0;
    tm.tm_sec = 0;
    tm.tm_isdst = -1;
    let ts = unsafe { libc::mktime(&mut tm as *mut libc::tm) };
    if ts < 0 {
        return None;
    }
    Some(ts as i64)
}

#[cfg(unix)]
fn local_offset_west_seconds_for(secs: i64) -> Option<i64> {
    let tm = local_tm_for_secs(secs)?;
    Some(-(tm.tm_gmtoff as i64))
}

#[cfg(unix)]
fn tzname_for_secs(secs: i64) -> Option<String> {
    let tm = local_tm_for_secs(secs)?;
    let mut buf = [0 as libc::c_char; 96];
    let fmt = b"%Z\0";
    let written = unsafe {
        libc::strftime(
            buf.as_mut_ptr(),
            buf.len(),
            fmt.as_ptr() as *const libc::c_char,
            &tm as *const libc::tm,
        )
    };
    if written == 0 {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, written as usize) };
    Some(String::from_utf8_lossy(bytes).to_string())
}

#[cfg(unix)]
fn timezone_profile_now() -> Option<(i64, String, String)> {
    let now_secs = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs(),
    )
    .ok()?;
    let year = local_tm_for_secs(now_secs)?.tm_year + 1900;
    let jan_secs = local_noon_epoch(year, 0, 1).unwrap_or(now_secs);
    let jul_secs = local_noon_epoch(year, 6, 1).unwrap_or(now_secs);
    let jan_off = local_offset_west_seconds_for(jan_secs).unwrap_or(0);
    let jul_off = local_offset_west_seconds_for(jul_secs).unwrap_or(0);
    let jan_name = tzname_for_secs(jan_secs).unwrap_or_else(|| "UTC".to_string());
    let jul_name = tzname_for_secs(jul_secs).unwrap_or_else(|| jan_name.clone());
    if jan_off >= jul_off {
        let dst = if jan_off == jul_off {
            jan_name.clone()
        } else {
            jul_name
        };
        Some((jan_off, jan_name, dst))
    } else {
        let dst = if jan_off == jul_off {
            jul_name.clone()
        } else {
            jan_name
        };
        Some((jul_off, jul_name, dst))
    }
}

fn host_time_timezone() -> i64 {
    #[cfg(unix)]
    {
        timezone_profile_now()
            .map(|profile| profile.0)
            .unwrap_or(i64::MIN)
    }
    #[cfg(not(unix))]
    {
        0
    }
}

fn host_time_local_offset(secs: i64) -> i64 {
    #[cfg(unix)]
    {
        local_offset_west_seconds_for(secs).unwrap_or(i64::MIN)
    }
    #[cfg(not(unix))]
    {
        let _ = secs;
        0
    }
}

fn host_time_tzname(which: i32) -> Option<String> {
    if which != 0 && which != 1 {
        return None;
    }
    #[cfg(unix)]
    {
        let profile = timezone_profile_now()?;
        if which == 0 {
            return Some(profile.1);
        }
        Some(profile.2)
    }
    #[cfg(not(unix))]
    {
        Some("UTC".to_string())
    }
}

pub(super) fn define_time_host(
    linker: &mut Linker<HostState>,
    store: &mut Store<HostState>,
) -> Result<()> {
    let timezone = Func::wrap(&mut *store, || -> i64 { host_time_timezone() });
    let local_offset = Func::wrap(&mut *store, |secs: i64| -> i64 {
        host_time_local_offset(secs)
    });
    let tzname = Func::wrap(
        &mut *store,
        |mut caller: Caller<'_, HostState>,
         which: i32,
         buf_ptr: i32,
         buf_cap: i32,
         out_len_ptr: i32|
         -> i32 {
            if out_len_ptr == 0 {
                return -libc::EINVAL;
            }
            if buf_cap < 0 {
                return -libc::EINVAL;
            }
            let Some(label) = host_time_tzname(which) else {
                return -libc::EINVAL;
            };
            let bytes = label.as_bytes();
            let memory = match ensure_memory(&mut caller) {
                Ok(mem) => mem,
                Err(_) => return -libc::ENOSYS,
            };
            if write_u32(&mut caller, &memory, out_len_ptr, bytes.len() as u32).is_err() {
                return -libc::EINVAL;
            }
            let cap = buf_cap as usize;
            if bytes.len() > cap {
                return -libc::ENOMEM;
            }
            if !bytes.is_empty() && write_bytes(&mut caller, &memory, buf_ptr, bytes).is_err() {
                return -libc::EINVAL;
            }
            0
        },
    );
    linker.define(&mut *store, "env", "molt_time_timezone_host", timezone)?;
    linker.define(
        &mut *store,
        "env",
        "molt_time_local_offset_host",
        local_offset,
    )?;
    linker.define(&mut *store, "env", "molt_time_tzname_host", tzname)?;
    Ok(())
}
