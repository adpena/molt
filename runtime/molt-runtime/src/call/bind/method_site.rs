// Compiled method call sites (call_anatomy.md 4.5 "Methods", invariant J3).
//
// Each `obj.m(args)` site owns one record in the program's writable data
// (`molt_codegen_abi::method_site`). Compiled code tests it inline and calls
// the recorded function's fixed-arity entry itself; this file is the site's
// cold side: the load miss that resolves the method and fills the record, the
// registry that invalidates call plans when a function's defaults or code
// change, and the release of every record edge at runtime teardown. The record
// is the site's only method cache.

use super::*;
use crate::builtins::attr::AttributeLookupMode;
use molt_codegen_abi::method_site::{
    METHOD_SITE_ARITY_OFFSET, METHOD_SITE_CLASS_OFFSET, METHOD_SITE_DEFAULT_COUNT_OFFSET,
    METHOD_SITE_DEFAULTS_OFFSET, METHOD_SITE_DICT_OFFSET, METHOD_SITE_EMPTY_CLASS,
    METHOD_SITE_ENTRY_OFFSET, METHOD_SITE_FUNCTION_OFFSET, METHOD_SITE_MAX_DEFAULTS,
    METHOD_SITE_NAME_BYTES_OFFSET, METHOD_SITE_NAME_LEN_OFFSET, METHOD_SITE_NAME_OFFSET,
    METHOD_SITE_NO_DICT, METHOD_SITE_REGISTERED_OFFSET, METHOD_SITE_SHAPE_OFFSET,
    METHOD_SITE_VERSION_OFFSET, method_site_header_shape,
};
use std::sync::Mutex;

/// One site record, addressed by byte offset. A wasm32 linker may place the
/// record's data segment at any byte, so every access tolerates an unaligned
/// address; native records are 8-byte aligned.
#[derive(Clone, Copy)]
struct Record(*mut u8);

impl Record {
    fn from_bits(bits: u64) -> Option<Self> {
        crate::provenance::abi::mut_ptr::<u8>(bits)
            .filter(|ptr| !ptr.is_null())
            .map(Self)
    }

    unsafe fn word(self, offset: i32) -> u64 {
        unsafe { self.0.add(offset as usize).cast::<u64>().read_unaligned() }
    }

    unsafe fn set(self, offset: i32, value: u64) {
        unsafe {
            self.0
                .add(offset as usize)
                .cast::<u64>()
                .write_unaligned(value)
        }
    }

    /// The record still describes `recv_ptr`'s class as it is now.
    unsafe fn class_matches(self, recv_ptr: *mut u8) -> Option<*mut u8> {
        unsafe {
            let class_bits = self.word(METHOD_SITE_CLASS_OFFSET);
            if class_bits == METHOD_SITE_EMPTY_CLASS || object_class_bits(recv_ptr) != class_bits {
                return None;
            }
            let class_ptr = obj_from_bits(class_bits).as_ptr()?;
            (class_layout_version_bits(class_ptr) == self.word(METHOD_SITE_VERSION_OFFSET))
                .then_some(class_ptr)
        }
    }

    /// Every owned edge, in the order the record holds them.
    unsafe fn owned_edges(self) -> [u64; 2 + METHOD_SITE_MAX_DEFAULTS] {
        unsafe {
            let class = self.word(METHOD_SITE_CLASS_OFFSET);
            let mut edges = [0; 2 + METHOD_SITE_MAX_DEFAULTS];
            edges[0] = if class == METHOD_SITE_EMPTY_CLASS {
                0
            } else {
                class
            };
            edges[1] = self.word(METHOD_SITE_FUNCTION_OFFSET);
            for (slot, edge) in edges[2..].iter_mut().enumerate() {
                *edge = self.word(METHOD_SITE_DEFAULTS_OFFSET + 8 * slot as i32);
            }
            edges
        }
    }

    /// Write `fill` as the record's contents and return the edges it held,
    /// which the caller releases after the record is coherent again.
    unsafe fn replace(self, fill: &Fill) -> [u64; 2 + METHOD_SITE_MAX_DEFAULTS] {
        unsafe {
            let previous = self.owned_edges();
            self.set(METHOD_SITE_CLASS_OFFSET, fill.class_bits);
            self.set(METHOD_SITE_SHAPE_OFFSET, fill.shape);
            self.set(METHOD_SITE_VERSION_OFFSET, fill.version);
            self.set(METHOD_SITE_DICT_OFFSET, fill.dict_offset);
            self.set(METHOD_SITE_FUNCTION_OFFSET, fill.func_bits);
            self.set(METHOD_SITE_ENTRY_OFFSET, fill.plan.entry);
            self.set(METHOD_SITE_ARITY_OFFSET, fill.plan.arity);
            self.set(METHOD_SITE_DEFAULT_COUNT_OFFSET, fill.plan.default_count);
            for (slot, &bits) in fill.plan.defaults.iter().enumerate() {
                self.set(METHOD_SITE_DEFAULTS_OFFSET + 8 * slot as i32, bits);
            }
            previous
        }
    }

    /// Empty the record's class side and call plan, as the program image
    /// left it, and return the edges they held. The name stays.
    unsafe fn clear(self) -> [u64; 2 + METHOD_SITE_MAX_DEFAULTS] {
        unsafe {
            let previous = self.owned_edges();
            let mut offset = 0;
            while offset < METHOD_SITE_REGISTERED_OFFSET {
                self.set(offset, 0);
                offset += 8;
            }
            self.set(METHOD_SITE_CLASS_OFFSET, METHOD_SITE_EMPTY_CLASS);
            previous
        }
    }

    /// The site's interned name and whether the caller owns it. A runtime
    /// that fills records interns the record's bytes on the site's first
    /// miss and keeps the name in the record; any other runtime interns a
    /// name of its own for this load. None: an exception is pending.
    unsafe fn name(self, py: &PyToken<'_>) -> Option<(u64, bool)> {
        unsafe {
            let cached = self.word(METHOD_SITE_NAME_OFFSET);
            if cached != 0 {
                return Some((cached, false));
            }
            let len = usize::try_from(self.word(METHOD_SITE_NAME_LEN_OFFSET)).ok()?;
            let bytes =
                std::slice::from_raw_parts(self.0.add(METHOD_SITE_NAME_BYTES_OFFSET as usize), len);
            let name = crate::dynamic_attr_name_bits(py, bytes)?;
            if !runtime_fills_records(py) {
                return Some((name, true));
            }
            self.set(METHOD_SITE_NAME_OFFSET, name);
            self.register(py);
            Some((name, false))
        }
    }

    /// Enter the record in the registry of the runtime that fills it.
    unsafe fn register(self, py: &PyToken<'_>) {
        unsafe {
            if self.word(METHOD_SITE_REGISTERED_OFFSET) == 0 {
                self.set(METHOD_SITE_REGISTERED_OFFSET, 1);
                let mut registry = RECORDS.lock().unwrap();
                registry.owner = crate::runtime_state(py).identity();
                registry.records.push(self.0.expose_provenance());
            }
        }
    }
}

fn release_edges(py: &PyToken<'_>, edges: [u64; 2 + METHOD_SITE_MAX_DEFAULTS]) {
    for bits in edges {
        if bits != 0 {
            dec_ref_bits(py, bits);
        }
    }
}

/// How the site's call reaches the function's fixed-arity entry.
#[derive(Default)]
struct CallPlan {
    entry: u64,
    arity: u64,
    default_count: u64,
    /// Right aligned and owned.
    defaults: [u64; METHOD_SITE_MAX_DEFAULTS],
}

/// A record's new contents. Every edge is owned.
struct Fill {
    class_bits: u64,
    shape: u64,
    version: u64,
    dict_offset: u64,
    func_bits: u64,
    plan: CallPlan,
}

/// Records the runtime has filled, for function-mutation invalidation and
/// teardown. Only the process's primary runtime fills records, so every
/// edge they hold belongs to it: `owner` is that runtime's identity, which
/// its teardown matches even after the lifecycle stops naming it primary.
struct Registry {
    owner: usize,
    records: Vec<usize>,
}

static RECORDS: Mutex<Registry> = Mutex::new(Registry {
    owner: 0,
    records: Vec::new(),
});

/// The site record words compiled code reads are plain memory shared by every
/// thread of the program. A free-threaded runtime and an isolate have no lock
/// that orders their writes with another runtime's reads, so they never fill
/// a record; their sites always take the runtime.
fn runtime_fills_records(py: &PyToken<'_>) -> bool {
    !cfg!(feature = "free-threaded")
        && crate::state::runtime_state::runtime_is_primary(crate::runtime_state(py))
}

/// The direct-entry call plan of `func_ptr`, or none when its calls need the
/// runtime: binding beyond trailing positional defaults, an execution kind
/// other than a plain frame, a closure, or a borrowing entry.
unsafe fn call_plan(py: &PyToken<'_>, func_bits: u64, func_ptr: *mut u8) -> CallPlan {
    unsafe {
        let Some(arity) = function_arity_usize(func_ptr) else {
            return CallPlan::default();
        };
        if crate::object::ops_builtins::direct_call_function(py, func_bits, arity, false, true)
            .is_none()
        {
            return CallPlan::default();
        }
        // The entry compiled code published as the function's executable
        // identity: its code address on native targets and its index in the
        // compiled module's own function table on wasm32, which is the table
        // the site's `call_indirect` reads, also when the runtime is a
        // separate module. A named key is a runtime callable, never a
        // compiled entry.
        let entry = function_fn_ptr(func_ptr);
        if entry == 0 || crate::builtins::functions::runtime_callable_key_is_named(entry) {
            return CallPlan::default();
        }
        let mut plan = CallPlan {
            entry,
            arity: arity as u64,
            ..CallPlan::default()
        };
        let defaults = obj_from_bits(frame_binding::function_binding_meta(
            py,
            func_ptr,
            FunctionBindingField::Defaults,
        ));
        if let Some(tuple) = defaults.as_ptr()
            && object_type_id(tuple) == TYPE_ID_TUPLE
        {
            crate::object::seq_access::with_immutable_tuple_slice(tuple, |items| {
                let usable = items.len().min(METHOD_SITE_MAX_DEFAULTS);
                let tail = &items[items.len() - usable..];
                for (slot, &bits) in plan.defaults[METHOD_SITE_MAX_DEFAULTS - usable..]
                    .iter_mut()
                    .zip(tail)
                {
                    inc_ref_bits(py, bits);
                    *slot = bits;
                }
                plan.default_count = usable as u64;
            });
        }
        plan
    }
}

/// What a method load at a site selects for a heap receiver.
enum Selection {
    /// A plain function found on the receiver's type (owned).
    Function(u64),
    /// The instance shadows the class function: its own attribute (owned).
    Shadow(u64),
    /// The site cannot answer; the generic attribute lookup does.
    Generic,
}

/// Select a plain class function for `recv_ptr.name` without a bound method,
/// filling the record when the selection is one compiled code can repeat.
/// Err means an exception is pending.
///
/// # Safety
/// `recv_ptr` is live; `name_bits` is a `str`; the GIL is held.
unsafe fn select(
    py: &PyToken<'_>,
    record: Record,
    recv_ptr: *mut u8,
    name_bits: u64,
) -> Result<Selection, ()> {
    unsafe {
        // The class side still holds and only the instance tier sent the load
        // here: an instance dictionary, which may shadow the method.
        if let Some(class_ptr) = record.class_matches(recv_ptr) {
            let func_bits = record.word(METHOD_SITE_FUNCTION_OFFSET);
            // Shadow lookup can run Python code that refills the record:
            // the load keeps the function it selected, as CPython's does.
            inc_ref_bits(py, func_bits);
            if record.word(METHOD_SITE_DICT_OFFSET) != METHOD_SITE_NO_DICT {
                match crate::builtins::attr::object_instance_attribute(
                    py, recv_ptr, class_ptr, name_bits,
                ) {
                    Ok(Some(value)) => {
                        dec_ref_bits(py, func_bits);
                        return Ok(Selection::Shadow(value));
                    }
                    Ok(None) => {}
                    Err(()) => {
                        dec_ref_bits(py, func_bits);
                        return Err(());
                    }
                }
            }
            return Ok(Selection::Function(func_bits));
        }

        let type_version = crate::object::global_type_version();
        let resolved =
            crate::builtins::attr::object_method_ic_resolve(py, recv_ptr, name_bits, |info| {
                // The function is pinned only inside this callback.
                inc_ref_bits(py, info.func_bits);
                info
            })?;
        let Some(info) = resolved else {
            return Ok(Selection::Generic);
        };
        if info.can_shadow() {
            let class_ptr = obj_from_bits(info.class_bits)
                .as_ptr()
                .expect("a resolved class is an object");
            match crate::builtins::attr::object_instance_attribute(
                py, recv_ptr, class_ptr, name_bits,
            ) {
                Ok(Some(value)) => {
                    dec_ref_bits(py, info.func_bits);
                    return Ok(Selection::Shadow(value));
                }
                Ok(None) => {}
                Err(()) => {
                    dec_ref_bits(py, info.func_bits);
                    return Err(());
                }
            }
        }
        fill(py, record, recv_ptr, &info, type_version);
        Ok(Selection::Function(info.func_bits))
    }
}

/// Fill `record` from a resolution the instance did not shadow, when compiled
/// code can repeat it: a class-inline header, a class whose instances may
/// hold an own attribute only in a dictionary, and a class and MRO unchanged
/// since the resolution began.
unsafe fn fill(
    py: &PyToken<'_>,
    record: Record,
    recv_ptr: *mut u8,
    info: &crate::builtins::attr::MethodIcResolution,
    type_version: u64,
) {
    unsafe {
        if info.field_shadow
            || !runtime_fills_records(py)
            || !inline_cache::type_resolution_epoch_is_stable(type_version)
            || object_class_bits(recv_ptr) != info.class_bits
        {
            return;
        }
        let Some(class_ptr) = obj_from_bits(info.class_bits).as_ptr() else {
            return;
        };
        if class_layout_version_bits(class_ptr) != info.class_version {
            return;
        }
        let header = &*header_from_obj_ptr(recv_ptr);
        if header.aux_kind != molt_codegen_abi::HEADER_AUX_KIND_CLASS_INLINE {
            return;
        }
        let dict_offset = if info.allows_dict {
            let slot = crate::object::instance_dict_bits_ptr(recv_ptr);
            if slot.is_null() {
                return;
            }
            (slot as usize - recv_ptr as usize) as u64
        } else {
            METHOD_SITE_NO_DICT
        };
        let Some(func_ptr) = obj_from_bits(info.func_bits).as_ptr() else {
            return;
        };
        let plan = call_plan(py, info.func_bits, func_ptr);
        inc_ref_bits(py, info.class_bits);
        inc_ref_bits(py, info.func_bits);
        let previous = record.replace(&Fill {
            class_bits: info.class_bits,
            shape: method_site_header_shape(header.size_class),
            version: info.class_version,
            dict_offset,
            func_bits: info.func_bits,
            plan,
        });
        record.register(py);
        // Releases may run finalizers that reach this site again: the record
        // is already coherent.
        release_edges(py, previous);
    }
}

/// CPython's `LOAD_ATTR` in method form at a compiled method site, after its
/// inline guard missed.
///
/// # Safety
/// `record` is a site record; the GIL is held.
unsafe fn load(py: &PyToken<'_>, record: Record, recv_bits: u64) -> LoadedMethod {
    unsafe {
        let Some((name_bits, owned_name)) = record.name(py) else {
            return LoadedMethod::failed();
        };
        let loaded = load_named(py, record, recv_bits, name_bits);
        if owned_name {
            dec_ref_bits(py, name_bits);
        }
        loaded
    }
}

/// The load with the site's interned name.
///
/// # Safety
/// As [`load`]; `name_bits` is a live `str`.
unsafe fn load_named(
    py: &PyToken<'_>,
    record: Record,
    recv_bits: u64,
    name_bits: u64,
) -> LoadedMethod {
    unsafe {
        if let Some(recv_ptr) = obj_from_bits(recv_bits).as_ptr() {
            match select(py, record, recv_ptr, name_bits) {
                Ok(Selection::Function(func_bits)) => {
                    let loaded = LoadedMethod::function(py, func_bits, recv_bits);
                    dec_ref_bits(py, func_bits);
                    return loaded;
                }
                Ok(Selection::Shadow(value)) => return LoadedMethod::attribute(value),
                Ok(Selection::Generic) => {}
                Err(()) => return LoadedMethod::failed(),
            }
        }
        // A data descriptor, a custom attribute hook, an instance attribute or
        // a receiver without a class shape. The record address doubles as the
        // site's id for the field tier of the generic lookup.
        let receiver = std::cell::Cell::new(None);
        let selected = crate::builtins::attributes::get_attr_object_site_with_mode(
            py,
            recv_bits,
            name_bits,
            record.0.expose_provenance() as u64,
            AttributeLookupMode::Method(&receiver),
        );
        match selected {
            Ok(callable) => match receiver.get() {
                Some(receiver) => {
                    let loaded = LoadedMethod::function(py, callable, receiver);
                    dec_ref_bits(py, callable);
                    loaded
                }
                None => LoadedMethod::attribute(callable),
            },
            Err(()) => LoadedMethod::failed(),
        }
    }
}

/// C-ABI entry for a compiled method site's load miss. Returns the callable
/// and writes `self_or_null` to `self_out_bits`, as `LOAD_ATTR` in method
/// form leaves its pair; both words are owned. `record_bits` addresses the
/// site's record (`molt_codegen_abi::method_site`), which names the
/// attribute.
///
/// # Safety
/// `record_bits` addresses a site record; `self_out_bits` encodes writable
/// storage for one `u64`.
pub(crate) fn molt_method_site_load_attached_body(
    _py: &crate::PyToken<'_>,
    record_bits: u64,
    recv_bits: u64,
    self_out_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let Some(record) = Record::from_bits(record_bits) else {
            return raise_exception::<u64>(
                _py,
                BuiltinExc::RuntimeError,
                "method site record address is invalid for the active target",
            );
        };
        publish_loaded_method(_py, self_out_bits, || unsafe {
            load(_py, record, recv_bits)
        })
    })
}

/// A user change to `func_ptr`'s binding metadata or code: no record may call
/// its old entry or append its old defaults. The loads still select the
/// function; their calls take the runtime, which reads the live metadata.
/// Writes no edge, so it runs no code.
pub(crate) fn forget_method_site_call_plans(func_ptr: *mut u8) {
    let func_bits = MoltObject::from_ptr(func_ptr).bits();
    let registry = RECORDS.lock().unwrap();
    for &address in registry.records.iter() {
        let record = Record(std::ptr::with_exposed_provenance_mut(address));
        unsafe {
            if record.word(METHOD_SITE_FUNCTION_OFFSET) == func_bits {
                record.set(METHOD_SITE_ENTRY_OFFSET, 0);
            }
        }
    }
}

/// Runtime teardown: empty every record the tearing-down runtime filled,
/// then release the edges they held, the names last. Returns whether any
/// edge existed.
pub(crate) fn release_method_site_records(py: &PyToken<'_>, state: &crate::RuntimeState) -> bool {
    let records = {
        let mut registry = RECORDS.lock().unwrap();
        if registry.owner != state.identity() {
            return false;
        }
        registry.owner = 0;
        std::mem::take(&mut registry.records)
    };
    let mut edges = Vec::with_capacity(records.len() * (3 + METHOD_SITE_MAX_DEFAULTS));
    let mut names = Vec::with_capacity(records.len());
    for address in records {
        let record = Record(std::ptr::with_exposed_provenance_mut(address));
        unsafe {
            edges.extend(record.clear());
            names.push(record.word(METHOD_SITE_NAME_OFFSET));
            record.set(METHOD_SITE_NAME_OFFSET, 0);
            record.set(METHOD_SITE_REGISTERED_OFFSET, 0);
        }
    }
    edges.extend(names);
    let mut released = false;
    for bits in edges {
        if bits != 0 {
            released = true;
            dec_ref_bits(py, bits);
        }
    }
    released
}

/// A site record in test memory, as a program image holds one.
#[cfg(test)]
pub(super) struct TestSite {
    words: Box<[u64]>,
}

#[cfg(test)]
impl TestSite {
    pub(super) fn new(name: &[u8]) -> Self {
        let size = molt_codegen_abi::method_site::method_site_record_size(name.len());
        let mut image = vec![0u8; size];
        molt_codegen_abi::method_site::write_method_site_record_image(name, &mut image);
        let words = image
            .chunks_exact(8)
            .map(|word| u64::from_le_bytes(word.try_into().unwrap()))
            .collect();
        Self { words }
    }

    fn record(&self) -> Record {
        Record(self.words.as_ptr().cast_mut().cast())
    }

    pub(super) fn word(&self, offset: i32) -> u64 {
        unsafe { self.record().word(offset) }
    }

    /// The site's load miss.
    pub(super) unsafe fn load(&self, py: &PyToken<'_>, recv_bits: u64) -> LoadedMethod {
        unsafe { load(py, self.record(), recv_bits) }
    }

    /// Unregister the record and release every edge it holds.
    pub(super) fn release(self, py: &PyToken<'_>) {
        let address = self.record().0.expose_provenance();
        RECORDS
            .lock()
            .unwrap()
            .records
            .retain(|&entry| entry != address);
        unsafe {
            release_edges(py, self.record().clear());
            let name = self.record().word(METHOD_SITE_NAME_OFFSET);
            if name != 0 {
                dec_ref_bits(py, name);
            }
        }
    }
}

include!("../../attached_exports_generated/call/bind/method_site.rs");

#[cfg(test)]
#[path = "method_site_tests.rs"]
mod tests;
