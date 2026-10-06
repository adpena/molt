//! Frame objects' typed binding sources, `frame.f_locals`, `frame.clear()`
//! and the PEP 667 `FrameLocalsProxy` (3.13 onward).
//!
//! A frame object records the object that holds its frame's bindings in a
//! typed field of its `TYPES_FRAME` payload, never in its `__dict__`: an
//! optimized activation's `FRAME_BINDINGS` payload (synchronous or stateful),
//! or a module body's namespace mapping. `f_locals` reads it afresh on every
//! access. Before PEP 667 an optimized frame reports its activation's one
//! refreshed dict; from 3.13 a `FrameLocalsProxy` over the same payload.
//! Proxy reads project the frame's binding storage when called; proxy writes
//! go to that same storage: a synchronous frame's homes (or its retired
//! payload), a stateful activation's task payload, a cell's contents, or the
//! frame's extra locals for a name that is not a code slot.

use molt_obj_model::MoltObject;

use super::bindings::{
    frame_bindings_clear, frame_bindings_locals_dict, frame_bindings_pep667, frame_bindings_ptr,
    frame_bindings_snapshot, frame_bindings_write,
};
use crate::object::{ObjectShapeId, object_shape_id};
use crate::{
    FRAME_STACK, PyToken, TYPE_ID_DICT, TYPE_ID_FRAME_BINDINGS, TYPE_ID_TUPLE, TYPE_ID_TYPE,
    alloc_dict_with_pairs, alloc_instance_for_class, alloc_list, alloc_string, alloc_tuple,
    dec_ref_bits, dict_order, exception_pending, inc_ref_bits, molt_contains, molt_dict_get,
    molt_eq, molt_index, molt_iter, molt_len, molt_repr_from_obj, obj_from_bits, object_type_id,
    raise_exception, string_obj_to_owned,
};

const WORD: usize = std::mem::size_of::<u64>();

enum FrameSource {
    Empty,
    /// An optimized activation's `FRAME_BINDINGS` payload.
    Bindings(u64),
    /// A module body's namespace.
    Mapping(u64),
}

/// Classify a source the runtime stored.
fn frame_source(bits: u64) -> FrameSource {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return FrameSource::Empty;
    };
    if unsafe { object_type_id(ptr) } == TYPE_ID_FRAME_BINDINGS {
        FrameSource::Bindings(bits)
    } else {
        FrameSource::Mapping(bits)
    }
}

/// The typed source word of an object of `shape`: its second-to-last payload
/// word. The last is its managed `__dict__`.
unsafe fn source_slot(ptr: *mut u8, shape: ObjectShapeId) -> Option<*mut u64> {
    unsafe {
        if object_shape_id(ptr) != shape {
            return None;
        }
        let payload = crate::object::object_payload_size(ptr);
        (payload >= 2 * WORD).then(|| ptr.add(payload - 2 * WORD).cast::<u64>())
    }
}

/// Record a new frame object's binding source; the frame takes a reference.
/// `false`: the frame class lacks its typed layout (`SystemError` raised).
pub(crate) fn frame_object_set_source(py: &PyToken<'_>, frame_ptr: *mut u8, source: u64) -> bool {
    let Some(slot) = (unsafe { source_slot(frame_ptr, ObjectShapeId::TypesFrame) }) else {
        if !exception_pending(py) {
            raise_exception::<u64>(
                py,
                "SystemError",
                "frame object has no binding source field",
            );
        }
        return false;
    };
    inc_ref_bits(py, source);
    let previous = unsafe { slot.replace(source) };
    if previous != 0 {
        dec_ref_bits(py, previous);
    }
    true
}

/// # Safety
/// `ptr` must be a live `TYPES_FRAME` object.
pub(crate) unsafe fn frame_object_visit(ptr: *mut u8, mut visit: impl FnMut(u64)) {
    if let Some(slot) = unsafe { source_slot(ptr, ObjectShapeId::TypesFrame) } {
        visit(unsafe { *slot });
    }
}

/// # Safety
/// `ptr` must be a live `TYPES_FRAME` object under the GIL.
pub(crate) unsafe fn frame_object_detach(ptr: *mut u8, mut detach: impl FnMut(u64)) {
    if let Some(slot) = unsafe { source_slot(ptr, ObjectShapeId::TypesFrame) } {
        let previous = unsafe { slot.replace(0) };
        if previous != 0 {
            detach(previous);
        }
    }
}

/// # Safety
/// `ptr` must be a live `TYPES_FRAME_LOCALS_PROXY` object.
pub(crate) unsafe fn frame_locals_proxy_visit(ptr: *mut u8, mut visit: impl FnMut(u64)) {
    if let Some(slot) = unsafe { source_slot(ptr, ObjectShapeId::TypesFrameLocalsProxy) } {
        visit(unsafe { *slot });
    }
}

/// # Safety
/// `ptr` must be a live `TYPES_FRAME_LOCALS_PROXY` object under the GIL.
pub(crate) unsafe fn frame_locals_proxy_detach(ptr: *mut u8, mut detach: impl FnMut(u64)) {
    if let Some(slot) = unsafe { source_slot(ptr, ObjectShapeId::TypesFrameLocalsProxy) } {
        let previous = unsafe { slot.replace(0) };
        if previous != 0 {
            detach(previous);
        }
    }
}

fn empty_dict(py: &PyToken<'_>) -> u64 {
    let ptr = alloc_dict_with_pairs(py, &[]);
    if ptr.is_null() {
        if !exception_pending(py) {
            raise_exception::<u64>(py, "MemoryError", "cannot allocate frame locals");
        }
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(ptr).bits()
}

fn frame_locals_proxy(py: &PyToken<'_>, source: u64) -> u64 {
    let class_bits = crate::builtins::types::frame_locals_proxy_class(py);
    let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
        return MoltObject::none().bits();
    };
    if unsafe { object_type_id(class_ptr) } != TYPE_ID_TYPE {
        return raise_exception::<u64>(py, "SystemError", "FrameLocalsProxy class is invalid");
    }
    let proxy_bits = unsafe { alloc_instance_for_class(py, class_ptr) };
    let Some(proxy_ptr) = obj_from_bits(proxy_bits).as_ptr() else {
        return MoltObject::none().bits();
    };
    let Some(slot) = (unsafe { source_slot(proxy_ptr, ObjectShapeId::TypesFrameLocalsProxy) })
    else {
        dec_ref_bits(py, proxy_bits);
        return raise_exception::<u64>(py, "SystemError", "FrameLocalsProxy has no source field");
    };
    inc_ref_bits(py, source);
    let previous = unsafe { slot.replace(source) };
    if previous != 0 {
        dec_ref_bits(py, previous);
    }
    proxy_bits
}

/// The typed source word of a frame object. `TypeError` for anything else.
fn frame_object_source_slot(
    py: &PyToken<'_>,
    frame_bits: u64,
    method: &str,
) -> Result<*mut u64, ()> {
    let slot = obj_from_bits(frame_bits)
        .as_ptr()
        .and_then(|ptr| unsafe { source_slot(ptr, ObjectShapeId::TypesFrame) });
    slot.ok_or_else(|| {
        raise_exception::<u64>(
            py,
            "TypeError",
            &format!("descriptor '{method}' for 'frame' objects doesn't apply to this object"),
        );
    })
}

/// `frame.f_locals`, read now. An optimized frame (synchronous or stateful)
/// reports its activation's one refreshed dict before PEP 667 and a proxy from
/// 3.13; a module body reports its namespace mapping.
#[unsafe(no_mangle)]
pub extern "C" fn molt_frame_f_locals_get(_descriptor_bits: u64, frame_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Ok(slot) = frame_object_source_slot(py, frame_bits, "f_locals") else {
            return MoltObject::none().bits();
        };
        let source = unsafe { *slot };
        match frame_source(source) {
            FrameSource::Empty => empty_dict(py),
            FrameSource::Mapping(bits) => {
                inc_ref_bits(py, bits);
                bits
            }
            FrameSource::Bindings(payload) => match frame_bindings_ptr(py, payload) {
                Ok(ptr) if frame_bindings_pep667(ptr) => frame_locals_proxy(py, source),
                Ok(_) => frame_bindings_locals_dict(py, payload)
                    .unwrap_or_else(|()| MoltObject::none().bits()),
                Err(()) => MoltObject::none().bits(),
            },
        }
    })
}

/// `frame.clear()` (CPython `frame_clear`), through the owner `f_locals`
/// reads. An executing frame raises `RuntimeError`; a stateful frame follows
/// the target's generator rules; a finished optimized frame releases what its
/// frame object owns in the explicit-clear order; a finished module body's
/// frame drops its namespace. Returns None; a failure leaves its exception
/// pending.
#[unsafe(no_mangle)]
pub extern "C" fn molt_frame_clear(frame_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Ok(slot) = frame_object_source_slot(py, frame_bits, "clear") else {
            return MoltObject::none().bits();
        };
        match frame_source(unsafe { *slot }) {
            FrameSource::Empty => {}
            FrameSource::Bindings(payload) => {
                let _ = frame_bindings_clear(py, payload);
            }
            FrameSource::Mapping(mapping) => {
                let executing = FRAME_STACK.with(|stack| {
                    stack
                        .borrow()
                        .iter()
                        .any(|entry| entry.locals_bits == mapping)
                });
                if executing {
                    return raise_exception::<u64>(
                        py,
                        "RuntimeError",
                        "cannot clear an executing frame",
                    );
                }
                let previous = unsafe { slot.replace(0) };
                dec_ref_bits(py, previous);
            }
        }
        MoltObject::none().bits()
    })
}

/// The frame payload a proxy reads and writes, borrowed from the proxy.
fn proxy_source(py: &PyToken<'_>, self_bits: u64) -> Result<u64, ()> {
    let slot = obj_from_bits(self_bits)
        .as_ptr()
        .and_then(|ptr| unsafe { source_slot(ptr, ObjectShapeId::TypesFrameLocalsProxy) });
    let Some(slot) = slot else {
        raise_exception::<u64>(py, "TypeError", "expected a FrameLocalsProxy");
        return Err(());
    };
    match frame_source(unsafe { *slot }) {
        FrameSource::Bindings(payload) => Ok(payload),
        FrameSource::Empty | FrameSource::Mapping(_) => {
            raise_exception::<u64>(
                py,
                "SystemError",
                "FrameLocalsProxy is not bound to a frame",
            );
            Err(())
        }
    }
}

/// A dict of the proxied frame's bindings as they are now. Owned.
fn proxy_snapshot(py: &PyToken<'_>, self_bits: u64) -> Result<u64, ()> {
    frame_bindings_snapshot(py, proxy_source(py, self_bits)?)
}

/// Write (`Some`) or delete (`None`) one key in the proxied frame's storage.
fn proxy_write(py: &PyToken<'_>, self_bits: u64, key: u64, value: Option<u64>) -> Result<(), ()> {
    frame_bindings_write(py, proxy_source(py, self_bits)?, key, value)
}

/// Run `operation` over a snapshot dict and release the snapshot.
fn with_snapshot(py: &PyToken<'_>, self_bits: u64, operation: impl FnOnce(u64) -> u64) -> u64 {
    let Ok(snapshot) = proxy_snapshot(py, self_bits) else {
        return MoltObject::none().bits();
    };
    let result = operation(snapshot);
    dec_ref_bits(py, snapshot);
    result
}

/// Positional arguments of a vararg method, rejecting keyword arguments.
fn positional_args(
    py: &PyToken<'_>,
    method: &str,
    args_bits: u64,
    kwargs_bits: u64,
    range: std::ops::RangeInclusive<usize>,
) -> Option<Vec<u64>> {
    let args = obj_from_bits(args_bits)
        .as_ptr()
        .filter(|ptr| unsafe { object_type_id(*ptr) } == TYPE_ID_TUPLE)
        .and_then(|ptr| unsafe {
            crate::object::seq_access::with_immutable_tuple_slice(ptr, |args| args.to_vec())
        });
    let keywords = obj_from_bits(kwargs_bits)
        .as_ptr()
        .filter(|ptr| unsafe { object_type_id(*ptr) } == TYPE_ID_DICT)
        .is_some_and(|ptr| unsafe { !dict_order(ptr).is_empty() });
    match args {
        Some(args) if !keywords && range.contains(&args.len()) => Some(args),
        _ => {
            raise_exception::<u64>(
                py,
                "TypeError",
                &format!(
                    "FrameLocalsProxy.{method}() takes {} to {} positional arguments",
                    range.start(),
                    range.end()
                ),
            );
            None
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_frame_locals_proxy_getitem(self_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        with_snapshot(py, self_bits, |snapshot| molt_index(snapshot, key_bits))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_frame_locals_proxy_contains(self_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        with_snapshot(py, self_bits, |snapshot| molt_contains(snapshot, key_bits))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_frame_locals_proxy_len(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        with_snapshot(py, self_bits, |snapshot| molt_len(snapshot))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_frame_locals_proxy_iter(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        with_snapshot(py, self_bits, |snapshot| molt_iter(snapshot))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_frame_locals_proxy_get(
    self_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(args) = positional_args(py, "get", args_bits, kwargs_bits, 1..=2) else {
            return MoltObject::none().bits();
        };
        let default = args
            .get(1)
            .copied()
            .unwrap_or_else(|| MoltObject::none().bits());
        with_snapshot(py, self_bits, |snapshot| {
            molt_dict_get(snapshot, args[0], default)
        })
    })
}

/// One list built from the snapshot's ordered pairs.
fn snapshot_list(py: &PyToken<'_>, self_bits: u64, project: impl Fn(u64, u64) -> u64) -> u64 {
    with_snapshot(py, self_bits, |snapshot| {
        let Some(dict) = obj_from_bits(snapshot).as_ptr() else {
            return MoltObject::none().bits();
        };
        let pairs: Vec<u64> = unsafe { dict_order(dict).clone() };
        let mut items = Vec::with_capacity(pairs.len() / 2);
        for pair in pairs.chunks_exact(2) {
            let item = project(pair[0], pair[1]);
            if exception_pending(py) {
                for bits in items {
                    dec_ref_bits(py, bits);
                }
                return MoltObject::none().bits();
            }
            items.push(item);
        }
        let list = alloc_list(py, &items);
        for bits in items {
            dec_ref_bits(py, bits);
        }
        if list.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(list).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_frame_locals_proxy_keys(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        snapshot_list(py, self_bits, |key, _| {
            inc_ref_bits(py, key);
            key
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_frame_locals_proxy_values(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        snapshot_list(py, self_bits, |_, value| {
            inc_ref_bits(py, value);
            value
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_frame_locals_proxy_items(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        snapshot_list(py, self_bits, |key, value| {
            let pair = alloc_tuple(py, &[key, value]);
            if pair.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(pair).bits()
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_frame_locals_proxy_copy(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        proxy_snapshot(py, self_bits).unwrap_or_else(|()| MoltObject::none().bits())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_frame_locals_proxy_repr(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        with_snapshot(py, self_bits, |snapshot| {
            let repr_bits = molt_repr_from_obj(snapshot);
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
            let repr = string_obj_to_owned(obj_from_bits(repr_bits)).unwrap_or_default();
            dec_ref_bits(py, repr_bits);
            let out = alloc_string(py, format!("FrameLocalsProxy({repr})").as_bytes());
            if out.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(out).bits()
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_frame_locals_proxy_eq(self_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        with_snapshot(py, self_bits, |snapshot| molt_eq(snapshot, other_bits))
    })
}

/// The proxied frame's current binding of `key`, owned, or `None` when the
/// key is not bound. `Err`: an exception is pending.
fn proxy_lookup(py: &PyToken<'_>, self_bits: u64, key: u64) -> Result<Option<u64>, ()> {
    let snapshot = proxy_snapshot(py, self_bits)?;
    let missing = crate::missing_bits(py);
    let value = molt_dict_get(snapshot, key, missing);
    dec_ref_bits(py, snapshot);
    if exception_pending(py) {
        return Err(());
    }
    if value == missing {
        dec_ref_bits(py, value);
        return Ok(None);
    }
    Ok(Some(value))
}

/// Returns `None`; a failed write leaves its exception pending.
#[unsafe(no_mangle)]
pub extern "C" fn molt_frame_locals_proxy_setitem(
    self_bits: u64,
    key_bits: u64,
    value_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let _ = proxy_write(py, self_bits, key_bits, Some(value_bits));
        MoltObject::none().bits()
    })
}

/// Returns `None`; a failed deletion leaves its exception pending.
#[unsafe(no_mangle)]
pub extern "C" fn molt_frame_locals_proxy_delitem(self_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let _ = proxy_write(py, self_bits, key_bits, None);
        MoltObject::none().bits()
    })
}

/// `setdefault` of a bound name is a read; of an unbound one, a write.
#[unsafe(no_mangle)]
pub extern "C" fn molt_frame_locals_proxy_setdefault(
    self_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(args) = positional_args(py, "setdefault", args_bits, kwargs_bits, 1..=2) else {
            return MoltObject::none().bits();
        };
        match proxy_lookup(py, self_bits, args[0]) {
            Err(()) => MoltObject::none().bits(),
            Ok(Some(value)) => value,
            Ok(None) => {
                let default = args
                    .get(1)
                    .copied()
                    .unwrap_or_else(|| MoltObject::none().bits());
                if proxy_write(py, self_bits, args[0], Some(default)).is_err() {
                    return MoltObject::none().bits();
                }
                inc_ref_bits(py, default);
                default
            }
        }
    })
}

/// Removes an extra local; a code slot's binding cannot be removed.
#[unsafe(no_mangle)]
pub extern "C" fn molt_frame_locals_proxy_pop(
    self_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(args) = positional_args(py, "pop", args_bits, kwargs_bits, 1..=2) else {
            return MoltObject::none().bits();
        };
        match proxy_lookup(py, self_bits, args[0]) {
            Err(()) => MoltObject::none().bits(),
            Ok(Some(value)) => {
                if proxy_write(py, self_bits, args[0], None).is_err() {
                    dec_ref_bits(py, value);
                    return MoltObject::none().bits();
                }
                value
            }
            Ok(None) => match args.get(1) {
                Some(&default) => {
                    inc_ref_bits(py, default);
                    default
                }
                None => crate::builtins::exceptions::raise_key_error_with_key::<u64>(py, args[0]),
            },
        }
    })
}

/// Writes every item of a dict or of another frame's proxy.
#[unsafe(no_mangle)]
pub extern "C" fn molt_frame_locals_proxy_update(
    self_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(args) = positional_args(py, "update", args_bits, kwargs_bits, 1..=1) else {
            return MoltObject::none().bits();
        };
        let other = args[0];
        let is_proxy = obj_from_bits(other).as_ptr().is_some_and(|ptr| unsafe {
            source_slot(ptr, ObjectShapeId::TypesFrameLocalsProxy).is_some()
        });
        let source = if is_proxy {
            match proxy_snapshot(py, other) {
                Ok(snapshot) => snapshot,
                Err(()) => return MoltObject::none().bits(),
            }
        } else if obj_from_bits(other)
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) } == TYPE_ID_DICT)
        {
            inc_ref_bits(py, other);
            other
        } else {
            return raise_exception::<u64>(
                py,
                "TypeError",
                "update() argument must be dict or another FrameLocalsProxy",
            );
        };
        // Items are read before the first write: a write can run a finalizer
        // that changes the source mapping.
        let pairs: Vec<u64> =
            unsafe { dict_order(obj_from_bits(source).as_ptr().expect("dict source")).clone() };
        for &bits in &pairs {
            inc_ref_bits(py, bits);
        }
        for pair in pairs.chunks_exact(2) {
            // The first failure leaves its exception pending.
            if proxy_write(py, self_bits, pair[0], Some(pair[1])).is_err() {
                break;
            }
        }
        for bits in pairs {
            dec_ref_bits(py, bits);
        }
        dec_ref_bits(py, source);
        MoltObject::none().bits()
    })
}
