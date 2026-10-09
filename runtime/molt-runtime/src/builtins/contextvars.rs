//! One Context/ContextVar/Token authority for Python, scheduling and the C API.
//! Root reads are retained while holding the actual runtime GIL, before any
//! callback can replace the root. Root replacement publishes before releasing.
//! This follows CPython 3.14.8 context_get_vars/context_set_vars and the checked
//! token allocation in 3.13.16; it does not change recorded reference versions.
mod native;
#[cfg(test)]
mod tests;
pub(crate) mod trie;
use crate::object::builders::PtrDropGuard;
use crate::object::{ObjectShapeId, TYPE_ID_OBJECT, object_shape_id, object_type_id};
use crate::{
    MoltObject, PyToken, dec_ref_bits, exception_pending, inc_ref_bits, obj_from_bits,
    raise_exception,
};
pub use native::*;
use std::cell::Cell;

#[repr(C)]
struct Context {
    root: u64,
    previous: u64,
    len: u64,
    entered: u64,
}
#[repr(C)]
struct Variable {
    name: u64,
    default: u64,
    hash: i64,
    has_default: u64,
}
#[repr(C)]
struct Token {
    context: u64,
    variable: u64,
    old: u64,
    flags: u64,
}
#[repr(C)]
struct ContextIterator {
    root: u64,
    mode: u64,
    cursor: trie::Cursor,
}
const HAD_OLD: u64 = 1;
const USED: u64 = 2;
thread_local! {
    // A zero current pointer means no Context has been materialized. Contexts
    // entered on this thread own the previous edge; TLS owns only the head.
    static CURRENT:Cell<u64> = const { Cell::new(0) };
    static DORMANT_TASK:Cell<usize> = const { Cell::new(0) };
    // An empty independently scheduled task needs isolation without allocation.
    // This owned predecessor moves into Context.previous on first use.
    static DORMANT_PREVIOUS:Cell<u64> = const { Cell::new(0) };
}
fn none() -> u64 {
    MoltObject::none().bits()
}
fn owned(py: &PyToken<'_>, bits: u64) -> u64 {
    if bits != 0 {
        inc_ref_bits(py, bits);
    }
    bits
}
fn release(py: &PyToken<'_>, bits: u64) {
    if bits != 0 {
        dec_ref_bits(py, bits);
    }
}
fn checked(py: &PyToken<'_>, bits: u64, shape: ObjectShapeId, label: &str) -> Option<*mut u8> {
    let p = obj_from_bits(bits).as_ptr();
    if let Some(p) = p
        && unsafe { object_type_id(p) == TYPE_ID_OBJECT && object_shape_id(p) == shape }
    {
        return Some(p);
    }
    raise_exception::<()>(
        py,
        "TypeError",
        &format!("an instance of {label} was expected"),
    );
    None
}
pub(crate) fn is_context(bits: u64) -> bool {
    obj_from_bits(bits).as_ptr().is_some_and(|p| unsafe {
        object_type_id(p) == TYPE_ID_OBJECT && object_shape_id(p) == ObjectShapeId::Context
    })
}
unsafe fn context(bits: u64) -> &'static mut Context {
    unsafe {
        &mut *obj_from_bits(bits)
            .as_ptr()
            .expect("Context")
            .cast::<Context>()
    }
}
unsafe fn variable(bits: u64) -> &'static Variable {
    unsafe {
        &*obj_from_bits(bits)
            .as_ptr()
            .expect("ContextVar")
            .cast::<Variable>()
    }
}
fn var_hash(bits: u64) -> u32 {
    let hash = unsafe { variable(bits).hash } as u64;
    (hash ^ (hash >> 32)) as u32
}
fn alloc<T>(py: &PyToken<'_>, class: u64, value: T) -> Option<u64> {
    if class == 0 || exception_pending(py) {
        return None;
    }
    let bits = crate::object::builders::alloc_class_instance(py, std::mem::size_of::<T>(), class);
    let p = obj_from_bits(bits).as_ptr()?;
    unsafe {
        p.cast::<T>().write(value);
        crate::object::gc::gc_publish_initialized(py, p);
    }
    Some(bits)
}
pub(crate) fn new_context(py: &PyToken<'_>) -> Option<u64> {
    alloc(
        py,
        context_class(py),
        Context {
            root: 0,
            previous: 0,
            len: 0,
            entered: 0,
        },
    )
}
pub(crate) fn current(py: &PyToken<'_>) -> Option<u64> {
    let existing = CURRENT.with(Cell::get);
    if existing != 0 {
        return Some(existing);
    }
    let bits = new_context(py)?;
    let task = DORMANT_TASK.with(Cell::get);
    if task != 0 {
        // Install the attachment before exposing the Context as entered. The
        // attachment and TLS are separate physical owners of the same object.
        crate::async_rt::cancellation::materialize_task_context(py, task as *mut u8, bits);
        unsafe {
            context(bits).entered = 1;
            context(bits).previous = DORMANT_PREVIOUS.with(|v| v.replace(0));
        }
    }
    CURRENT.with(|v| v.set(bits));
    Some(bits)
}
pub(crate) fn copy_context(py: &PyToken<'_>, source: u64) -> Option<u64> {
    checked(py, source, ObjectShapeId::Context, "Context")?;
    let (root, len) = unsafe {
        let c = context(source);
        (owned(py, c.root), c.len)
    };
    let _pin = (root != 0).then(|| PtrDropGuard::preserving(obj_from_bits(root).as_ptr().unwrap()));
    let out = alloc(
        py,
        context_class(py),
        Context {
            root: 0,
            previous: 0,
            len,
            entered: 0,
        },
    )?;
    unsafe {
        context(out).root = owned(py, root);
    }
    Some(out)
}
pub(crate) fn copy_current(py: &PyToken<'_>) -> Option<u64> {
    let c = current(py)?;
    copy_context(py, c)
}
pub(crate) fn enter(py: &PyToken<'_>, bits: u64) -> bool {
    if checked(py, bits, ObjectShapeId::Context, "Context").is_none() {
        return false;
    }
    unsafe {
        if context(bits).entered != 0 {
            raise_exception::<()>(
                py,
                "RuntimeError",
                "cannot enter context: Context is already entered",
            );
            return false;
        }
        inc_ref_bits(py, bits);
        context(bits).previous = CURRENT.with(|v| v.replace(bits));
        context(bits).entered = 1;
    }
    true
}
pub(crate) fn exit(py: &PyToken<'_>, bits: u64) -> bool {
    if checked(py, bits, ObjectShapeId::Context, "Context").is_none() {
        return false;
    }
    unsafe {
        if context(bits).entered == 0 {
            raise_exception::<()>(
                py,
                "RuntimeError",
                "cannot exit context: Context is not entered",
            );
            return false;
        }
        if CURRENT.with(Cell::get) != bits {
            raise_exception::<()>(
                py,
                "RuntimeError",
                "cannot exit context: Context is not current",
            );
            return false;
        }
        let previous = std::mem::replace(&mut context(bits).previous, 0);
        context(bits).entered = 0;
        CURRENT.with(|v| v.set(previous));
    }
    dec_ref_bits(py, bits);
    true
}
pub(crate) struct EnteredContext<'a, 'py> {
    py: &'a PyToken<'py>,
    bits: u64,
}
impl<'a, 'py> EnteredContext<'a, 'py> {
    pub(crate) fn enter(py: &'a PyToken<'py>, bits: u64) -> Option<Self> {
        enter(py, bits).then_some(Self { py, bits })
    }
}
impl Drop for EnteredContext<'_, '_> {
    fn drop(&mut self) {
        molt_cpython_abi::api::errors::with_preserved_error(|| {
            exit(self.py, self.bits);
        });
    }
}

pub(crate) fn new_variable(py: &PyToken<'_>, name: u64, default: Option<u64>) -> Option<u64> {
    if unsafe { crate::object::ops_format::with_string_bytes(obj_from_bits(name), |_| ()) }
        .is_none()
    {
        raise_exception::<()>(py, "TypeError", "context variable name must be a str");
        return None;
    }
    let class = variable_class(py);
    if class == 0 {
        return None;
    }
    // Allocate before computing the identity hash, but keep the unpublished
    // native shape valid and locally pinned across a str-subclass hash call.
    let bits =
        crate::object::builders::alloc_class_instance(py, std::mem::size_of::<Variable>(), class);
    let p = obj_from_bits(bits).as_ptr()?;
    let mut pin = PtrDropGuard::preserving(p);
    let hash = crate::object::ops_hash::hash_bits_signed(py, name);
    if exception_pending(py) {
        return None;
    }
    let pointer_hash = crate::object::ops_hash::hash_pointer(p as u64);
    let hash = hash ^ pointer_hash;
    let hash = if hash == -1 { -2 } else { hash };
    unsafe {
        p.cast::<Variable>().write(Variable {
            name: owned(py, name),
            default: default.map_or(0, |b| owned(py, b)),
            hash,
            has_default: u64::from(default.is_some()),
        });
        crate::object::gc::gc_publish_initialized(py, p);
    }
    pin.release();
    Some(bits)
}
pub(crate) fn get_variable(
    py: &PyToken<'_>,
    var: u64,
    default: Option<u64>,
) -> Option<Option<u64>> {
    checked(py, var, ObjectShapeId::ContextVar, "ContextVar")?;
    let current = CURRENT.with(Cell::get);
    let found = if current == 0 {
        None
    } else {
        trie::lookup(unsafe { context(current).root }, var, var_hash(var))
    };
    let value = found.or(default).or_else(|| unsafe {
        let v = variable(var);
        (v.has_default != 0).then_some(v.default)
    });
    Some(value.map(|bits| owned(py, bits)))
}
pub(crate) fn set_variable(py: &PyToken<'_>, var: u64, value: u64) -> Option<u64> {
    checked(py, var, ObjectShapeId::ContextVar, "ContextVar")?;
    let ctx = current(py)?;
    let old = trie::lookup(unsafe { context(ctx).root }, var, var_hash(var));
    owned(py, ctx);
    let _context_pin = PtrDropGuard::preserving(obj_from_bits(ctx).as_ptr().unwrap());
    let _old_pin = old.and_then(|bits| {
        owned(py, bits);
        obj_from_bits(bits).as_ptr().map(PtrDropGuard::preserving)
    });
    // Token construction MUST succeed before association. It holds the actual
    // Context identity and the old value independently of later reset/GC.
    let token = alloc(
        py,
        token_class(py),
        Token {
            context: 0,
            variable: 0,
            old: 0,
            flags: 0,
        },
    )?;
    let mut pin = PtrDropGuard::preserving(obj_from_bits(token).as_ptr().unwrap());
    unsafe {
        let t = &mut *obj_from_bits(token).as_ptr().unwrap().cast::<Token>();
        t.context = owned(py, ctx);
        t.variable = owned(py, var);
        t.old = old.map_or(0, |b| owned(py, b));
        t.flags = if old.is_some() { HAD_OLD } else { 0 };
    }
    let root = trie::assoc(py, unsafe { context(ctx).root }, var, value, var_hash(var))?;
    let prior = unsafe {
        let c = context(ctx);
        if old.is_none() {
            c.len += 1;
        }
        std::mem::replace(&mut c.root, root)
    };
    release(py, prior);
    pin.release();
    Some(token)
}
pub(crate) fn reset_variable(py: &PyToken<'_>, var: u64, token: u64) -> bool {
    if checked(py, var, ObjectShapeId::ContextVar, "ContextVar").is_none() {
        return false;
    }
    let Some(p) = checked(py, token, ObjectShapeId::ContextToken, "Token") else {
        return false;
    };
    let (ctx, tvar, old, flags) = unsafe {
        let t = &*p.cast::<Token>();
        (t.context, t.variable, t.old, t.flags)
    };
    if flags & USED != 0 {
        raise_exception::<()>(py, "RuntimeError", "Token has already been used once");
        return false;
    }
    if tvar != var {
        raise_exception::<()>(
            py,
            "ValueError",
            "Token was created by a different ContextVar",
        );
        return false;
    }
    let Some(current) = current(py) else {
        return false;
    };
    if current != ctx {
        raise_exception::<()>(py, "ValueError", "Token was created in a different Context");
        return false;
    }
    unsafe {
        (*p.cast::<Token>()).flags |= USED;
    }
    let prior = unsafe { context(ctx).root };
    let had = trie::lookup(prior, var, var_hash(var)).is_some();
    let root = if flags & HAD_OLD != 0 {
        trie::assoc(py, prior, var, old, var_hash(var))
    } else {
        if !had {
            raise_exception::<()>(py, "LookupError", "ContextVar has no value");
            return false;
        }
        trie::without(py, prior, var, var_hash(var))
    };
    let Some(root) = root else {
        return false;
    };
    unsafe {
        let c = context(ctx);
        c.root = root;
        if flags & HAD_OLD != 0 && !had {
            c.len += 1;
        } else if flags & HAD_OLD == 0 && had {
            c.len -= 1;
        }
    }
    release(py, prior);
    true
}

/// Only independently scheduled execution roots have a Context binding. Manual
/// send/throw and nested future polling inherit the running TLS head unchanged.
pub(crate) struct ExecutionContextLease<'a, 'py> {
    py: &'a PyToken<'py>,
    binding: crate::async_rt::cancellation::TaskContextBinding,
    saved_task: usize,
    saved_previous: u64,
}
impl<'a, 'py> ExecutionContextLease<'a, 'py> {
    pub(crate) fn enter(py: &'a PyToken<'py>, task: *mut u8) -> Option<Self> {
        use crate::async_rt::cancellation::TaskContextBinding as B;
        let binding = crate::async_rt::cancellation::task_context_binding(py, task);
        if let B::Owned(ctx) = binding {
            if !enter(py, ctx) {
                return None;
            }
        }
        let saved_task = if matches!(binding, B::Inherited) {
            0
        } else {
            DORMANT_TASK.with(|v| {
                v.replace(if matches!(binding, B::OwnedEmpty) {
                    task as usize
                } else {
                    0
                })
            })
        };
        let saved_previous = if matches!(binding, B::Inherited) {
            0
        } else {
            DORMANT_PREVIOUS.with(|v| {
                v.replace(if matches!(binding, B::OwnedEmpty) {
                    CURRENT.with(|c| c.replace(0))
                } else {
                    0
                })
            })
        };
        Some(Self {
            py,
            binding,
            saved_task,
            saved_previous,
        })
    }
}
impl Drop for ExecutionContextLease<'_, '_> {
    fn drop(&mut self) {
        use crate::async_rt::cancellation::TaskContextBinding as B;
        if matches!(self.binding, B::Inherited) {
            return;
        }
        molt_cpython_abi::api::errors::with_preserved_error(|| {
            match self.binding {
                B::Owned(ctx) => {
                    exit(self.py, ctx);
                }
                B::OwnedEmpty => {
                    let ctx = CURRENT.with(Cell::get);
                    if ctx == 0 {
                        CURRENT.with(|v| v.set(DORMANT_PREVIOUS.with(|p| p.replace(0))));
                    } else {
                        exit(self.py, ctx);
                    }
                }
                B::Inherited => {}
            }
            DORMANT_TASK.with(|v| v.set(self.saved_task));
            DORMANT_PREVIOUS.with(|v| v.set(self.saved_previous));
        });
    }
}
pub(crate) fn capture_scheduled_context(
    py: &PyToken<'_>,
) -> Option<crate::async_rt::cancellation::TaskContextBinding> {
    use crate::async_rt::cancellation::TaskContextBinding as B;
    let ctx = CURRENT.with(Cell::get);
    if ctx == 0 {
        Some(B::OwnedEmpty)
    } else {
        copy_context(py, ctx).map(B::Owned)
    }
}
pub(crate) fn clear_thread_context(py: &PyToken<'_>) -> bool {
    let current = CURRENT.with(|v| v.replace(0));
    let previous = DORMANT_PREVIOUS.with(|v| v.replace(0));
    DORMANT_TASK.with(|v| v.set(0));
    // Teardown is a thread boundary, never ordinary C-call detach. Sever all
    // entered predecessors before any decref can call back into fresh TLS.
    let mut cursor = current;
    while cursor != 0 {
        let prior = unsafe {
            let c = context(cursor);
            c.entered = 0;
            std::mem::replace(&mut c.previous, 0)
        };
        release(py, cursor);
        cursor = prior;
    }
    release(py, previous);
    current != 0 || previous != 0
}
pub(crate) unsafe fn contextvars_visit_owned_edges(
    shape: ObjectShapeId,
    p: *mut u8,
    mut visit: impl FnMut(u64),
) {
    unsafe {
        match shape {
            ObjectShapeId::Context => {
                let c = &*p.cast::<Context>();
                visit(c.root);
                visit(c.previous);
            }
            ObjectShapeId::ContextVar => {
                let v = &*p.cast::<Variable>();
                visit(v.name);
                if v.has_default != 0 {
                    visit(v.default);
                }
            }
            ObjectShapeId::ContextToken => {
                let t = &*p.cast::<Token>();
                visit(t.context);
                visit(t.variable);
                if t.flags & HAD_OLD != 0 {
                    visit(t.old);
                }
            }
            ObjectShapeId::ContextIterator => visit((*p.cast::<ContextIterator>()).root),
            _ => unreachable!("wrong Context shape"),
        }
    }
}
pub(crate) unsafe fn contextvars_clear_cycle_edges(
    shape: ObjectShapeId,
    p: *mut u8,
    sink: &mut crate::object::heap_lifecycle::DetachedEdgeSink,
) {
    unsafe {
        contextvars_visit_owned_edges(shape, p, |bits| sink.detach_if_heap(bits));
        match shape {
            ObjectShapeId::Context => p.cast::<Context>().write(Context {
                root: 0,
                previous: 0,
                len: 0,
                entered: 0,
            }),
            ObjectShapeId::ContextVar => p.cast::<Variable>().write(Variable {
                name: 0,
                default: 0,
                hash: 0,
                has_default: 0,
            }),
            ObjectShapeId::ContextToken => p.cast::<Token>().write(Token {
                context: 0,
                variable: 0,
                old: 0,
                flags: USED,
            }),
            ObjectShapeId::ContextIterator => p.cast::<ContextIterator>().write(ContextIterator {
                root: 0,
                mode: 0,
                cursor: trie::Cursor::new(0),
            }),
            _ => unreachable!("wrong Context shape"),
        }
    }
}
