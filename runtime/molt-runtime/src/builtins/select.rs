use molt_obj_model::MoltObject;

use crate::audit::{AuditArgs, audit_capability_decision};
#[cfg(not(target_arch = "wasm32"))]
use crate::{GilReleaseGuard, raise_os_error};
use crate::{
    IO_EVENT_ERROR, IO_EVENT_READ, IO_EVENT_WRITE, TYPE_ID_LIST, TYPE_ID_TUPLE,
    alloc_dict_with_pairs, alloc_list, alloc_string, alloc_tuple, attr_name_bits_from_bytes,
    bits_from_ptr, call_callable0, dec_ref_bits, exception_pending, inc_ref_bits,
    int_bits_from_i64, is_truthy, maybe_ptr_from_bits, missing_bits, molt_getattr_builtin,
    molt_is_callable, molt_iter, molt_iter_next, monotonic_now_secs, obj_from_bits, ptr_from_bits,
    raise_exception, release_ptr, seq_vec_ref, to_f64, to_i64,
};
use std::collections::HashMap;
use std::collections::hash_map::Entry as HashMapEntry;
#[cfg(not(target_arch = "wasm32"))]
use std::io::ErrorKind;
use std::sync::atomic::{AtomicI64, Ordering as AtomicOrdering};
#[cfg(not(target_arch = "wasm32"))]
use std::thread;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

/// Checks the "net" capability and emits an audit event. Returns `Err(bits)`
/// with a PermissionError if the capability is denied.
#[inline]
fn require_net_capability(_py: &crate::PyToken<'_>, operation: &'static str) -> Result<(), u64> {
    let allowed = crate::has_capability(_py, "net");
    audit_capability_decision(operation, "net", AuditArgs::None, allowed);
    if !allowed {
        return Err(raise_exception::<u64>(
            _py,
            "PermissionError",
            "missing net capability for select/poll operations",
        ));
    }
    Ok(())
}

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
    obj_bits: u64,
    events: u32,
}

struct SelectorRegistry {
    entries: HashMap<i64, SelectorRegistryEntry>,
    obj_to_fd: HashMap<u64, i64>,
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
    if let Some(iterable_ptr) = obj_from_bits(iterable_bits).as_ptr() {
        let type_id = unsafe { crate::object_type_id(iterable_ptr) };
        if matches!(type_id, TYPE_ID_LIST | TYPE_ID_TUPLE) {
            let seq = unsafe { seq_vec_ref(iterable_ptr) };
            let mut out: Vec<u64> = Vec::with_capacity(seq.len());
            for &obj_bits in seq {
                inc_ref_bits(_py, obj_bits);
                out.push(obj_bits);
            }
            return Ok(out);
        }
    }

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

fn select_construct_private_object(
    _py: &crate::PyToken<'_>,
    class_name: &[u8],
) -> Result<u64, u64> {
    let module_name_ptr = alloc_string(_py, b"select");
    if module_name_ptr.is_null() {
        return Err(MoltObject::none().bits());
    }
    let module_name_bits = MoltObject::from_ptr(module_name_ptr).bits();
    let module_bits = crate::molt_module_import(module_name_bits);
    dec_ref_bits(_py, module_name_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let Some(class_name_bits) = attr_name_bits_from_bytes(_py, class_name) else {
        dec_ref_bits(_py, module_bits);
        return Err(MoltObject::none().bits());
    };
    let missing = missing_bits(_py);
    let class_bits = molt_getattr_builtin(module_bits, class_name_bits, missing);
    dec_ref_bits(_py, class_name_bits);
    dec_ref_bits(_py, module_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if class_bits == missing {
        return Err(raise_exception::<u64>(
            _py,
            "RuntimeError",
            "select backend class is unavailable",
        ));
    }
    if !is_truthy(_py, obj_from_bits(molt_is_callable(class_bits))) {
        dec_ref_bits(_py, class_bits);
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "select backend class is not callable",
        ));
    }
    let out_bits = unsafe { call_callable0(_py, class_bits) };
    dec_ref_bits(_py, class_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(out_bits)
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

fn selector_release_obj_bits(_py: &crate::PyToken<'_>, obj_bits: u64) {
    if !obj_from_bits(obj_bits).is_none() {
        dec_ref_bits(_py, obj_bits);
    }
}

fn selector_parse_fd(_py: &crate::PyToken<'_>, fd_bits: u64) -> Result<i64, u64> {
    let Some(fd) = to_i64(obj_from_bits(fd_bits)) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "fd must be an integer",
        ));
    };
    if fd < 0 {
        return Err(raise_exception::<u64>(
            _py,
            "ValueError",
            "invalid file descriptor",
        ));
    }
    Ok(fd)
}

fn selector_resolve_fd_from_obj(
    _py: &crate::PyToken<'_>,
    selector: &mut SelectorRegistry,
    fileobj_bits: u64,
) -> Result<i64, u64> {
    if let Some(fd) = selector.obj_to_fd.get(&fileobj_bits).copied() {
        return Ok(fd);
    }
    let fd = object_to_handle(_py, fileobj_bits)?;
    if selector
        .entries
        .get(&fd)
        .is_some_and(|entry| entry.obj_bits == fileobj_bits)
    {
        selector.obj_to_fd.insert(fileobj_bits, fd);
    }
    Ok(fd)
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
fn poll_interest_from_events(events: u32) -> libc::c_short {
    let mut interest: libc::c_short = 0;
    if (events & IO_EVENT_READ) != 0 {
        interest |= libc::POLLIN as libc::c_short;
    }
    if (events & IO_EVENT_WRITE) != 0 {
        interest |= libc::POLLOUT as libc::c_short;
    }
    interest
}

#[cfg(not(target_arch = "wasm32"))]
fn poll_mask_from_revents(revents: libc::c_short) -> u32 {
    let mut mask = 0u32;
    if (revents & (libc::POLLIN | libc::POLLPRI | libc::POLLHUP) as libc::c_short) != 0 {
        mask |= IO_EVENT_READ;
    }
    if (revents & (libc::POLLOUT as libc::c_short)) != 0 {
        mask |= IO_EVENT_WRITE;
    }
    if (revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) as libc::c_short) != 0 {
        mask |= IO_EVENT_ERROR;
    }
    mask
}

#[cfg(not(target_arch = "wasm32"))]
fn poll_timeout_ms(_py: &crate::PyToken<'_>, timeout: Option<f64>, deadline: Option<f64>) -> i32 {
    match timeout {
        None => -1,
        Some(value) if value <= 0.0 => 0,
        Some(_) => {
            let remaining = deadline
                .map(|end| end - monotonic_now_secs(_py))
                .unwrap_or(0.0);
            if remaining <= 0.0 {
                0
            } else {
                let millis = (remaining * 1000.0).ceil();
                if millis >= i32::MAX as f64 {
                    i32::MAX
                } else {
                    millis as i32
                }
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn poll_masks_batch(
    _py: &crate::PyToken<'_>,
    watches: &[SelectWatch],
    timeout_ms: i32,
) -> Result<Vec<u32>, u64> {
    let mut masks = vec![0u32; watches.len()];
    if watches.is_empty() {
        return Ok(masks);
    }

    let mut pollfds: Vec<libc::pollfd> = Vec::with_capacity(watches.len());
    let mut watch_poll_index: Vec<usize> = vec![usize::MAX; watches.len()];
    if watches.len() <= 16 {
        for (index, watch) in watches.iter().enumerate() {
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
            let interest = poll_interest_from_events(watch.events);
            if interest == 0 {
                continue;
            }
            if let Some(poll_index) = pollfds.iter().position(|pollfd| pollfd.fd == fd) {
                pollfds[poll_index].events |= interest;
                watch_poll_index[index] = poll_index;
                continue;
            }
            let poll_index = pollfds.len();
            pollfds.push(libc::pollfd {
                fd,
                events: interest,
                revents: 0,
            });
            watch_poll_index[index] = poll_index;
        }
    } else {
        let mut fd_to_poll_index: HashMap<i32, usize> = HashMap::with_capacity(watches.len());
        for (index, watch) in watches.iter().enumerate() {
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
            let interest = poll_interest_from_events(watch.events);
            if interest == 0 {
                continue;
            }
            match fd_to_poll_index.entry(fd) {
                HashMapEntry::Occupied(entry) => {
                    let poll_index = *entry.get();
                    pollfds[poll_index].events |= interest;
                    watch_poll_index[index] = poll_index;
                }
                HashMapEntry::Vacant(entry) => {
                    let poll_index = pollfds.len();
                    pollfds.push(libc::pollfd {
                        fd,
                        events: interest,
                        revents: 0,
                    });
                    entry.insert(poll_index);
                    watch_poll_index[index] = poll_index;
                }
            }
        }
    }
    if pollfds.is_empty() {
        return Ok(masks);
    }

    let rc = {
        let _release = GilReleaseGuard::new();
        unsafe {
            libc::poll(
                pollfds.as_mut_ptr(),
                pollfds.len() as libc::nfds_t,
                timeout_ms,
            )
        }
    };
    if rc < 0 {
        let err = std::io::Error::last_os_error();
        if err.kind() == ErrorKind::Interrupted {
            return Ok(masks);
        }
        return Err(raise_os_error::<u64>(_py, err, "select"));
    }
    if rc == 0 {
        return Ok(masks);
    }

    for (watch_index, poll_index) in watch_poll_index.into_iter().enumerate() {
        if poll_index != usize::MAX {
            let mask = poll_mask_from_revents(pollfds[poll_index].revents);
            if mask != 0 {
                masks[watch_index] = mask;
            }
        }
    }
    Ok(masks)
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_select_constants() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mut pairs: Vec<u64> = Vec::new();
        let mut owned_bits: Vec<u64> = Vec::new();
        macro_rules! push_const {
            ($name:expr, $value:expr) => {{
                let key_ptr = alloc_string(_py, $name.as_bytes());
                if key_ptr.is_null() {
                    for bits in owned_bits {
                        dec_ref_bits(_py, bits);
                    }
                    return MoltObject::none().bits();
                }
                let key_bits = MoltObject::from_ptr(key_ptr).bits();
                let value_bits = MoltObject::from_int($value).bits();
                pairs.push(key_bits);
                pairs.push(value_bits);
                owned_bits.push(key_bits);
                owned_bits.push(value_bits);
            }};
        }

        #[cfg(target_os = "windows")]
        push_const!("_HAS_POLL", 0);
        #[cfg(not(target_os = "windows"))]
        push_const!("_HAS_POLL", 1);

        #[cfg(any(target_os = "linux", target_os = "android"))]
        push_const!("_HAS_EPOLL", 1);
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        push_const!("_HAS_EPOLL", 0);

        #[cfg(any(target_os = "solaris", target_os = "illumos"))]
        push_const!("_HAS_DEVPOLL", 1);
        #[cfg(not(any(target_os = "solaris", target_os = "illumos")))]
        push_const!("_HAS_DEVPOLL", 0);

        #[cfg(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))]
        push_const!("_HAS_KQUEUE", 1);
        #[cfg(not(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        )))]
        push_const!("_HAS_KQUEUE", 0);

        #[cfg(not(any(target_os = "windows", target_arch = "wasm32")))]
        {
            push_const!("POLLIN", libc::POLLIN as i64);
            push_const!("POLLPRI", libc::POLLPRI as i64);
            push_const!("POLLOUT", libc::POLLOUT as i64);
            push_const!("POLLERR", libc::POLLERR as i64);
            push_const!("POLLHUP", libc::POLLHUP as i64);
            push_const!("POLLNVAL", libc::POLLNVAL as i64);
            push_const!("PIPE_BUF", libc::PIPE_BUF as i64);
        }

        #[cfg(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly",
            target_os = "linux",
            target_os = "android"
        ))]
        {
            push_const!("POLLRDNORM", libc::POLLRDNORM as i64);
            push_const!("POLLRDBAND", libc::POLLRDBAND as i64);
            push_const!("POLLWRNORM", libc::POLLWRNORM as i64);
            push_const!("POLLWRBAND", libc::POLLWRBAND as i64);
        }

        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            push_const!("EPOLLIN", libc::EPOLLIN as i64);
            push_const!("EPOLLPRI", libc::EPOLLPRI as i64);
            push_const!("EPOLLOUT", libc::EPOLLOUT as i64);
            push_const!("EPOLLERR", libc::EPOLLERR as i64);
            push_const!("EPOLLHUP", libc::EPOLLHUP as i64);
            push_const!("EPOLLRDHUP", libc::EPOLLRDHUP as i64);
            push_const!("EPOLLET", libc::EPOLLET as i64);
            push_const!("EPOLLONESHOT", libc::EPOLLONESHOT as i64);
            push_const!("EPOLLEXCLUSIVE", libc::EPOLLEXCLUSIVE as i64);
            push_const!("EPOLLWAKEUP", libc::EPOLLWAKEUP as i64);
            push_const!("EPOLLMSG", libc::EPOLLMSG as i64);
            push_const!("EPOLL_CTL_ADD", libc::EPOLL_CTL_ADD as i64);
            push_const!("EPOLL_CTL_DEL", libc::EPOLL_CTL_DEL as i64);
            push_const!("EPOLL_CTL_MOD", libc::EPOLL_CTL_MOD as i64);
        }

        #[cfg(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))]
        {
            push_const!("KQ_FILTER_READ", libc::EVFILT_READ as i64);
            push_const!("KQ_FILTER_WRITE", libc::EVFILT_WRITE as i64);
            push_const!("KQ_FILTER_AIO", libc::EVFILT_AIO as i64);
            push_const!("KQ_FILTER_VNODE", libc::EVFILT_VNODE as i64);
            push_const!("KQ_FILTER_PROC", libc::EVFILT_PROC as i64);
            push_const!("KQ_FILTER_SIGNAL", libc::EVFILT_SIGNAL as i64);
            push_const!("KQ_FILTER_TIMER", libc::EVFILT_TIMER as i64);
            push_const!("KQ_EV_ADD", libc::EV_ADD as i64);
            push_const!("KQ_EV_DELETE", libc::EV_DELETE as i64);
            push_const!("KQ_EV_ENABLE", libc::EV_ENABLE as i64);
            push_const!("KQ_EV_DISABLE", libc::EV_DISABLE as i64);
            push_const!("KQ_EV_CLEAR", libc::EV_CLEAR as i64);
            push_const!("KQ_EV_ONESHOT", libc::EV_ONESHOT as i64);
            push_const!("KQ_EV_EOF", libc::EV_EOF as i64);
            push_const!("KQ_EV_ERROR", libc::EV_ERROR as i64);
            push_const!("KQ_EV_FLAG1", libc::EV_FLAG1 as i64);
            push_const!("KQ_EV_SYSFLAGS", libc::EV_SYSFLAGS as i64);
            push_const!("KQ_NOTE_DELETE", libc::NOTE_DELETE as i64);
            push_const!("KQ_NOTE_WRITE", libc::NOTE_WRITE as i64);
            push_const!("KQ_NOTE_EXTEND", libc::NOTE_EXTEND as i64);
            push_const!("KQ_NOTE_ATTRIB", libc::NOTE_ATTRIB as i64);
            push_const!("KQ_NOTE_LINK", libc::NOTE_LINK as i64);
            push_const!("KQ_NOTE_RENAME", libc::NOTE_RENAME as i64);
            push_const!("KQ_NOTE_REVOKE", libc::NOTE_REVOKE as i64);
            push_const!("KQ_NOTE_TRACK", libc::NOTE_TRACK as i64);
            push_const!("KQ_NOTE_TRACKERR", libc::NOTE_TRACKERR as i64);
            push_const!("KQ_NOTE_CHILD", libc::NOTE_CHILD as i64);
            push_const!("KQ_NOTE_FORK", libc::NOTE_FORK as i64);
            push_const!("KQ_NOTE_EXEC", libc::NOTE_EXEC as i64);
            push_const!("KQ_NOTE_EXIT", libc::NOTE_EXIT as i64);
            push_const!("KQ_NOTE_PDATAMASK", libc::NOTE_PDATAMASK as i64);
            push_const!("KQ_NOTE_PCTRLMASK", (libc::NOTE_PCTRLMASK as i32) as i64);
            push_const!("KQ_NOTE_LOWAT", libc::NOTE_LOWAT as i64);
        }

        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned_bits {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(dict_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_select_poll() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Err(err) = require_net_capability(_py, "select.poll") {
            return err;
        }
        match select_construct_private_object(_py, b"_PollObject") {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_select_epoll(sizehint_bits: u64, flags_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Err(err) = require_net_capability(_py, "select.epoll") {
            return err;
        }
        if to_i64(obj_from_bits(sizehint_bits)).is_none() {
            return raise_exception::<u64>(_py, "TypeError", "sizehint must be an integer");
        }
        if to_i64(obj_from_bits(flags_bits)).is_none() {
            return raise_exception::<u64>(_py, "TypeError", "flags must be an integer");
        }
        match select_construct_private_object(_py, b"_EpollObject") {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_select_devpoll() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Err(err) = require_net_capability(_py, "select.devpoll") {
            return err;
        }
        match select_construct_private_object(_py, b"_DevpollObject") {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_select_fileno(fileobj_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        match object_to_handle(_py, fileobj_bits) {
            Ok(fd) => int_bits_from_i64(_py, fd),
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_select_default_selector_kind() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let kind = if cfg!(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        )) {
            SELECT_KIND_KQUEUE
        } else if cfg!(any(target_os = "linux", target_os = "android")) {
            SELECT_KIND_EPOLL
        } else if cfg!(any(target_os = "solaris", target_os = "illumos")) {
            SELECT_KIND_DEVPOLL
        } else if cfg!(target_os = "windows") {
            -1
        } else {
            SELECT_KIND_POLL
        };
        int_bits_from_i64(_py, kind)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_select_backend_available(kind_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(kind) = to_i64(obj_from_bits(kind_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "selector kind must be an integer");
        };
        let available = match kind {
            SELECT_KIND_POLL => cfg!(not(target_os = "windows")),
            SELECT_KIND_EPOLL => cfg!(any(target_os = "linux", target_os = "android")),
            SELECT_KIND_KQUEUE => cfg!(any(
                target_os = "macos",
                target_os = "freebsd",
                target_os = "openbsd",
                target_os = "netbsd",
                target_os = "dragonfly"
            )),
            SELECT_KIND_DEVPOLL => cfg!(any(target_os = "solaris", target_os = "illumos")),
            _ => false,
        };
        MoltObject::from_bool(available).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_select_selector_new(kind_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Err(err) = require_net_capability(_py, "select.selector_new") {
            return err;
        }
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
            entries: HashMap::new(),
            obj_to_fd: HashMap::new(),
            closed: false,
            fileno,
            kind,
        });
        selector_handle_from_ptr(Box::into_raw(selector) as *mut u8)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_select_selector_fileno(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let selector = match selector_state(_py, handle_bits) {
            Ok(selector) => selector,
            Err(err) => return err,
        };
        int_bits_from_i64(_py, selector.fileno)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_select_selector_len(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let selector = match selector_state(_py, handle_bits) {
            Ok(selector) => selector,
            Err(err) => return err,
        };
        let count = i64::try_from(selector.entries.len()).unwrap_or(i64::MAX);
        int_bits_from_i64(_py, count)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_select_selector_events(handle_bits: u64, fd_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let selector = match selector_state(_py, handle_bits) {
            Ok(selector) => selector,
            Err(err) => return err,
        };
        let fd = match selector_parse_fd(_py, fd_bits) {
            Ok(fd) => fd,
            Err(err) => return err,
        };
        let events = selector
            .entries
            .get(&fd)
            .map(|entry| i64::from(entry.events))
            .unwrap_or(0);
        int_bits_from_i64(_py, events)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_select_selector_register(
    handle_bits: u64,
    fileobj_bits: u64,
    events_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Err(err) = require_net_capability(_py, "select.selector_register") {
            return err;
        }
        let selector_ptr = match selector_state_mut_ptr(_py, handle_bits) {
            Ok(selector) => selector,
            Err(err) => return err,
        };
        let selector = unsafe { &mut *selector_ptr };
        let events = match parse_selector_events(_py, events_bits) {
            Ok(events) => events,
            Err(err) => return err,
        };
        if let Some(existing_fd) = selector.obj_to_fd.get(&fileobj_bits).copied()
            && selector.entries.contains_key(&existing_fd)
        {
            return raise_exception::<u64>(_py, "KeyError", "fd is already registered");
        }
        let fd = match object_to_handle(_py, fileobj_bits) {
            Ok(fd) => fd,
            Err(err) => return err,
        };
        match selector.entries.entry(fd) {
            HashMapEntry::Occupied(_) => {
                raise_exception::<u64>(_py, "KeyError", "fd is already registered")
            }
            HashMapEntry::Vacant(entry) => {
                inc_ref_bits(_py, fileobj_bits);
                entry.insert(SelectorRegistryEntry {
                    obj_bits: fileobj_bits,
                    events,
                });
                selector.obj_to_fd.insert(fileobj_bits, fd);
                int_bits_from_i64(_py, fd)
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_select_selector_register_fd(
    handle_bits: u64,
    fd_bits: u64,
    events_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Err(err) = require_net_capability(_py, "select.selector_register_fd") {
            return err;
        }
        let selector_ptr = match selector_state_mut_ptr(_py, handle_bits) {
            Ok(selector) => selector,
            Err(err) => return err,
        };
        let selector = unsafe { &mut *selector_ptr };
        let events = match parse_selector_events(_py, events_bits) {
            Ok(events) => events,
            Err(err) => return err,
        };
        let fd = match selector_parse_fd(_py, fd_bits) {
            Ok(fd) => fd,
            Err(err) => return err,
        };
        match selector.entries.entry(fd) {
            HashMapEntry::Occupied(_) => {
                raise_exception::<u64>(_py, "KeyError", "fd is already registered")
            }
            HashMapEntry::Vacant(entry) => {
                entry.insert(SelectorRegistryEntry {
                    obj_bits: MoltObject::none().bits(),
                    events,
                });
                int_bits_from_i64(_py, fd)
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_select_selector_unregister(handle_bits: u64, fd_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let selector_ptr = match selector_state_mut_ptr(_py, handle_bits) {
            Ok(selector) => selector,
            Err(err) => return err,
        };
        let selector = unsafe { &mut *selector_ptr };
        let fd = match selector_parse_fd(_py, fd_bits) {
            Ok(fd) => fd,
            Err(err) => return err,
        };
        let Some(entry) = selector.entries.remove(&fd) else {
            return raise_exception::<u64>(_py, "KeyError", "fd is not registered");
        };
        if !obj_from_bits(entry.obj_bits).is_none() {
            selector.obj_to_fd.remove(&entry.obj_bits);
        }
        selector_release_obj_bits(_py, entry.obj_bits);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_select_selector_unregister_obj(handle_bits: u64, fileobj_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let selector_ptr = match selector_state_mut_ptr(_py, handle_bits) {
            Ok(selector) => selector,
            Err(err) => return err,
        };
        let selector = unsafe { &mut *selector_ptr };
        let fd = match selector_resolve_fd_from_obj(_py, selector, fileobj_bits) {
            Ok(fd) => fd,
            Err(err) => return err,
        };
        let Some(entry) = selector.entries.remove(&fd) else {
            return raise_exception::<u64>(_py, "KeyError", "fd is not registered");
        };
        if !obj_from_bits(entry.obj_bits).is_none() {
            selector.obj_to_fd.remove(&entry.obj_bits);
        }
        selector_release_obj_bits(_py, entry.obj_bits);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_select_selector_modify(
    handle_bits: u64,
    fd_bits: u64,
    events_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Err(err) = require_net_capability(_py, "select.selector_modify") {
            return err;
        }
        let selector_ptr = match selector_state_mut_ptr(_py, handle_bits) {
            Ok(selector) => selector,
            Err(err) => return err,
        };
        let selector = unsafe { &mut *selector_ptr };
        let fd = match selector_parse_fd(_py, fd_bits) {
            Ok(fd) => fd,
            Err(err) => return err,
        };
        let events = match parse_selector_events(_py, events_bits) {
            Ok(events) => events,
            Err(err) => return err,
        };
        let Some(entry) = selector.entries.get_mut(&fd) else {
            return raise_exception::<u64>(_py, "KeyError", "fd is not registered");
        };
        entry.events = events;
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_select_selector_modify_obj(
    handle_bits: u64,
    fileobj_bits: u64,
    events_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Err(err) = require_net_capability(_py, "select.selector_modify_obj") {
            return err;
        }
        let selector_ptr = match selector_state_mut_ptr(_py, handle_bits) {
            Ok(selector) => selector,
            Err(err) => return err,
        };
        let selector = unsafe { &mut *selector_ptr };
        let fd = match selector_resolve_fd_from_obj(_py, selector, fileobj_bits) {
            Ok(fd) => fd,
            Err(err) => return err,
        };
        let events = match parse_selector_events(_py, events_bits) {
            Ok(events) => events,
            Err(err) => return err,
        };
        let Some(entry) = selector.entries.get_mut(&fd) else {
            return raise_exception::<u64>(_py, "KeyError", "fd is not registered");
        };
        entry.events = events;
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_select_selector_poll(handle_bits: u64, timeout_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Err(err) = require_net_capability(_py, "select.selector_poll") {
            return err;
        }
        let selector = match selector_state(_py, handle_bits) {
            Ok(selector) => selector,
            Err(err) => return err,
        };
        let timeout = match parse_selector_timeout(_py, timeout_bits) {
            Ok(timeout) => timeout,
            Err(err) => return err,
        };
        if selector.entries.is_empty() {
            if let Some(timeout) = timeout
                && timeout > 0.0
            {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let _release = GilReleaseGuard::new();
                    thread::sleep(Duration::from_secs_f64(timeout));
                }
                // On WASM there is no real fd polling and sleeping would freeze the
                // host event loop; fall through immediately so the empty-list return
                // below is reached without spinning.
                #[cfg(target_arch = "wasm32")]
                {}
            }
            let ptr = alloc_list(_py, &[]);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }

        let deadline = timeout.map(|value| monotonic_now_secs(_py) + value);
        let mut ready_masks: Vec<u32> = Vec::new();
        let mut watches: Vec<SelectWatch> = Vec::with_capacity(selector.entries.len());
        for (fd, entry) in &selector.entries {
            watches.push(SelectWatch {
                obj_bits: entry.obj_bits,
                handle: *fd,
                events: entry.events,
                kind: WatchKind::Read,
            });
        }

        loop {
            #[cfg(not(target_arch = "wasm32"))]
            {
                let timeout_ms = poll_timeout_ms(_py, timeout, deadline);
                let masks = match poll_masks_batch(_py, &watches, timeout_ms) {
                    Ok(masks) => masks,
                    Err(err) => return err,
                };
                if masks.iter().any(|mask| *mask != 0) {
                    ready_masks = masks;
                }
            }
            #[cfg(target_arch = "wasm32")]
            {
                ready_masks.clear();
                ready_masks.reserve(watches.len());
                for watch in &watches {
                    let mask = match poll_once(_py, watch) {
                        Ok(mask) => mask,
                        Err(err) => return err,
                    };
                    ready_masks.push(mask);
                }
            }
            if ready_masks.iter().any(|mask| *mask != 0) {
                break;
            }
            if timeout == Some(0.0) {
                break;
            }
            if let Some(deadline) = deadline
                && monotonic_now_secs(_py) >= deadline
            {
                break;
            }
            // On WASM there is no real fd polling; spinning would freeze the host
            // event loop. Break immediately so callers receive empty results when
            // no fds are ready — matching Python's behavior when timeout expires.
            #[cfg(target_arch = "wasm32")]
            break;
        }

        let ready_count = ready_masks.iter().filter(|mask| **mask != 0).count();
        let mut tuple_bits: Vec<u64> = Vec::with_capacity(ready_count);
        for (watch, mask) in watches.iter().zip(ready_masks.into_iter()) {
            if mask == 0 {
                continue;
            }
            let fd = watch.handle;
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_select_selector_close(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ptr = unsafe { selector_ptr_from_handle(handle_bits) };
        if ptr.is_null() {
            return raise_exception::<u64>(_py, "TypeError", "invalid selector handle");
        }
        let selector = unsafe { &mut *(ptr as *mut SelectorRegistry) };
        if selector.closed {
            return MoltObject::none().bits();
        }
        for (_, entry) in selector.entries.drain() {
            selector_release_obj_bits(_py, entry.obj_bits);
        }
        selector.obj_to_fd.clear();
        selector.closed = true;
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_select_selector_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ptr = unsafe { selector_ptr_from_handle(handle_bits) };
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        let mut selector = unsafe { Box::from_raw(ptr as *mut SelectorRegistry) };
        if !selector.closed {
            for (_, entry) in selector.entries.drain() {
                selector_release_obj_bits(_py, entry.obj_bits);
            }
            selector.obj_to_fd.clear();
            selector.closed = true;
        }
        unsafe { selector_release_ptr(ptr) };
        drop(selector);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_select_select(
    rlist_bits: u64,
    wlist_bits: u64,
    xlist_bits: u64,
    timeout_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Err(err) = require_net_capability(_py, "select.select") {
            return err;
        }
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

        let mut ready_r: Vec<u64> = Vec::with_capacity(r_objects.len());
        let mut ready_w: Vec<u64> = Vec::with_capacity(w_objects.len());
        let mut ready_x: Vec<u64> = Vec::with_capacity(x_objects.len());
        let deadline = timeout.map(|value| monotonic_now_secs(_py) + value);

        loop {
            #[cfg(not(target_arch = "wasm32"))]
            let watch_masks = {
                let timeout_ms = poll_timeout_ms(_py, timeout, deadline);
                match poll_masks_batch(_py, &watches, timeout_ms) {
                    Ok(masks) => masks,
                    Err(err) => {
                        release_bits(_py, &r_objects);
                        release_bits(_py, &w_objects);
                        release_bits(_py, &x_objects);
                        release_bits(_py, &ready_r);
                        release_bits(_py, &ready_w);
                        release_bits(_py, &ready_x);
                        return err;
                    }
                }
            };
            #[cfg(target_arch = "wasm32")]
            let watch_masks = {
                let mut masks: Vec<u32> = Vec::with_capacity(watches.len());
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
                    masks.push(mask);
                }
                masks
            };

            for (watch, mask) in watches.iter().zip(watch_masks.into_iter()) {
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
            if let Some(deadline) = deadline
                && monotonic_now_secs(_py) >= deadline
            {
                break;
            }
            // On WASM there is no real fd polling; spinning would freeze the host
            // event loop. Break immediately so callers receive empty fd lists when
            // no fds are ready — matching Python's behavior when timeout expires.
            #[cfg(target_arch = "wasm32")]
            break;
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
