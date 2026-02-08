use molt_obj_model::MoltObject;

use crate::{
    alloc_list, alloc_tuple, attr_name_bits_from_bytes, bits_from_ptr, call_callable0,
    dec_ref_bits, exception_pending, inc_ref_bits, int_bits_from_i64, is_truthy,
    maybe_ptr_from_bits, missing_bits, molt_getattr_builtin, molt_is_callable, molt_iter,
    molt_iter_next, monotonic_now_secs, obj_from_bits, ptr_from_bits, raise_exception, release_ptr,
    seq_vec_ref, to_f64, to_i64, IO_EVENT_ERROR, IO_EVENT_READ, IO_EVENT_WRITE, TYPE_ID_TUPLE,
};
#[cfg(not(target_arch = "wasm32"))]
use crate::{raise_os_error, GilReleaseGuard};
#[cfg(not(target_arch = "wasm32"))]
use std::io::ErrorKind;
use std::sync::atomic::{AtomicI64, Ordering as AtomicOrdering};
#[cfg(not(target_arch = "wasm32"))]
use std::thread;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

const SELECT_KIND_POLL: i64 = 0;
const SELECT_KIND_EPOLL: i64 = 1;
const SELECT_KIND_KQUEUE: i64 = 2;
const SELECT_KIND_DEVPOLL: i64 = 3;
const SELECTOR_EVENT_MASK: u32 = IO_EVENT_READ | IO_EVENT_WRITE;
static SELECTOR_FILENO_NEXT: AtomicI64 = AtomicI64::new(10_000);

#[derive(Copy, Clone)]
enum WatchKind {
    Read,
    Write,
    Except,
}

struct SelectWatch {
    obj_bits: u64,
    handle: i64,
    events: u32,
    kind: WatchKind,
}

struct SelectorRegistryEntry {
    fd: i64,
    obj_bits: u64,
    events: u32,
}

struct SelectorRegistry {
    entries: Vec<SelectorRegistryEntry>,
    closed: bool,
    fileno: i64,
    #[allow(dead_code)]
    kind: i64,
}

type SelectorHandle = u64;

#[inline]
fn selector_handle_from_ptr(ptr: *mut u8) -> SelectorHandle {
    bits_from_ptr(ptr)
}

#[inline]
unsafe fn selector_ptr_from_handle(handle: SelectorHandle) -> *mut u8 {
    ptr_from_bits(handle)
}

#[inline]
unsafe fn selector_release_ptr(ptr: *mut u8) {
    release_ptr(ptr);
}

fn collect_iterable(_py: &crate::PyToken<'_>, iterable_bits: u64) -> Result<Vec<u64>, u64> {
    let iter_bits = molt_iter(iterable_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let mut out: Vec<u64> = Vec::new();
    loop {
        let pair_bits = molt_iter_next(iter_bits);
        let Some(pair_ptr) = maybe_ptr_from_bits(pair_bits) else {
            return Err(MoltObject::none().bits());
        };
        unsafe {
            if crate::object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                return Err(MoltObject::none().bits());
            }
        }
        let pair = unsafe { seq_vec_ref(pair_ptr) };
        if pair.len() < 2 {
            return Err(MoltObject::none().bits());
        }
        if is_truthy(_py, obj_from_bits(pair[1])) {
            break;
        }
        let obj_bits = pair[0];
        inc_ref_bits(_py, obj_bits);
        out.push(obj_bits);
    }
    Ok(out)
}

fn get_attr_optional(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    name: &[u8],
) -> Result<Option<u64>, u64> {
    let Some(name_bits) = attr_name_bits_from_bytes(_py, name) else {
        return Err(MoltObject::none().bits());
    };
    let missing = missing_bits(_py);
    let value_bits = molt_getattr_builtin(obj_bits, name_bits, missing);
    dec_ref_bits(_py, name_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if value_bits == missing {
        return Ok(None);
    }
    Ok(Some(value_bits))
}

fn object_to_handle(_py: &crate::PyToken<'_>, obj_bits: u64) -> Result<i64, u64> {
    if let Some(fd) = to_i64(obj_from_bits(obj_bits)) {
        if fd < 0 {
            return Err(raise_exception::<u64>(
                _py,
                "ValueError",
                "invalid file descriptor",
            ));
        }
        return Ok(fd);
    }

    if let Some(fileno_attr_bits) = get_attr_optional(_py, obj_bits, b"fileno")? {
        let callable = is_truthy(_py, obj_from_bits(molt_is_callable(fileno_attr_bits)));
        let raw_bits = if callable {
            let out = unsafe { call_callable0(_py, fileno_attr_bits) };
            dec_ref_bits(_py, fileno_attr_bits);
            if exception_pending(_py) {
                return Err(MoltObject::none().bits());
            }
            out
        } else {
            fileno_attr_bits
        };
        let out = to_i64(obj_from_bits(raw_bits)).ok_or_else(|| {
            raise_exception::<u64>(_py, "ValueError", "invalid file object fileno")
        })?;
        if !obj_from_bits(raw_bits).is_none() {
            dec_ref_bits(_py, raw_bits);
        }
        if out < 0 {
            return Err(raise_exception::<u64>(
                _py,
                "ValueError",
                "invalid file descriptor",
            ));
        }
        return Ok(out);
    }

    if let Some(handle_bits) = get_attr_optional(_py, obj_bits, b"_handle")? {
        let out = to_i64(obj_from_bits(handle_bits))
            .ok_or_else(|| raise_exception::<u64>(_py, "ValueError", "invalid socket handle"))?;
        dec_ref_bits(_py, handle_bits);
        if out < 0 {
            return Err(raise_exception::<u64>(
                _py,
                "ValueError",
                "invalid socket handle",
            ));
        }
        return Ok(out);
    }

    Err(raise_exception::<u64>(
        _py,
        "ValueError",
        "fileobj must be a socket or file descriptor",
    ))
}

fn parse_selector_events(_py: &crate::PyToken<'_>, events_bits: u64) -> Result<u32, u64> {
    let Some(raw) = to_i64(obj_from_bits(events_bits)) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "events must be an integer",
        ));
    };
    if raw <= 0 {
        return Err(raise_exception::<u64>(
            _py,
            "ValueError",
            "events must be non-zero",
        ));
    }
    let events = raw as u32;
    if (events & !SELECTOR_EVENT_MASK) != 0 {
        return Err(raise_exception::<u64>(
            _py,
            "ValueError",
            "unsupported selector events",
        ));
    }
    Ok(events)
}

fn parse_selector_timeout(_py: &crate::PyToken<'_>, timeout_bits: u64) -> Result<Option<f64>, u64> {
    if obj_from_bits(timeout_bits).is_none() {
        return Ok(None);
    }
    let Some(value) = to_f64(obj_from_bits(timeout_bits)) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "timeout must be float or None",
        ));
    };
    if value < 0.0 {
        return Err(raise_exception::<u64>(
            _py,
            "ValueError",
            "timeout must be non-negative",
        ));
    }
    Ok(Some(value))
}

fn selector_state_mut_ptr(
    _py: &crate::PyToken<'_>,
    handle_bits: u64,
) -> Result<*mut SelectorRegistry, u64> {
    let ptr = unsafe { selector_ptr_from_handle(handle_bits) };
    if ptr.is_null() {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "invalid selector handle",
        ));
    }
    let selector = unsafe { &mut *(ptr as *mut SelectorRegistry) };
    if selector.closed {
        return Err(raise_exception::<u64>(
            _py,
            "ValueError",
            "I/O operation on closed selector",
        ));
    }
    Ok(ptr as *mut SelectorRegistry)
}

fn selector_state<'a>(
    _py: &'a crate::PyToken<'_>,
    handle_bits: u64,
) -> Result<&'a SelectorRegistry, u64> {
    let ptr = unsafe { selector_ptr_from_handle(handle_bits) };
    if ptr.is_null() {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "invalid selector handle",
        ));
    }
    let selector = unsafe { &*(ptr as *mut SelectorRegistry) };
    if selector.closed {
        return Err(raise_exception::<u64>(
            _py,
            "ValueError",
            "I/O operation on closed selector",
        ));
    }
    Ok(selector)
}

fn add_watchers(
    _py: &crate::PyToken<'_>,
    watches: &mut Vec<SelectWatch>,
    objects: &[u64],
    events: u32,
    kind: WatchKind,
) -> Result<(), u64> {
    for &obj_bits in objects {
        let handle = object_to_handle(_py, obj_bits)?;
        watches.push(SelectWatch {
            obj_bits,
            handle,
            events,
            kind,
        });
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn poll_once(_py: &crate::PyToken<'_>, watch: &SelectWatch) -> Result<u32, u64> {
    let fd = match i32::try_from(watch.handle) {
        Ok(fd) if fd >= 0 => fd,
        _ => {
            return Err(raise_exception::<u64>(
                _py,
                "ValueError",
                "invalid file descriptor",
            ));
        }
    };

    let mut interest: libc::c_short = 0;
    if (watch.events & IO_EVENT_READ) != 0 {
        interest |= libc::POLLIN as libc::c_short;
    }
    if (watch.events & IO_EVENT_WRITE) != 0 {
        interest |= libc::POLLOUT as libc::c_short;
    }
    if interest == 0 {
        return Ok(0);
    }

    let mut pollfd = libc::pollfd {
        fd,
        events: interest,
        revents: 0,
    };
    let rc = unsafe { libc::poll(&mut pollfd as *mut libc::pollfd, 1, 0) };
    if rc < 0 {
        let err = std::io::Error::last_os_error();
        if err.kind() == ErrorKind::Interrupted {
            return Ok(0);
        }
        return Err(raise_os_error::<u64>(_py, err, "select"));
    }
    if rc == 0 {
        return Ok(0);
    }

    let mut mask = 0u32;
    let revents = pollfd.revents;
    if (revents & (libc::POLLIN | libc::POLLPRI | libc::POLLHUP) as libc::c_short) != 0 {
        mask |= IO_EVENT_READ;
    }
    if (revents & (libc::POLLOUT as libc::c_short)) != 0 {
        mask |= IO_EVENT_WRITE;
    }
    if (revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) as libc::c_short) != 0 {
        mask |= IO_EVENT_ERROR;
    }
    Ok(mask)
}

#[cfg(target_arch = "wasm32")]
fn poll_once(_py: &crate::PyToken<'_>, watch: &SelectWatch) -> Result<u32, u64> {
    let rc = unsafe { crate::molt_socket_poll_host(watch.handle, watch.events) };
    if rc == 0 {
        return Ok(0);
    }
    if rc < 0 {
        return Ok(IO_EVENT_ERROR | IO_EVENT_READ | IO_EVENT_WRITE);
    }
    Ok(rc as u32)
}

fn push_ready(ready: &mut Vec<u64>, obj_bits: u64) {
    ready.push(obj_bits);
}

fn release_bits(_py: &crate::PyToken<'_>, bits: &[u64]) {
    for &bits in bits {
        if !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
        }
    }
}

#[no_mangle]
pub extern "C" fn molt_select_selector_new(kind_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(kind) = to_i64(obj_from_bits(kind_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "selector kind must be an integer");
        };
        if !matches!(
            kind,
            SELECT_KIND_POLL | SELECT_KIND_EPOLL | SELECT_KIND_KQUEUE | SELECT_KIND_DEVPOLL
        ) {
            return raise_exception::<u64>(_py, "ValueError", "unsupported selector kind");
        }
        let fileno = SELECTOR_FILENO_NEXT.fetch_add(1, AtomicOrdering::Relaxed);
        let selector = Box::new(SelectorRegistry {
            entries: Vec::new(),
            closed: false,
            fileno,
            kind,
        });
        selector_handle_from_ptr(Box::into_raw(selector) as *mut u8)
    })
}

#[no_mangle]
pub extern "C" fn molt_select_selector_fileno(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let selector = match selector_state(_py, handle_bits) {
            Ok(selector) => selector,
            Err(err) => return err,
        };
        int_bits_from_i64(_py, selector.fileno)
    })
}

#[no_mangle]
pub extern "C" fn molt_select_selector_register(
    handle_bits: u64,
    fileobj_bits: u64,
    events_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let selector_ptr = match selector_state_mut_ptr(_py, handle_bits) {
            Ok(selector) => selector,
            Err(err) => return err,
        };
        let selector = unsafe { &mut *selector_ptr };
        let events = match parse_selector_events(_py, events_bits) {
            Ok(events) => events,
            Err(err) => return err,
        };
        let fd = match object_to_handle(_py, fileobj_bits) {
            Ok(fd) => fd,
            Err(err) => return err,
        };
        if selector.entries.iter().any(|entry| entry.fd == fd) {
            return raise_exception::<u64>(_py, "KeyError", "fd is already registered");
        }
        inc_ref_bits(_py, fileobj_bits);
        selector.entries.push(SelectorRegistryEntry {
            fd,
            obj_bits: fileobj_bits,
            events,
        });
        int_bits_from_i64(_py, fd)
    })
}

#[no_mangle]
pub extern "C" fn molt_select_selector_unregister(handle_bits: u64, fd_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let selector_ptr = match selector_state_mut_ptr(_py, handle_bits) {
            Ok(selector) => selector,
            Err(err) => return err,
        };
        let selector = unsafe { &mut *selector_ptr };
        let Some(fd) = to_i64(obj_from_bits(fd_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "fd must be an integer");
        };
        let Some(pos) = selector.entries.iter().position(|entry| entry.fd == fd) else {
            return raise_exception::<u64>(_py, "KeyError", "fd is not registered");
        };
        let entry = selector.entries.swap_remove(pos);
        dec_ref_bits(_py, entry.obj_bits);
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_select_selector_modify(
    handle_bits: u64,
    fd_bits: u64,
    events_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let selector_ptr = match selector_state_mut_ptr(_py, handle_bits) {
            Ok(selector) => selector,
            Err(err) => return err,
        };
        let selector = unsafe { &mut *selector_ptr };
        let Some(fd) = to_i64(obj_from_bits(fd_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "fd must be an integer");
        };
        let events = match parse_selector_events(_py, events_bits) {
            Ok(events) => events,
            Err(err) => return err,
        };
        let Some(entry) = selector.entries.iter_mut().find(|entry| entry.fd == fd) else {
            return raise_exception::<u64>(_py, "KeyError", "fd is not registered");
        };
        entry.events = events;
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_select_selector_poll(handle_bits: u64, timeout_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let selector = match selector_state(_py, handle_bits) {
            Ok(selector) => selector,
            Err(err) => return err,
        };
        let timeout = match parse_selector_timeout(_py, timeout_bits) {
            Ok(timeout) => timeout,
            Err(err) => return err,
        };
        if selector.entries.is_empty() {
            if let Some(timeout) = timeout {
                if timeout > 0.0 {
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        let _release = GilReleaseGuard::new();
                        thread::sleep(Duration::from_secs_f64(timeout));
                    }
                    #[cfg(target_arch = "wasm32")]
                    {
                        std::hint::spin_loop();
                    }
                }
            }
            let ptr = alloc_list(_py, &[]);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }

        let deadline = timeout.map(|value| monotonic_now_secs(_py) + value);
        let mut ready_pairs: Vec<(i64, u32)> = Vec::new();

        loop {
            for entry in &selector.entries {
                let watch = SelectWatch {
                    obj_bits: entry.obj_bits,
                    handle: entry.fd,
                    events: entry.events,
                    kind: WatchKind::Read,
                };
                let mask = match poll_once(_py, &watch) {
                    Ok(mask) => mask,
                    Err(err) => return err,
                };
                if mask != 0 {
                    ready_pairs.push((entry.fd, mask));
                }
            }
            if !ready_pairs.is_empty() {
                break;
            }
            if timeout == Some(0.0) {
                break;
            }
            if let Some(deadline) = deadline {
                let now = monotonic_now_secs(_py);
                if now >= deadline {
                    break;
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let sleep_secs = match deadline {
                    Some(deadline) => {
                        let remaining = deadline - monotonic_now_secs(_py);
                        if remaining <= 0.0 {
                            0.0
                        } else {
                            remaining.min(0.01)
                        }
                    }
                    None => 0.01,
                };
                if sleep_secs > 0.0 {
                    let _release = GilReleaseGuard::new();
                    thread::sleep(Duration::from_secs_f64(sleep_secs));
                }
            }
            #[cfg(target_arch = "wasm32")]
            {
                std::hint::spin_loop();
            }
        }

        let mut tuple_bits: Vec<u64> = Vec::with_capacity(ready_pairs.len());
        for (fd, mask) in ready_pairs {
            let fd_bits = int_bits_from_i64(_py, fd);
            if obj_from_bits(fd_bits).is_none() {
                release_bits(_py, &tuple_bits);
                return MoltObject::none().bits();
            }
            let mask_bits = int_bits_from_i64(_py, mask as i64);
            if obj_from_bits(mask_bits).is_none() {
                dec_ref_bits(_py, fd_bits);
                release_bits(_py, &tuple_bits);
                return MoltObject::none().bits();
            }
            let tup_ptr = alloc_tuple(_py, &[fd_bits, mask_bits]);
            dec_ref_bits(_py, fd_bits);
            dec_ref_bits(_py, mask_bits);
            if tup_ptr.is_null() {
                release_bits(_py, &tuple_bits);
                return MoltObject::none().bits();
            }
            tuple_bits.push(MoltObject::from_ptr(tup_ptr).bits());
        }
        let list_ptr = alloc_list(_py, &tuple_bits);
        release_bits(_py, &tuple_bits);
        if list_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_select_selector_close(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = unsafe { selector_ptr_from_handle(handle_bits) };
        if ptr.is_null() {
            return raise_exception::<u64>(_py, "TypeError", "invalid selector handle");
        }
        let selector = unsafe { &mut *(ptr as *mut SelectorRegistry) };
        if selector.closed {
            return MoltObject::none().bits();
        }
        for entry in selector.entries.drain(..) {
            dec_ref_bits(_py, entry.obj_bits);
        }
        selector.closed = true;
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_select_selector_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = unsafe { selector_ptr_from_handle(handle_bits) };
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        let mut selector = unsafe { Box::from_raw(ptr as *mut SelectorRegistry) };
        if !selector.closed {
            for entry in selector.entries.drain(..) {
                dec_ref_bits(_py, entry.obj_bits);
            }
            selector.closed = true;
        }
        unsafe { selector_release_ptr(ptr) };
        drop(selector);
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_select_select(
    rlist_bits: u64,
    wlist_bits: u64,
    xlist_bits: u64,
    timeout_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let r_objects = match collect_iterable(_py, rlist_bits) {
            Ok(values) => values,
            Err(err) => return err,
        };
        let w_objects = match collect_iterable(_py, wlist_bits) {
            Ok(values) => values,
            Err(err) => {
                release_bits(_py, &r_objects);
                return err;
            }
        };
        let x_objects = match collect_iterable(_py, xlist_bits) {
            Ok(values) => values,
            Err(err) => {
                release_bits(_py, &r_objects);
                release_bits(_py, &w_objects);
                return err;
            }
        };

        let mut watches: Vec<SelectWatch> =
            Vec::with_capacity(r_objects.len() + w_objects.len() + x_objects.len());
        if let Err(err) = add_watchers(
            _py,
            &mut watches,
            &r_objects,
            IO_EVENT_READ,
            WatchKind::Read,
        ) {
            release_bits(_py, &r_objects);
            release_bits(_py, &w_objects);
            release_bits(_py, &x_objects);
            return err;
        }
        if let Err(err) = add_watchers(
            _py,
            &mut watches,
            &w_objects,
            IO_EVENT_WRITE,
            WatchKind::Write,
        ) {
            release_bits(_py, &r_objects);
            release_bits(_py, &w_objects);
            release_bits(_py, &x_objects);
            return err;
        }
        if let Err(err) = add_watchers(
            _py,
            &mut watches,
            &x_objects,
            IO_EVENT_READ | IO_EVENT_WRITE,
            WatchKind::Except,
        ) {
            release_bits(_py, &r_objects);
            release_bits(_py, &w_objects);
            release_bits(_py, &x_objects);
            return err;
        }

        let timeout = if obj_from_bits(timeout_bits).is_none() {
            None
        } else {
            let Some(value) = to_f64(obj_from_bits(timeout_bits)) else {
                release_bits(_py, &r_objects);
                release_bits(_py, &w_objects);
                release_bits(_py, &x_objects);
                return raise_exception::<u64>(_py, "TypeError", "timeout must be float or None");
            };
            if value < 0.0 {
                release_bits(_py, &r_objects);
                release_bits(_py, &w_objects);
                release_bits(_py, &x_objects);
                return raise_exception::<u64>(_py, "ValueError", "timeout must be non-negative");
            }
            Some(value)
        };

        let mut ready_r: Vec<u64> = Vec::new();
        let mut ready_w: Vec<u64> = Vec::new();
        let mut ready_x: Vec<u64> = Vec::new();
        let deadline = timeout.map(|value| monotonic_now_secs(_py) + value);

        loop {
            for watch in &watches {
                let mask = match poll_once(_py, watch) {
                    Ok(mask) => mask,
                    Err(err) => {
                        release_bits(_py, &r_objects);
                        release_bits(_py, &w_objects);
                        release_bits(_py, &x_objects);
                        release_bits(_py, &ready_r);
                        release_bits(_py, &ready_w);
                        release_bits(_py, &ready_x);
                        return err;
                    }
                };
                if mask == 0 {
                    continue;
                }
                match watch.kind {
                    WatchKind::Read => {
                        if (mask & IO_EVENT_READ) != 0 {
                            inc_ref_bits(_py, watch.obj_bits);
                            push_ready(&mut ready_r, watch.obj_bits);
                        }
                    }
                    WatchKind::Write => {
                        if (mask & IO_EVENT_WRITE) != 0 {
                            inc_ref_bits(_py, watch.obj_bits);
                            push_ready(&mut ready_w, watch.obj_bits);
                        }
                    }
                    WatchKind::Except => {
                        if (mask & IO_EVENT_ERROR) != 0 {
                            inc_ref_bits(_py, watch.obj_bits);
                            push_ready(&mut ready_x, watch.obj_bits);
                        }
                    }
                }
            }

            if !ready_r.is_empty() || !ready_w.is_empty() || !ready_x.is_empty() {
                break;
            }
            if timeout == Some(0.0) {
                break;
            }
            if let Some(deadline) = deadline {
                let now = monotonic_now_secs(_py);
                if now >= deadline {
                    break;
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let sleep_secs = match deadline {
                    Some(deadline) => {
                        let remaining = deadline - monotonic_now_secs(_py);
                        if remaining <= 0.0 {
                            0.0
                        } else {
                            remaining.min(0.01)
                        }
                    }
                    None => 0.01,
                };
                if sleep_secs > 0.0 {
                    let _release = GilReleaseGuard::new();
                    thread::sleep(Duration::from_secs_f64(sleep_secs));
                }
            }
            #[cfg(target_arch = "wasm32")]
            {
                std::hint::spin_loop();
            }
        }

        let r_ptr = alloc_list(_py, &ready_r);
        if r_ptr.is_null() {
            release_bits(_py, &r_objects);
            release_bits(_py, &w_objects);
            release_bits(_py, &x_objects);
            release_bits(_py, &ready_r);
            release_bits(_py, &ready_w);
            release_bits(_py, &ready_x);
            return MoltObject::none().bits();
        }
        let r_bits = MoltObject::from_ptr(r_ptr).bits();

        let w_ptr = alloc_list(_py, &ready_w);
        if w_ptr.is_null() {
            dec_ref_bits(_py, r_bits);
            release_bits(_py, &r_objects);
            release_bits(_py, &w_objects);
            release_bits(_py, &x_objects);
            release_bits(_py, &ready_r);
            release_bits(_py, &ready_w);
            release_bits(_py, &ready_x);
            return MoltObject::none().bits();
        }
        let w_bits = MoltObject::from_ptr(w_ptr).bits();

        let x_ptr = alloc_list(_py, &ready_x);
        if x_ptr.is_null() {
            dec_ref_bits(_py, r_bits);
            dec_ref_bits(_py, w_bits);
            release_bits(_py, &r_objects);
            release_bits(_py, &w_objects);
            release_bits(_py, &x_objects);
            release_bits(_py, &ready_r);
            release_bits(_py, &ready_w);
            release_bits(_py, &ready_x);
            return MoltObject::none().bits();
        }
        let x_bits = MoltObject::from_ptr(x_ptr).bits();

        let tup_ptr = alloc_tuple(_py, &[r_bits, w_bits, x_bits]);
        dec_ref_bits(_py, r_bits);
        dec_ref_bits(_py, w_bits);
        dec_ref_bits(_py, x_bits);

        release_bits(_py, &r_objects);
        release_bits(_py, &w_objects);
        release_bits(_py, &x_objects);
        release_bits(_py, &ready_r);
        release_bits(_py, &ready_w);
        release_bits(_py, &ready_x);

        if tup_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tup_ptr).bits()
    })
}
