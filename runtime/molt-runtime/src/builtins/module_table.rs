//! The module import bedrock: `ModuleRegistry` + `ModuleTable` +
//! `molt_module_ensure` (design authority:
//! `docs/design/foundation/import_bedrock_frozen_module_layer.md`, PR1).
//!
//! Within a build a module's identity is its dense `ModuleId` (u32), assigned
//! in sorted-canonical-name order by the compile-time generator
//! (`molt.cli.module_registry`, the checked-in blob layout authority; this
//! file is the only reader).  The application object carries the registry as
//! one relocated data blob (`molt_module_registry_blob`) whose init-pointer
//! column is the `MODULE_INIT_TABLE`; the C main stub installs it here before
//! `molt_runtime_init`.
//!
//! This module owns every module-state transition (invariant I4). Compiled
//! literal import sites call `molt_module_ensure(id)` with a constant id,
//! and every dynamic lane (importlib transaction, `__import__`,
//! `PyImport_*`, runpy, thread payload imports) resolves string→id at most
//! once per call (`module_id_of`, binary search over the sorted name table —
//! the design's sanctioned resolver) and enters the same function. Normal
//! imports project the current public sys.modules dictionary before accessing
//! registered state; dynamic callers transfer their existing projection. This
//! path includes name lookup and locking. Bootstrap and suppressed execution
//! can use the private table/cache lane; its cost is not the public-import cost.
//!
//! Five observable states per row: {Uninit, Initializing, Ready, Tombstone,
//! Replaced}, plus an internal `ExecutionReserved` custody state used only by
//! transactional runpy/importlib re-execution. Init-exactly-once is the
//! `Uninit→Initializing` CAS (invariant
//! I5); publication happens before body execution (invariant I6) via the
//! body's own `MODULE_CACHE_SET`, which mirrors into `slots[id]` while this
//! ensure transaction is open (`publish_from_cache_set`). Public replacement
//! and deletion are reconciled at ensure entry through the existing typed
//! `module_table_view_replace`/`_tombstone` transitions. Trusted extension
//! publication uses the same projection transition immediately, releasing an
//! obsolete READY owner before its public replacement returns. A foreign initializer
//! still owns completion, so an observed partial module cannot bypass its wait.
//!
//! The private module_cache stores admitted runtime publications for bootstrap
//! and explicit runpy/loader execution. Public reads never populate it. Once
//! sys.modules exists, its misses and arbitrary values are authoritative for
//! normal imports; stale private entries cannot adopt a publicly deleted name.
//!
//! * wasm32 projects the same table through an app-owned integer ModuleId
//!   dispatcher.  Module names never cross the app/runtime ABI.
//! * Extension tombstones reenter the same initializer transaction. The
//!   extension primitive owns phase-aware reinitialization and legacy m_copy
//!   snapshots in the existing C-API runtime state owner.
//!
//! Platform note: everything here is target-neutral by construction (atomics +
//! GIL discipline; thread ids from `crate::concurrency::current_thread_id`);
//! there are no host-OS branches, so Windows/macOS/Linux share one code path.
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use molt_obj_model::MoltObject;

use crate::PyToken;
use crate::{
    alloc_string, dec_ref_bits, exception_pending, inc_ref_bits, obj_from_bits, raise_exception,
    runtime_state,
};

// ─── Blob layout (reader side; writer authority: molt.cli.module_registry) ──
// Structural gate: tests/test_module_registry_gates.py asserts these
// constants equal the Python authority's. Bump both in the same arc.

pub(crate) const MODULE_REGISTRY_SCHEMA_VERSION: u32 = 2;
const MODULE_REGISTRY_MAGIC: u64 = u64::from_le_bytes(*b"MOLTMOD2");
const MODULE_REGISTRY_HEADER_BYTES: usize = 48;
const MODULE_REGISTRY_ROW_BYTES: usize = 40;
const NO_MODULE_ID: u32 = u32::MAX;

const MODULE_KIND_SOURCE: u8 = 0;
const MODULE_KIND_EXTENSION: u8 = 1;
const MODULE_KIND_ALIAS: u8 = 2;
#[allow(dead_code)] // reserved by the schema; no namespace-parent rows in PR1
const MODULE_KIND_NAMESPACE_PARENT: u8 = 3;
const MODULE_KIND_RUNTIME_BUILTIN: u8 = 4;

#[allow(dead_code)] // consumed by the PR4 extension-snapshot reinit lane
const MODULE_FLAG_REINIT_RESURRECT: u8 = 0x01;
const MODULE_FLAG_PACKAGE: u8 = 0x02;
const MODULE_FLAG_HAS_BODY: u8 = 0x04;

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn molt_isolate_import(module_id: u64) -> u64;
}

// ─── Module state machine (design §4.1) ─────────────────────────────────────

const STATE_UNINIT: u8 = 0;
const STATE_INITIALIZING: u8 = 1;
const STATE_READY: u8 = 2;
const STATE_TOMBSTONE: u8 = 3;
const STATE_REPLACED: u8 = 4;
/// Transient custody for runpy/importlib fresh execution.  It reserves the row
/// before the normal ensure dispatcher enters Initializing, preventing a
/// free-threaded importer from winning the Tombstone race.
const STATE_EXECUTION_RESERVED: u8 = 5;

// ─── Registry (per-process, installed by the app bootstrap) ────────────────

#[derive(Clone, Copy)]
pub(crate) struct RegistryRow {
    pub(crate) init_ptr: u64,
    pub(crate) parent: Option<u32>,
    pub(crate) alias_target: Option<u32>,
    pub(crate) kind: u8,
    #[allow(dead_code)] // reinit policy flags; resurrect lane lands in PR4
    pub(crate) module_flags: u8,
}

#[derive(Debug)]
pub(crate) struct ModuleRegistry {
    base: *const u8,
    count: u32,
    names_off: usize,
    origins_off: usize,
    origins_len: usize,
    digest: [u8; 16],
}

// The blob is immutable static data in the executable image.
unsafe impl Send for ModuleRegistry {}
unsafe impl Sync for ModuleRegistry {}

impl ModuleRegistry {
    pub(crate) fn count(&self) -> u32 {
        self.count
    }

    pub(crate) fn digest(&self) -> &[u8; 16] {
        &self.digest
    }

    fn row_base(&self, id: u32) -> *const u8 {
        debug_assert!(id < self.count);
        let row_offset = MODULE_REGISTRY_HEADER_BYTES + (id as usize) * MODULE_REGISTRY_ROW_BYTES;
        unsafe { self.base.add(row_offset) }
    }

    fn read_u32(addr: *const u8) -> u32 {
        unsafe { addr.cast::<u32>().read_unaligned() }
    }

    fn read_u64(addr: *const u8) -> u64 {
        unsafe { addr.cast::<u64>().read_unaligned() }
    }

    pub(crate) fn name_of(&self, id: u32) -> &'static str {
        debug_assert!(id < self.count);
        let row = self.row_base(id);
        let name_off = Self::read_u32(row) as usize;
        let name_len = Self::read_u32(unsafe { row.add(4) }) as usize;
        let bytes = unsafe {
            std::slice::from_raw_parts(self.base.add(self.names_off + name_off), name_len)
        };
        // UTF-8 validity is checked once at install; fail closed on decode.
        std::str::from_utf8(bytes).expect("module registry names validated at install")
    }

    pub(crate) fn origin_of(&self, id: u32) -> &'static str {
        debug_assert!(id < self.count);
        let row = self.row_base(id);
        let origin_off = Self::read_u32(unsafe { row.add(28) }) as usize;
        let origin_len = Self::read_u32(unsafe { row.add(32) }) as usize;
        let bytes = unsafe {
            std::slice::from_raw_parts(self.base.add(self.origins_off + origin_off), origin_len)
        };
        std::str::from_utf8(bytes).expect("module registry origins validated at install")
    }

    pub(crate) fn row(&self, id: u32) -> RegistryRow {
        debug_assert!(id < self.count);
        let row = self.row_base(id);
        let parent = Self::read_u32(unsafe { row.add(16) });
        let alias_target = Self::read_u32(unsafe { row.add(20) });
        RegistryRow {
            init_ptr: Self::read_u64(unsafe { row.add(8) }),
            parent: (parent != NO_MODULE_ID).then_some(parent),
            alias_target: (alias_target != NO_MODULE_ID).then_some(alias_target),
            kind: unsafe { row.add(24).read() },
            module_flags: unsafe { row.add(25).read() },
        }
    }

    /// String → ModuleId, at most once per dynamic call site (design §4.2).
    /// Ids are assigned in sorted-name order, so the name table is sorted by
    /// construction and a binary search needs no auxiliary index.
    pub(crate) fn id_of(&self, name: &str) -> Option<u32> {
        let mut lo = 0u32;
        let mut hi = self.count;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            match self.name_of(mid).cmp(name) {
                std::cmp::Ordering::Equal => return Some(mid),
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
            }
        }
        None
    }

    fn parse(blob: *const u8) -> Result<Self, String> {
        if blob.is_null() {
            return Err("module registry blob pointer is null".to_string());
        }
        let base = blob;
        let magic = Self::read_u64(base);
        if magic != MODULE_REGISTRY_MAGIC {
            return Err(format!(
                "module registry blob magic mismatch: 0x{magic:016x} (artifact mixing?)"
            ));
        }
        let schema = Self::read_u32(unsafe { base.add(8) });
        if schema != MODULE_REGISTRY_SCHEMA_VERSION {
            return Err(format!(
                "module registry schema {schema} does not match runtime schema \
                 {MODULE_REGISTRY_SCHEMA_VERSION}; the binary and runtime come from \
                 different molt builds"
            ));
        }
        let count = Self::read_u32(unsafe { base.add(12) });
        let mut digest = [0u8; 16];
        digest.copy_from_slice(unsafe { std::slice::from_raw_parts(base.add(16), 16) });
        let names_len = usize::try_from(Self::read_u64(unsafe { base.add(32) }))
            .map_err(|_| "module registry name table exceeds the target address space")?;
        let origins_len = usize::try_from(Self::read_u64(unsafe { base.add(40) }))
            .map_err(|_| "module registry origin table exceeds the target address space")?;
        let names_off = usize::try_from(count)
            .ok()
            .and_then(|count| count.checked_mul(MODULE_REGISTRY_ROW_BYTES))
            .and_then(|rows_len| MODULE_REGISTRY_HEADER_BYTES.checked_add(rows_len))
            .ok_or("module registry row table exceeds the target address space")?;
        let origins_off = names_off
            .checked_add(names_len)
            .ok_or("module registry name table offset overflow")?;
        let total_len = origins_off
            .checked_add(origins_len)
            .ok_or("module registry origin table offset overflow")?;
        base.addr()
            .checked_add(total_len)
            .ok_or("module registry address range overflow")?;
        let registry = Self {
            base,
            count,
            names_off,
            origins_off,
            origins_len,
            digest,
        };
        // Validate every row once so all later reads are infallible.
        let mut previous_name: Option<&str> = None;
        for id in 0..count {
            let row_base = registry.row_base(id);
            let name_off = Self::read_u32(row_base) as usize;
            let name_len = Self::read_u32(unsafe { row_base.add(4) }) as usize;
            if name_off
                .checked_add(name_len)
                .is_none_or(|end| end > names_len)
            {
                return Err(format!(
                    "module registry row {id} name span exceeds the name table"
                ));
            }
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    registry.base.add(registry.names_off + name_off),
                    name_len,
                )
            };
            let name = std::str::from_utf8(bytes)
                .map_err(|_| format!("module registry row {id} name is not UTF-8"))?;
            let origin_off = Self::read_u32(unsafe { row_base.add(28) }) as usize;
            let origin_len = Self::read_u32(unsafe { row_base.add(32) }) as usize;
            if origin_off
                .checked_add(origin_len)
                .is_none_or(|end| end > registry.origins_len)
            {
                return Err(format!(
                    "module registry row {id} origin span exceeds the origin table"
                ));
            }
            let origin_bytes = unsafe {
                std::slice::from_raw_parts(
                    registry.base.add(registry.origins_off + origin_off),
                    origin_len,
                )
            };
            std::str::from_utf8(origin_bytes)
                .map_err(|_| format!("module registry row {id} origin is not UTF-8"))?;
            if let Some(previous) = previous_name
                && previous >= name
            {
                return Err(format!(
                    "module registry rows are not in sorted-name order at row {id}"
                ));
            }
            previous_name = Some(name);
            let row = registry.row(id);
            if let Some(parent) = row.parent
                && parent >= count
            {
                return Err(format!("module registry row {id} parent out of range"));
            }
            match row.kind {
                MODULE_KIND_ALIAS => {
                    let Some(target) = row.alias_target else {
                        return Err(format!("module registry alias row {id} has no target"));
                    };
                    if target >= count {
                        return Err(format!(
                            "module registry alias row {id} target out of range"
                        ));
                    }
                    if row.init_ptr != 0 {
                        return Err(format!(
                            "module registry alias row {id} carries an init pointer"
                        ));
                    }
                }
                MODULE_KIND_SOURCE
                | MODULE_KIND_EXTENSION
                | MODULE_KIND_NAMESPACE_PARENT
                | MODULE_KIND_RUNTIME_BUILTIN => {}
                other => {
                    return Err(format!("module registry row {id} has unknown kind {other}"));
                }
            }
        }
        // Alias rows are generated, but the runtime still validates the full
        // chain before accepting an artifact.  A cycle would otherwise recurse
        // indefinitely in both normal imports and fresh-execution dispatch.
        for id in 0..count {
            let mut current = id;
            for depth in 0..=count {
                let row = registry.row(current);
                if row.kind != MODULE_KIND_ALIAS {
                    break;
                }
                if depth == count {
                    return Err(format!(
                        "module registry alias chain from row {id} does not terminate"
                    ));
                }
                current = row.alias_target.expect("alias target validated above");
            }
        }
        Ok(registry)
    }
}

static INSTALLED_REGISTRY: OnceLock<ModuleRegistry> = OnceLock::new();

/// Install the application's registry blob.  Called by the C main stub before
/// `molt_runtime_init`; digest/schema/shape violations fail the process
/// immediately (a mixed-artifact binary must never reach an import).
#[unsafe(no_mangle)]
pub extern "C" fn molt_module_registry_install(blob: *const u8) -> u64 {
    match ModuleRegistry::parse(blob) {
        Ok(registry) => {
            let digest = *registry.digest();
            let installed = INSTALLED_REGISTRY.get_or_init(|| registry);
            if installed.digest() != &digest {
                eprintln!(
                    "molt: fatal: module registry re-install with a different digest \
                     (two application images in one process?)"
                );
                std::process::exit(70);
            }
            0
        }
        Err(err) => {
            eprintln!("molt: fatal: {err}");
            std::process::exit(70);
        }
    }
}

pub(crate) fn module_registry() -> Option<&'static ModuleRegistry> {
    INSTALLED_REGISTRY.get()
}

/// String → id against the installed registry (None when no registry is
/// installed — direct-link tests and embedding hosts without compiled apps).
pub(crate) fn module_id_of(name: &str) -> Option<u32> {
    module_registry()?.id_of(name)
}

/// Resolve a generated alias to the row that owns its initializer.  Fresh
/// execution must release and dispatch that terminal row; alias rows own no
/// body and remain ordinary co-publication views.
pub(crate) fn module_execution_target_name(name: &str) -> Option<&'static str> {
    let registry = module_registry()?;
    let mut id = registry.id_of(name)?;
    loop {
        let row = registry.row(id);
        if row.kind != MODULE_KIND_ALIAS {
            return Some(registry.name_of(id));
        }
        id = row
            .alias_target
            .expect("alias target validated at registry install");
    }
}

/// Catalog admission for fresh execution. `None` means no registry is
/// installed (direct-link tests/embedding); `Some(false)` is an authoritative
/// no-body row and must not enter the importer or allocate transaction state.
pub(crate) fn module_execution_target_has_body(name: &str) -> Option<bool> {
    let registry = module_registry()?;
    let mut id = registry.id_of(name)?;
    loop {
        let row = registry.row(id);
        if row.kind != MODULE_KIND_ALIAS {
            return Some(row.module_flags & MODULE_FLAG_HAS_BODY != 0);
        }
        id = row
            .alias_target
            .expect("alias target validated at registry install");
    }
}

pub(crate) fn module_catalog_is_package(name: &str) -> Option<bool> {
    let registry = module_registry()?;
    let id = registry.id_of(name)?;
    Some(registry.row(id).module_flags & MODULE_FLAG_PACKAGE != 0)
}

pub(crate) fn module_catalog_origin(name: &str) -> Option<&'static str> {
    let registry = module_registry()?;
    let id = registry.id_of(name)?;
    Some(registry.origin_of(id))
}

pub(crate) fn module_catalog_name_by_origin(origin: &str) -> Option<&'static str> {
    let registry = module_registry()?;
    (0..registry.count())
        .find(|&id| registry.origin_of(id) == origin)
        .map(|id| registry.name_of(id))
}

// ─── ModuleTable (one per isolate, design §4.1) ─────────────────────────────

const STATE_RETIRED: u8 = 6;

/// One interpreter-owned sys namespace, distinct from mutable import views.
/// Zero is uninitialized; None is terminal retirement. Only the current
/// canonical initializer may establish the first module owner.
pub(crate) struct InterpreterSysNamespace(AtomicU64);

impl InterpreterSysNamespace {
    pub(crate) fn new() -> Self {
        Self(AtomicU64::new(0))
    }

    /// Borrow under the GIL. Public cache edits cannot release this owner.
    pub(crate) fn module(&self, _py: &PyToken<'_>) -> Option<u64> {
        crate::gil_assert();
        let bits = self.0.load(Ordering::Acquire);
        (bits != 0 && !is_none_bits(bits)).then_some(bits)
    }

    pub(crate) fn allows_bootstrap(&self, _py: &PyToken<'_>) -> bool {
        crate::gil_assert();
        self.0.load(Ordering::Acquire) == 0
    }

    fn publish(&self, py: &PyToken<'_>, bits: u64) -> bool {
        crate::gil_assert();
        if !obj_from_bits(bits)
            .as_ptr()
            .is_some_and(|ptr| unsafe { crate::object_type_id(ptr) == crate::TYPE_ID_MODULE })
        {
            raise_exception::<u64>(
                py,
                "TypeError",
                "canonical sys initializer must publish a module",
            );
            return false;
        }
        inc_ref_bits(py, bits);
        if self
            .0
            .compare_exchange(0, bits, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            dec_ref_bits(py, bits);
        }
        true
    }

    /// Transfer exactly one owner to ModuleRetirement's sys cohort.
    pub(crate) fn take_for_shutdown(&self, _py: &PyToken<'_>) -> Option<u64> {
        crate::gil_assert();
        let bits = self.0.swap(none_bits(), Ordering::AcqRel);
        (bits != 0 && !is_none_bits(bits)).then_some(bits)
    }
}

/// Synthetic test namespaces enter through the real initializer publication
/// bridge. RuntimeTestTransaction escrows and restores this role and table row.
#[cfg(test)]
pub(crate) fn publish_interpreter_sys_for_test(py: &PyToken<'_>, bits: u64) -> u64 {
    tests::install_test_registry();
    let table = module_table(py).unwrap();
    let index = module_id_of("sys").unwrap() as usize;
    let previous = table.slots[index].swap(0, Ordering::AcqRel);
    table.states[index].store(STATE_INITIALIZING, Ordering::Release);
    table.owners[index].store(crate::concurrency::current_thread_id(), Ordering::Release);
    let name = crate::attr_name_bits_from_bytes(py, b"sys").unwrap();
    let result = crate::builtins::modules::molt_module_cache_set(name, bits);
    dec_ref_bits(py, name);
    table.owners[index].store(0, Ordering::Release);
    table.states[index].store(STATE_READY, Ordering::Release);
    if previous != 0 {
        dec_ref_bits(py, previous);
    }
    result
}

#[cfg(test)]
pub(crate) struct InterpreterSysTestSnapshot {
    namespace: u64,
    table: Option<(u8, u64, u64)>,
}

#[cfg(test)]
impl InterpreterSysTestSnapshot {
    pub(crate) fn detach(py: &PyToken<'_>) -> Self {
        let state = runtime_state(py);
        let namespace = state.interpreter_sys.0.swap(0, Ordering::AcqRel);
        let table = state.module_table.get().and_then(|table| {
            let index = module_id_of("sys")? as usize;
            Some((
                table.states[index].swap(STATE_UNINIT, Ordering::AcqRel),
                table.slots[index].swap(0, Ordering::AcqRel),
                table.owners[index].swap(0, Ordering::AcqRel),
            ))
        });
        Self { namespace, table }
    }

    pub(crate) fn restore(self, py: &PyToken<'_>) {
        let state = runtime_state(py);
        let current = state
            .interpreter_sys
            .0
            .swap(self.namespace, Ordering::AcqRel);
        let current_table = state.module_table.get().and_then(|table| {
            let index = module_id_of("sys")? as usize;
            let (status, bits, owner) = self.table.unwrap_or((STATE_UNINIT, 0, 0));
            table.states[index].store(status, Ordering::Release);
            table.owners[index].store(owner, Ordering::Release);
            Some(table.slots[index].swap(bits, Ordering::AcqRel))
        });
        for bits in [Some(current), current_table].into_iter().flatten() {
            if bits != 0 && !is_none_bits(bits) {
                dec_ref_bits(py, bits);
            }
        }
    }
}

pub(crate) struct ModuleTable {
    states: Box<[AtomicU8]>,
    slots: Box<[AtomicU64]>,
    owners: Box<[AtomicU64]>,
    /// Wait-for edges for cross-thread cyclic-import detection (the CPython
    /// `_blocking_on` analog, design R2.1): initiating thread id → module id
    /// it is waiting on.
    blocking_on: Mutex<HashMap<u64, u32>>,
}

impl ModuleTable {
    fn new(count: u32) -> Self {
        let n = count as usize;
        Self {
            states: (0..n).map(|_| AtomicU8::new(STATE_UNINIT)).collect(),
            slots: (0..n).map(|_| AtomicU64::new(0)).collect(),
            owners: (0..n).map(|_| AtomicU64::new(0)).collect(),
            blocking_on: Mutex::new(HashMap::new()),
        }
    }
}

/// Retain a canonical role independently of its mutable public/private views.
pub(crate) fn retain_shutdown_namespace(
    py: &PyToken<'_>,
    state: &crate::state::RuntimeState,
    name: &str,
) -> Option<u64> {
    crate::gil_assert();
    let table = state.module_table.get()?;
    let id = module_registry()?.id_of(name)?;
    let bits = table.slots[id as usize].load(Ordering::Acquire);
    if bits == 0 || is_none_bits(bits) {
        return None;
    }
    inc_ref_bits(py, bits);
    Some(bits)
}

/// Transfer the selected registry owners to the shared shutdown cohort.
/// No callback runs until all selected rows have terminal state and zero slots.
pub(crate) fn take_module_roots_for_shutdown(
    _py: &PyToken<'_>,
    state: &crate::state::RuntimeState,
    keep: impl Fn(&str, u64) -> bool,
) -> Vec<u64> {
    crate::gil_assert();
    let (Some(table), Some(registry)) = (state.module_table.get(), module_registry()) else {
        return Vec::new();
    };
    let mut detached = Vec::with_capacity(table.slots.len());
    for (index, slot) in table.slots.iter().enumerate() {
        let bits = slot.load(Ordering::Acquire);
        if keep(registry.name_of(index as u32), bits) {
            continue;
        }
        table.states[index].store(STATE_RETIRED, Ordering::Release);
        table.owners[index].store(0, Ordering::Release);
        let bits = slot.swap(0, Ordering::AcqRel);
        if bits != 0 {
            detached.push(bits);
        }
    }
    detached
}

fn module_table(_py: &PyToken<'_>) -> Option<&'static ModuleTable> {
    let registry = module_registry()?;
    Some(
        runtime_state(_py)
            .module_table
            .get_or_init(|| ModuleTable::new(registry.count())),
    )
}

fn none_bits() -> u64 {
    MoltObject::none().bits()
}

fn is_none_bits(bits: u64) -> bool {
    obj_from_bits(bits).is_none()
}

fn legacy_cache_lookup(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    let cache = crate::builtins::exceptions::internals::module_cache(_py);
    let guard = cache.lock().unwrap();
    guard.get(name).copied().filter(|bits| *bits != 0)
}

fn legacy_cache_set(_py: &PyToken<'_>, name: &str, bits: u64) {
    let name_ptr = alloc_string(_py, name.as_bytes());
    if name_ptr.is_null() {
        return;
    }
    let name_bits = MoltObject::from_ptr(name_ptr).bits();
    let _ = crate::builtins::modules::molt_module_cache_set(name_bits, bits);
    dec_ref_bits(_py, name_bits);
}

// Used by the typed deletion transition after a public cache miss.
#[allow(dead_code)]
fn legacy_cache_del(_py: &PyToken<'_>, name: &str) {
    let name_ptr = alloc_string(_py, name.as_bytes());
    if name_ptr.is_null() {
        return;
    }
    let name_bits = MoltObject::from_ptr(name_ptr).bits();
    let _ = crate::builtins::modules::molt_module_cache_del(name_bits);
    dec_ref_bits(_py, name_bits);
}

// ─── Publication bridges (PR1: table ↔ legacy store coherence) ──────────────

/// A first-publication slot owned by the current initializer, never a
/// same-named cached object, reserved row, or another thread's transaction.
fn initializing_publication_slot(_py: &PyToken<'_>, name: &str) -> Option<&'static AtomicU64> {
    let id = module_id_of(name)?;
    let table = module_table(_py)?;
    let idx = id as usize;
    (table.states[idx].load(Ordering::Acquire) == STATE_INITIALIZING
        && table.owners[idx].load(Ordering::Acquire) == crate::concurrency::current_thread_id()
        && table.slots[idx].load(Ordering::Acquire) == 0)
        .then_some(&table.slots[idx])
}

pub(crate) fn module_initialization_awaits_publication(_py: &PyToken<'_>, name: &str) -> bool {
    initializing_publication_slot(_py, name).is_some()
}

/// Mirror `MODULE_CACHE_SET` while its ensure transaction owns publication.
pub(crate) fn publish_from_cache_set(_py: &PyToken<'_>, name: &str, bits: u64) {
    if bits == 0 || is_none_bits(bits) {
        return;
    }
    let Some(slot) = initializing_publication_slot(_py, name) else {
        return;
    };
    if name == "sys" && !runtime_state(_py).interpreter_sys.publish(_py, bits) {
        return;
    }
    inc_ref_bits(_py, bits);
    slot.store(bits, Ordering::Release);
}

/// A trusted extension result reconciles quiescent public-cache projections
/// immediately. An active foreign initializer or reserved loader execution
/// retains its custody. Return the displaced owner for release after all cache
/// stores agree, without manufacturing a READY transition.
pub(crate) fn publish_extension_result(py: &PyToken<'_>, name: &str, bits: u64) -> u64 {
    let Some(id) = module_id_of(name) else {
        return 0;
    };
    let Some(table) = module_table(py) else {
        return 0;
    };
    let idx = id as usize;
    match table.states[idx].load(Ordering::Acquire) {
        STATE_INITIALIZING => {
            if table.owners[idx].load(Ordering::Acquire) != crate::concurrency::current_thread_id()
            {
                return 0;
            }
            inc_ref_bits(py, bits);
            table.slots[idx].swap(bits, Ordering::AcqRel)
        }
        STATE_EXECUTION_RESERVED => 0,
        _ => module_table_view_replace_deferred(py, id, bits),
    }
}

/// Detach failed-init publication while the current ensure transaction owns
/// it, or an identity-matched quiescent publication being explicitly removed.
/// Ready and replaced slots have the same owning-cache retirement obligation;
/// foreign initializers and reserved execution retain their custody.
pub(crate) fn detach_cache_publication(
    _py: &PyToken<'_>,
    name: &str,
    expected: Option<u64>,
) -> u64 {
    let Some(registry) = module_registry() else {
        return 0;
    };
    let Some(id) = registry.id_of(name) else {
        return 0;
    };
    let Some(table) = module_table(_py) else {
        return 0;
    };
    let idx = id as usize;
    let state = table.states[idx].load(Ordering::Acquire);
    if state == STATE_INITIALIZING {
        if table.owners[idx].load(Ordering::Acquire) != crate::concurrency::current_thread_id() {
            return 0;
        }
    } else if expected.is_some() && matches!(state, STATE_READY | STATE_REPLACED) {
        let own = expected.unwrap();
        if table.slots[idx]
            .compare_exchange(own, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            table.states[idx].store(STATE_TOMBSTONE, Ordering::Release);
            return own;
        }
        return 0;
    } else {
        return 0;
    }
    match expected {
        Some(bits) => table.slots[idx]
            .compare_exchange(bits, 0, Ordering::AcqRel, Ordering::Acquire)
            .unwrap_or(0),
        None => table.slots[idx].swap(0, Ordering::AcqRel),
    }
}

// ─── The dict-view mutation entry points (design §4.4; wired to the
//     public-cache projection at ensure entry) ────────

/// `sys.modules[name] = obj` over a registry name → `Replaced(obj)`.
/// Called by normal ensure after observing the public cache; also tested directly.
pub(crate) fn module_table_view_replace(_py: &PyToken<'_>, id: u32, bits: u64) {
    let previous = module_table_view_replace_deferred(_py, id, bits);
    if previous != 0 {
        dec_ref_bits(_py, previous);
    }
}

fn module_table_view_replace_deferred(_py: &PyToken<'_>, id: u32, bits: u64) -> u64 {
    let Some(table) = module_table(_py) else {
        return 0;
    };
    let idx = id as usize;
    if table.states[idx].load(Ordering::Acquire) == STATE_RETIRED {
        return 0;
    }
    inc_ref_bits(_py, bits);
    let previous = table.slots[idx].swap(bits, Ordering::AcqRel);
    table.states[idx].store(STATE_REPLACED, Ordering::Release);
    previous
}

/// `del sys.modules[name]` over a registry name → `Tombstone`.
/// Called by normal ensure after observing the public cache; also tested directly.
#[allow(dead_code)]
pub(crate) fn module_table_view_tombstone(_py: &PyToken<'_>, id: u32) {
    let Some(table) = module_table(_py) else {
        return;
    };
    let Some(registry) = module_registry() else {
        return;
    };
    let idx = id as usize;
    if table.states[idx].load(Ordering::Acquire) == STATE_RETIRED {
        return;
    }
    table.states[idx].store(STATE_TOMBSTONE, Ordering::Release);
    let previous = table.slots[idx].swap(0, Ordering::AcqRel);
    let private = {
        let cache = crate::builtins::exceptions::internals::module_cache(_py);
        let mut guard = cache.lock().unwrap();
        guard.remove(registry.name_of(id))
    };
    // Public absence was observed by the caller. Detach both remaining owners
    // before any finalizer can reimport; never delete its new publication later.
    for bits in [Some(previous), private].into_iter().flatten() {
        if bits != 0 {
            dec_ref_bits(_py, bits);
        }
    }
}

// ─── ensure: the only state-transition owner (design §4.3) ──────────────────

/// Depth guard for alias chains and parent recursion; the registry validates
/// alias chains terminate, so this is a fail-closed backstop only.
pub(crate) struct ModuleExecutionSnapshot {
    id: u32,
    state: u8,
    bits: u64,
}

/// Temporarily release one compiled row for a fresh execution transaction.
pub(crate) fn begin_module_execution(
    _py: &PyToken<'_>,
    name: &str,
) -> Result<Option<ModuleExecutionSnapshot>, u64> {
    let Some(registry) = module_registry() else {
        return Ok(None);
    };
    let Some(id) = registry.id_of(name) else {
        return Ok(None);
    };
    let Some(table) = module_table(_py) else {
        return Ok(None);
    };
    let idx = id as usize;
    if table.states[idx].load(Ordering::Acquire) == STATE_RETIRED {
        return Err(raise_exception::<_>(
            _py,
            "ImportError",
            "module namespace has been retired during runtime shutdown",
        ));
    }
    let self_tid = crate::concurrency::current_thread_id();
    let state = loop {
        let state = table.states[idx].load(Ordering::Acquire);
        if state == STATE_INITIALIZING || state == STATE_EXECUTION_RESERVED {
            if table.owners[idx].load(Ordering::Acquire) == self_tid {
                return Err(raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    &format!("module {name:?} is already executing on this thread"),
                ));
            }
            if wait_for_foreign_init(_py, table, id, self_tid) {
                return Err(raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    &format!(
                        "cannot start fresh execution of module {name:?} during a concurrent import cycle"
                    ),
                ));
            }
            continue;
        }
        if table.states[idx]
            .compare_exchange(
                state,
                STATE_EXECUTION_RESERVED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            break state;
        }
    };
    table.owners[idx].store(self_tid, Ordering::Release);
    let bits = table.slots[idx].swap(0, Ordering::AcqRel);
    Ok(Some(ModuleExecutionSnapshot { id, state, bits }))
}

/// Restore the exact table state displaced by `begin_module_execution`.
pub(crate) fn restore_module_execution(
    _py: &PyToken<'_>,
    snapshot: Option<ModuleExecutionSnapshot>,
) {
    let Some(snapshot) = snapshot else {
        return;
    };
    let Some(table) = module_table(_py) else {
        if snapshot.bits != 0 {
            dec_ref_bits(_py, snapshot.bits);
        }
        return;
    };
    let idx = snapshot.id as usize;
    let fresh = table.slots[idx].swap(snapshot.bits, Ordering::AcqRel);
    table.owners[idx].store(0, Ordering::Release);
    table.states[idx].store(snapshot.state, Ordering::Release);
    if fresh != 0 {
        dec_ref_bits(_py, fresh);
    }
}

static ENSURE_DEPTH: AtomicUsize = AtomicUsize::new(0);
const ENSURE_MAX_DEPTH: usize = 4096;

/// Whether this thread owns execution of this exact published module object.
/// A reserved row has no published namespace yet; naming its prior cache entry
/// does not grant the new execution's privileges to that displaced object.
pub(crate) fn module_execution_owns_initializing_namespace(
    _py: &PyToken<'_>,
    name: &str,
    module_bits: u64,
) -> bool {
    if module_bits == 0 || is_none_bits(module_bits) {
        return false;
    }
    let Some(id) = module_id_of(name) else {
        return false;
    };
    let Some(table) = module_table(_py) else {
        return false;
    };
    let idx = id as usize;
    matches!(
        table.states[idx].load(Ordering::Acquire),
        STATE_INITIALIZING | STATE_EXECUTION_RESERVED
    ) && table.owners[idx].load(Ordering::Acquire) == crate::concurrency::current_thread_id()
        && table.slots[idx].load(Ordering::Acquire) == module_bits
}

/// ABI entry: `ensure(const ModuleId)` — what compiled literal import sites
/// call.  The argument is a NaN-boxed integer module id (runtime-call
/// arguments are boxed values, the same convention as every other `call`
/// op); the constant is boxed at the emit site.  Returns an owned module
/// reference, or none-bits with the pending exception set.
#[unsafe(no_mangle)]
pub extern "C" fn molt_module_ensure(id_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(id) = crate::to_i64(obj_from_bits(id_bits)) else {
            return raise_exception::<_>(
                _py,
                "SystemError",
                "module ensure expects a boxed integer module id",
            );
        };
        let Ok(id) = u32::try_from(id) else {
            return raise_exception::<_>(_py, "SystemError", "module id out of range");
        };
        module_ensure(_py, id)
    })
}

pub(crate) fn module_ensure(py: &PyToken<'_>, id: u32) -> u64 {
    module_ensure_with_cache(py, id, None)
}

/// Consume an already observed public-cache owner from the dynamic importer,
/// or acquire that projection once for a direct compiled import.
pub(crate) fn module_ensure_with_cache(
    _py: &PyToken<'_>,
    id: u32,
    observed: Option<crate::builtins::modules::PublicModuleCache>,
) -> u64 {
    let Some(registry) = module_registry() else {
        if let Some(crate::builtins::modules::PublicModuleCache::Present(bits)) = observed {
            dec_ref_bits(_py, bits);
        }
        return raise_exception::<_>(
            _py,
            "SystemError",
            "module registry is not installed; compiled import sites require the \
             application bootstrap to call molt_module_registry_install",
        );
    };
    if id >= registry.count() {
        if let Some(crate::builtins::modules::PublicModuleCache::Present(bits)) = observed {
            dec_ref_bits(_py, bits);
        }
        return raise_exception::<_>(_py, "SystemError", "module id outside the registry");
    }
    let depth = ENSURE_DEPTH.fetch_add(1, Ordering::Relaxed);
    let result = if depth >= ENSURE_MAX_DEPTH {
        if let Some(crate::builtins::modules::PublicModuleCache::Present(bits)) = observed {
            dec_ref_bits(_py, bits);
        }
        raise_exception::<_>(
            _py,
            "RecursionError",
            "module ensure recursion limit exceeded (registry cycle?)",
        )
    } else {
        module_ensure_inner(_py, registry, id, observed)
    };
    ENSURE_DEPTH.fetch_sub(1, Ordering::Relaxed);
    result
}

fn module_ensure_inner(
    _py: &PyToken<'_>,
    registry: &'static ModuleRegistry,
    id: u32,
    mut observed: Option<crate::builtins::modules::PublicModuleCache>,
) -> u64 {
    let row = registry.row(id);
    let Some(table) = module_table(_py) else {
        if let Some(crate::builtins::modules::PublicModuleCache::Present(bits)) = observed {
            dec_ref_bits(_py, bits);
        }
        return raise_exception::<_>(_py, "SystemError", "module table unavailable");
    };
    let idx = id as usize;
    if table.states[idx].load(Ordering::Acquire) == STATE_RETIRED {
        if let Some(crate::builtins::modules::PublicModuleCache::Present(bits)) = observed {
            dec_ref_bits(_py, bits);
        }
        return raise_exception::<_>(
            _py,
            "ImportError",
            "module namespace has been retired during runtime shutdown",
        );
    }
    let self_tid = crate::concurrency::current_thread_id();
    let mut parent_ready = false;
    loop {
        // Normal compiled imports see the same cache as dynamic imports. The
        // existing typed view transitions retire stale registered slots on public
        // deletion/replacement; bootstrap and explicit execution scopes bypass it.
        let public = match observed.take() {
            Some(public) => public,
            None => match crate::builtins::modules::public_module_cache_lookup(
                _py,
                registry.name_of(id),
            ) {
                Ok(public) => public,
                Err(error) => return error,
            },
        };
        let private_cache_available = match public {
            crate::builtins::modules::PublicModuleCache::Present(bits) => {
                let foreign_execution = module_table(_py).is_some_and(|table| {
                    matches!(
                        table.states[id as usize].load(Ordering::Acquire),
                        STATE_INITIALIZING | STATE_EXECUTION_RESERVED
                    ) && table.owners[id as usize].load(Ordering::Acquire)
                        != crate::concurrency::current_thread_id()
                });
                if foreign_execution {
                    // A visible partial/old namespace cannot bypass the existing
                    // wait-for and cycle handling for a foreign initializer.
                    dec_ref_bits(_py, bits);
                    false
                } else {
                    if obj_from_bits(bits).is_none() {
                        dec_ref_bits(_py, bits);
                        return raise_exception::<_>(
                            _py,
                            "ModuleNotFoundError",
                            &format!(
                                "import of {} halted; None in sys.modules",
                                registry.name_of(id)
                            ),
                        );
                    }
                    if let Some(table) = module_table(_py)
                        && table.slots[id as usize].load(Ordering::Acquire) != bits
                        && !matches!(
                            table.states[id as usize].load(Ordering::Acquire),
                            STATE_INITIALIZING | STATE_EXECUTION_RESERVED
                        )
                    {
                        module_table_view_replace(_py, id, bits);
                    }
                    return bits;
                }
            }
            crate::builtins::modules::PublicModuleCache::Missing => {
                let state = table.states[idx].load(Ordering::Acquire);
                // Bootstrap may have published privately before this row was
                // first ensured. A public deletion retires that owner too;
                // otherwise the initializer's publication would revive it.
                if matches!(state, STATE_READY | STATE_REPLACED)
                    || (state == STATE_UNINIT
                        && legacy_cache_lookup(_py, registry.name_of(id)).is_some())
                {
                    module_table_view_tombstone(_py, id);
                    if exception_pending(_py) {
                        return none_bits();
                    }
                    continue;
                }
                false
            }
            crate::builtins::modules::PublicModuleCache::Unavailable => true,
        };
        if row.kind == MODULE_KIND_ALIAS {
            // Aliases own no init and no separate transaction: resolve the
            // target, co-publish under the alias row (design §4.3).
            let target = row.alias_target.expect("alias target validated at install");
            let bits = module_ensure(_py, target);
            if exception_pending(_py) || is_none_bits(bits) {
                return bits;
            }
            publish_alias(_py, registry, id, bits);
            return bits;
        }
        match table.states[idx].load(Ordering::Acquire) {
            STATE_READY => {
                // Registered result after public-cache/suppression admission.
                let bits = table.slots[idx].load(Ordering::Acquire);
                inc_ref_bits(_py, bits);
                return bits;
            }
            STATE_REPLACED => {
                let bits = table.slots[idx].load(Ordering::Acquire);
                if is_none_bits(bits) {
                    // CPython parity row 5.2: None in sys.modules halts the
                    // import with ModuleNotFoundError, exact message.
                    let name = registry.name_of(id);
                    return raise_exception::<_>(
                        _py,
                        "ModuleNotFoundError",
                        &format!("import of {name} halted; None in sys.modules"),
                    );
                }
                inc_ref_bits(_py, bits);
                return bits;
            }
            STATE_INITIALIZING => {
                if table.owners[idx].load(Ordering::Acquire) == self_tid {
                    // Same-thread circular import: return the partial module
                    // published before body exec (CPython parity row 5.4).
                    let bits = table.slots[idx].load(Ordering::Acquire);
                    if bits == 0 {
                        let name = registry.name_of(id);
                        return raise_exception::<_>(
                            _py,
                            "ImportError",
                            &format!(
                                "cannot import partially initialized module '{name}' \
                                 before its publication (circular import during \
                                 module allocation)"
                            ),
                        );
                    }
                    inc_ref_bits(_py, bits);
                    return bits;
                }
                if wait_for_foreign_init(_py, table, id, self_tid) {
                    // Cross-thread deadlock detected: accept the partially
                    // initialized module, exactly CPython's
                    // `_lock_unlock_module` behavior (parity row 5.10).
                    let bits = table.slots[idx].load(Ordering::Acquire);
                    if bits != 0 {
                        inc_ref_bits(_py, bits);
                        return bits;
                    }
                    let name = registry.name_of(id);
                    return raise_exception::<_>(
                        _py,
                        "ImportError",
                        &format!(
                            "cannot import partially initialized module '{name}' \
                             (concurrent circular import before publication)"
                        ),
                    );
                }
                continue;
            }
            STATE_EXECUTION_RESERVED => {
                if table.owners[idx].load(Ordering::Acquire) == self_tid {
                    if table.states[idx]
                        .compare_exchange(
                            STATE_EXECUTION_RESERVED,
                            STATE_INITIALIZING,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_err()
                    {
                        continue;
                    }
                    return run_init_transaction(_py, registry, table, id, row.init_ptr);
                }
                if wait_for_foreign_init(_py, table, id, self_tid) {
                    let name = registry.name_of(id);
                    return raise_exception::<_>(
                        _py,
                        "ImportError",
                        &format!(
                            "cannot import module '{name}' during a concurrent fresh-execution transaction"
                        ),
                    );
                }
                continue;
            }
            STATE_TOMBSTONE => {
                if !parent_ready {
                    if !ensure_parent_ready(_py, &row) {
                        return none_bits();
                    }
                    parent_ready = true;
                    // Parent code can publish/replace this child.
                    continue;
                }
                // The existing initializer owns reconstruction: source bodies
                // rerun, while extension_init applies the phase-specific rule.
                if table.states[idx]
                    .compare_exchange(
                        STATE_TOMBSTONE,
                        STATE_INITIALIZING,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_err()
                {
                    continue;
                }
                table.owners[idx].store(self_tid, Ordering::Release);
                return run_init_transaction(_py, registry, table, id, row.init_ptr);
            }
            STATE_UNINIT => {
                // Parent-first must precede the child's state transition.  If
                // the parent imports this child while building its metadata,
                // the recursive child ensure may win and publish it; our CAS
                // below then loses and re-reads the completed state.  Marking
                // the child Initializing first creates a false slot-less
                // circular import.
                if !parent_ready {
                    if !ensure_parent_ready(_py, &row) {
                        return none_bits();
                    }
                    parent_ready = true;
                    // Refresh public child state before claiming execution.
                    continue;
                }
                // PR1 adoption bridge: the legacy store may already own the
                // module (entry-module dual publication, host preloads).  The
                // adoption is itself an ensure-owned transition.
                if private_cache_available
                    && let Some(bits) = legacy_cache_lookup(_py, registry.name_of(id))
                {
                    if table.states[idx]
                        .compare_exchange(
                            STATE_UNINIT,
                            STATE_INITIALIZING,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_err()
                    {
                        continue;
                    }
                    table.owners[idx].store(self_tid, Ordering::Release);
                    inc_ref_bits(_py, bits);
                    table.slots[idx].store(bits, Ordering::Release);
                    table.owners[idx].store(0, Ordering::Release);
                    table.states[idx].store(STATE_READY, Ordering::Release);
                    inc_ref_bits(_py, bits);
                    return bits;
                }
                if table.states[idx]
                    .compare_exchange(
                        STATE_UNINIT,
                        STATE_INITIALIZING,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_err()
                {
                    continue;
                }
                table.owners[idx].store(self_tid, Ordering::Release);
                return run_init_transaction(_py, registry, table, id, row.init_ptr);
            }
            other => {
                return raise_exception::<_>(
                    _py,
                    "SystemError",
                    &format!("module table state {other} is not a known ModuleState"),
                );
            }
        }
    }
}

fn ensure_parent_ready(_py: &PyToken<'_>, row: &RegistryRow) -> bool {
    let Some(parent) = row.parent else {
        return true;
    };
    let parent_bits = module_ensure(_py, parent);
    if exception_pending(_py) {
        if !is_none_bits(parent_bits) {
            dec_ref_bits(_py, parent_bits);
        }
        return false;
    }
    dec_ref_bits(_py, parent_bits);
    true
}

/// The Uninit→Ready init transaction body.  Caller has already won the CAS
/// into `Initializing` and set `owners[id]`.
fn trace_ensure() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MOLT_TRACE_MODULE_ENSURE").as_deref() == Ok("1"))
}

fn run_init_transaction(
    _py: &PyToken<'_>,
    registry: &'static ModuleRegistry,
    table: &'static ModuleTable,
    id: u32,
    init_ptr: u64,
) -> u64 {
    let idx = id as usize;
    let name = registry.name_of(id);
    if trace_ensure() {
        eprintln!(
            "module ensure init: id={id} name={name} init_ptr=0x{init_ptr:x} \
             pending_before={}",
            exception_pending(_py)
        );
    }
    let unwind = |_py: &PyToken<'_>| {
        let previous = table.slots[idx].swap(0, Ordering::AcqRel);
        if previous != 0 {
            dec_ref_bits(_py, previous);
        }
        table.owners[idx].store(0, Ordering::Release);
        // R2.4: only step back to Uninit if no view transition raced us.
        let _ = table.states[idx].compare_exchange(
            STATE_INITIALIZING,
            STATE_UNINIT,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    };

    // An exception already in flight must propagate, never leak into a module
    // body: init bodies are check_exception-routed and entering them with a
    // pending flag corrupts publication instead of executing the body (the
    // legacy dispatch chain cleared the flag per arm; ensure propagates —
    // callers own the check between statements).
    if exception_pending(_py) {
        unwind(_py);
        return none_bits();
    }

    let row = registry.row(id);
    if row.module_flags & MODULE_FLAG_HAS_BODY == 0 {
        // Fail closed, exact CPython message so the importlib fallback ladder
        // (spec/runtime-roots imports) keeps its dynamic-path semantics; the
        // admission channel is the runtime import dispatch set (invariant I11).
        unwind(_py);
        return raise_exception::<_>(
            _py,
            "ModuleNotFoundError",
            &format!("No module named '{name}'"),
        );
    }

    // MODULE_INIT_TABLE dispatch: the init body allocates its module and
    // publishes it via MODULE_CACHE_SET (mirrored into slots[id] by
    // publish_from_cache_set — publish-before-exec, invariant I6), then
    // executes the module body.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(init_target) = crate::provenance::abi::function_ptr(init_ptr) else {
            unwind(_py);
            return raise_exception::<_>(
                _py,
                "ImportError",
                &format!("module '{name}' initializer exceeds the active address space"),
            );
        };
        if init_target.is_null() {
            unwind(_py);
            return raise_exception::<_>(
                _py,
                "ImportError",
                &format!("module '{name}' has a null initializer"),
            );
        }
        let init: unsafe extern "C" fn() -> u64 = unsafe { std::mem::transmute(init_target) };
        let _ = unsafe { init() };
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = unsafe { molt_isolate_import(u64::from(id)) };
    }

    if exception_pending(_py) {
        unwind(_py);
        return none_bits();
    }

    let mut bits = table.slots[idx].load(Ordering::Acquire);
    if bits == 0 {
        // The body did not publish through the runtime store (possible for
        // adopted host modules); bridge from the legacy store before failing.
        if let Some(cache_bits) = legacy_cache_lookup(_py, name) {
            inc_ref_bits(_py, cache_bits);
            table.slots[idx].store(cache_bits, Ordering::Release);
            bits = cache_bits;
        } else {
            unwind(_py);
            return raise_exception::<_>(
                _py,
                "ImportError",
                &format!("module '{name}' init completed without publishing a module object"),
            );
        }
    }

    table.owners[idx].store(0, Ordering::Release);
    // R2.4: if a view transition (Tombstone/Replaced) raced the finishing
    // owner, do not republish — callers holding the partial reference keep a
    // coherent module, matching CPython.
    let _ = table.states[idx].compare_exchange(
        STATE_INITIALIZING,
        STATE_READY,
        Ordering::AcqRel,
        Ordering::Acquire,
    );

    // Publication callbacks may remove the table/sys.modules references. The
    // caller's owner must exist before entering them, not be minted afterward.
    inc_ref_bits(_py, bits);
    if let Err(error) = bind_parent_attr(_py, registry, table, id, bits) {
        dec_ref_bits(_py, bits);
        return error;
    }
    bits
}

/// Fresh-load completion owns the one observable parent-attribute publication.
fn bind_parent_attr(
    _py: &PyToken<'_>,
    registry: &'static ModuleRegistry,
    table: &'static ModuleTable,
    id: u32,
    bits: u64,
) -> Result<(), u64> {
    let row = registry.row(id);
    let Some(parent) = row.parent else {
        return Ok(());
    };
    let parent_bits = table.slots[parent as usize].load(Ordering::Acquire);
    if parent_bits == 0 || is_none_bits(parent_bits) {
        return Ok(());
    }
    let name = registry.name_of(id);
    let leaf = name.rsplit('.').next().unwrap_or(name);
    crate::builtins::modules::publish_import_child(
        _py,
        parent_bits,
        registry.name_of(parent),
        leaf,
        bits,
    )
}

/// Alias co-publication inside the target's resolution (design §4.3): the
/// alias row becomes Ready holding the target's module, and the legacy store
/// learns the alias name for PR1 coherence.
fn publish_alias(_py: &PyToken<'_>, registry: &'static ModuleRegistry, id: u32, bits: u64) {
    let Some(table) = module_table(_py) else {
        return;
    };
    let idx = id as usize;
    if matches!(
        table.states[idx].load(Ordering::Acquire),
        STATE_READY | STATE_RETIRED
    ) {
        return;
    }
    inc_ref_bits(_py, bits);
    let previous = table.slots[idx].swap(bits, Ordering::AcqRel);
    if previous != 0 {
        dec_ref_bits(_py, previous);
    }
    table.states[idx].store(STATE_READY, Ordering::Release);
    legacy_cache_set(_py, registry.name_of(id), bits);
}

/// Wait for another thread's in-flight init.  Returns `true` when a
/// cross-thread import cycle is detected (caller accepts the partial module,
/// CPython `_lock_unlock_module` semantics); `false` means "state may have
/// changed, re-examine".
fn wait_for_foreign_init(
    _py: &PyToken<'_>,
    table: &'static ModuleTable,
    id: u32,
    self_tid: u64,
) -> bool {
    // Record the wait-for edge, then walk owner → blocking_on → owner … to
    // find a cycle back to this thread (the `_blocking_on` wait graph).
    {
        let mut blocking = table.blocking_on.lock().unwrap();
        blocking.insert(self_tid, id);
        let mut hops = 0usize;
        let mut current_owner = table.owners[id as usize].load(Ordering::Acquire);
        while current_owner != 0 && hops < 1024 {
            if current_owner == self_tid {
                blocking.remove(&self_tid);
                return true;
            }
            let Some(&next_module) = blocking.get(&current_owner) else {
                break;
            };
            current_owner = table.owners[next_module as usize].load(Ordering::Acquire);
            hops += 1;
        }
    }
    // Park briefly with the GIL released so the owner can finish; the state
    // machine loop re-examines on wake.
    {
        let _release = crate::concurrency::GilReleaseGuard::suspend();
        std::thread::sleep(std::time::Duration::from_micros(100));
    }
    table.blocking_on.lock().unwrap().remove(&self_tid);
    false
}

// ─── Dynamic dispatch entry (name resolution is cold; both targets execute
//     through the ModuleId table) ───────────────────────────────────────────

/// Isolate callers share the same typed resolution and cache projection as
/// public dynamic imports. This ABI retains its non-raising miss sentinel.
pub(crate) fn isolate_import_dispatch(py: &PyToken<'_>, name: &str) -> u64 {
    let Some(key) = crate::attr_name_bits_from_bytes(py, name.as_bytes()) else {
        return none_bits();
    };
    let result = crate::builtins::modules::module_import_attempt(key);
    dec_ref_bits(py, key);
    match result {
        Ok(crate::builtins::modules::ModuleImportOutcome::Imported(bits)) => bits,
        Ok(crate::builtins::modules::ModuleImportOutcome::Missing { .. }) => none_bits(),
        Err(error) => error,
    }
}

// ─── Gate G4: the ensure state machine, driven transition by transition ─────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64 as TestCounter;

    /// One synthetic registry row, in the Python layout writer's column order.
    type BlobRow = (String, u64, Option<u32>, Option<u32>, u8, u8);

    /// Mirror of the Python layout authority (`molt.cli.module_registry`),
    /// used to build a synthetic registry blob for the state-machine tests.
    /// Divergence from the real writer fails `ModuleRegistry::parse`.
    struct BlobBuilder {
        rows: Vec<BlobRow>,
    }

    impl BlobBuilder {
        fn new() -> Self {
            Self { rows: Vec::new() }
        }

        fn row(
            &mut self,
            name: &str,
            init_ptr: u64,
            parent: Option<u32>,
            alias_target: Option<u32>,
            kind: u8,
            flags: u8,
        ) -> &mut Self {
            self.rows.push((
                name.to_string(),
                init_ptr,
                parent,
                alias_target,
                kind,
                flags
                    | if init_ptr == 0 {
                        0
                    } else {
                        MODULE_FLAG_HAS_BODY
                    },
            ));
            self
        }

        fn build(&self) -> Vec<u8> {
            let mut sorted = self.rows.clone();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            assert_eq!(
                sorted, self.rows,
                "test rows must be pre-sorted so ids match declaration order"
            );
            let mut names = Vec::new();
            let mut spans = Vec::new();
            for (name, ..) in &self.rows {
                spans.push((names.len() as u32, name.len() as u32));
                names.extend_from_slice(name.as_bytes());
            }
            let mut blob = Vec::new();
            blob.extend_from_slice(&MODULE_REGISTRY_MAGIC.to_le_bytes());
            blob.extend_from_slice(&MODULE_REGISTRY_SCHEMA_VERSION.to_le_bytes());
            blob.extend_from_slice(&(self.rows.len() as u32).to_le_bytes());
            blob.extend_from_slice(&[0x47u8; 16]); // digest: synthetic
            blob.extend_from_slice(&(names.len() as u64).to_le_bytes());
            blob.extend_from_slice(&0u64.to_le_bytes());
            assert_eq!(blob.len(), MODULE_REGISTRY_HEADER_BYTES);
            for ((_, init_ptr, parent, alias, kind, flags), (off, len)) in
                self.rows.iter().zip(&spans)
            {
                blob.extend_from_slice(&off.to_le_bytes());
                blob.extend_from_slice(&len.to_le_bytes());
                blob.extend_from_slice(&init_ptr.to_le_bytes());
                blob.extend_from_slice(&parent.unwrap_or(NO_MODULE_ID).to_le_bytes());
                blob.extend_from_slice(&alias.unwrap_or(NO_MODULE_ID).to_le_bytes());
                blob.push(*kind);
                blob.push(*flags);
                blob.extend_from_slice(&0u16.to_le_bytes());
                blob.extend_from_slice(&0u32.to_le_bytes());
                blob.extend_from_slice(&0u32.to_le_bytes());
                blob.extend_from_slice(&0u32.to_le_bytes());
            }
            blob.extend_from_slice(&names);
            blob
        }
    }

    static PARENT_CHILD_OVERRIDE: TestCounter = TestCounter::new(0);
    static INIT_RUNS: TestCounter = TestCounter::new(0);
    static SYS_INIT_RUNS: TestCounter = TestCounter::new(0);
    static CYCLE_OBSERVED_PARTIAL: TestCounter = TestCounter::new(0);
    static EXT_FAIL_RUNS: TestCounter = TestCounter::new(0);

    fn publish_test_module(name: &str) -> u64 {
        crate::with_gil_entry_nopanic!(_py, {
            let name_ptr = alloc_string(_py, name.as_bytes());
            assert!(!name_ptr.is_null());
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let module_bits = crate::builtins::modules::molt_module_new(name_bits);
            let set_bits = if name == "sys" {
                publish_interpreter_sys_for_test(_py, module_bits)
            } else {
                crate::builtins::modules::molt_module_cache_set(name_bits, module_bits)
            };
            if !is_none_bits(set_bits) {
                dec_ref_bits(_py, set_bits);
            }
            dec_ref_bits(_py, name_bits);
            module_bits
        })
    }

    // Model a linked builtin initializer, including explicit loader reimports.
    // Publication alone is insufficient: a loader can remove its public cache
    // entry, after which the actual registry initializer owns reconstruction.
    extern "C" fn init_test_sys() -> u64 {
        SYS_INIT_RUNS.fetch_add(1, Ordering::SeqCst);
        crate::with_gil_entry_nopanic!(py, {
            let name = crate::attr_name_bits_from_bytes(py, b"sys").unwrap();
            let module = if let Some(module) = crate::builtins::modules::interpreter_sys_module(py)
            {
                inc_ref_bits(py, module);
                module
            } else {
                crate::builtins::modules::molt_module_new(name)
            };
            let result = crate::builtins::modules::molt_module_cache_set(name, module);
            dec_ref_bits(py, module);
            dec_ref_bits(py, name);
            result
        })
    }

    extern "C" fn init_g4_src() -> u64 {
        INIT_RUNS.fetch_add(1, Ordering::SeqCst);
        let bits = publish_test_module("g4_src");
        crate::with_gil_entry_nopanic!(_py, {
            dec_ref_bits(_py, bits);
        });
        0
    }

    extern "C" fn init_g4_fail() -> u64 {
        crate::with_gil_entry_nopanic!(_py, {
            raise_exception::<u64>(_py, "ValueError", "g4 init failure")
        })
    }

    extern "C" fn init_g4_z_static_ext_fail() -> u64 {
        EXT_FAIL_RUNS.fetch_add(1, Ordering::SeqCst);
        crate::with_gil_entry_nopanic!(_py, {
            raise_exception::<u64>(
                _py,
                "ImportError",
                "g4_z_static_ext_fail: static-link PyModuleDef Py_mod_exec slot \
                 returned non-zero",
            )
        })
    }

    extern "C" fn init_g4_pkg() -> u64 {
        let bits = publish_test_module("g4_pkg");
        crate::with_gil_entry_nopanic!(_py, {
            let replacement = PARENT_CHILD_OVERRIDE.load(Ordering::Relaxed);
            if replacement != 0 {
                let sys = legacy_cache_lookup(_py, "sys").unwrap();
                let modules_bits =
                    crate::builtins::modules::sys_modules_dict_bits(_py, sys).unwrap();
                let modules = obj_from_bits(modules_bits).as_ptr().unwrap();
                let _modules_owner = crate::PtrDropGuard::new(modules);
                let child = crate::attr_name_bits_from_bytes(_py, b"g4_pkg.sub").unwrap();
                unsafe { crate::dict_set_in_place(_py, modules, child, replacement) };
                dec_ref_bits(_py, child);
            }
            dec_ref_bits(_py, bits);
        });
        0
    }

    extern "C" fn init_g4_pkg_sub() -> u64 {
        crate::with_gil_entry_nopanic!(_py, {
            // Parent-first must have completed g4_pkg before this body runs.
            assert!(legacy_cache_lookup(_py, "g4_pkg").is_some());
        });
        let bits = publish_test_module("g4_pkg.sub");
        crate::with_gil_entry_nopanic!(_py, {
            dec_ref_bits(_py, bits);
        });
        0
    }

    extern "C" fn init_g4_target() -> u64 {
        let bits = publish_test_module("g4_target");
        crate::with_gil_entry_nopanic!(_py, {
            dec_ref_bits(_py, bits);
        });
        0
    }

    extern "C" fn init_g4_tomb() -> u64 {
        let bits = publish_test_module("g4_tomb");
        crate::with_gil_entry_nopanic!(_py, {
            dec_ref_bits(_py, bits);
        });
        0
    }

    extern "C" fn init_g4_tomb_ext() -> u64 {
        let bits = publish_test_module("g4_tomb_ext");
        crate::with_gil_entry_nopanic!(_py, {
            dec_ref_bits(_py, bits);
        });
        0
    }

    extern "C" fn init_g4_cycle_a() -> u64 {
        let bits = publish_test_module("g4_cycle_a");
        crate::with_gil_entry_nopanic!(_py, {
            dec_ref_bits(_py, bits);
            // Body imports g4_cycle_b, whose body re-imports g4_cycle_a.
            let b = module_ensure(_py, test_registry_id("g4_cycle_b"));
            assert!(!exception_pending(_py));
            dec_ref_bits(_py, b);
        });
        0
    }

    extern "C" fn init_g4_cycle_b() -> u64 {
        let bits = publish_test_module("g4_cycle_b");
        crate::with_gil_entry_nopanic!(_py, {
            dec_ref_bits(_py, bits);
            // Same-thread circular import: must observe the PARTIAL g4_cycle_a
            // (publish-before-exec, invariant I6) without re-entering its init.
            let a = module_ensure(_py, test_registry_id("g4_cycle_a"));
            assert!(!exception_pending(_py));
            assert!(!is_none_bits(a));
            CYCLE_OBSERVED_PARTIAL.fetch_add(1, Ordering::SeqCst);
            dec_ref_bits(_py, a);
        });
        0
    }

    fn test_registry_id(name: &str) -> u32 {
        module_registry()
            .expect("test registry installed")
            .id_of(name)
            .unwrap_or_else(|| panic!("test registry misses {name}"))
    }

    // Test scopes borrow stable rows, retaining their displaced slot through
    // the production execution snapshot. Synthetic initializer owners must not
    // survive an assertion unwind and strand another test in the import wait.
    struct ModuleTestRestore<'a, 'py> {
        py: &'a PyToken<'py>,
        snapshot: Option<ModuleExecutionSnapshot>,
        cache: Option<(u64, Option<u64>)>,
    }

    impl<'a, 'py> ModuleTestRestore<'a, 'py> {
        fn capture(py: &'a PyToken<'py>, name: &str) -> Self {
            let id = test_registry_id(name);
            let table = module_table(py).expect("test module table");
            let index = id as usize;
            assert_eq!(table.owners[index].load(Ordering::Acquire), 0);
            let state = table.states[index].load(Ordering::Acquire);
            assert!(!matches!(
                state,
                STATE_INITIALIZING | STATE_EXECUTION_RESERVED
            ));
            let name_bits = crate::attr_name_bits_from_bytes(py, name.as_bytes()).unwrap();
            let previous = legacy_cache_lookup(py, name);
            if let Some(bits) = previous {
                inc_ref_bits(py, bits);
            }
            let bits = table.slots[index].load(Ordering::Acquire);
            if bits != 0 {
                inc_ref_bits(py, bits);
            }
            Self {
                py,
                snapshot: Some(ModuleExecutionSnapshot { id, state, bits }),
                cache: Some((name_bits, previous)),
            }
        }

        fn execution(py: &'a PyToken<'py>, snapshot: ModuleExecutionSnapshot) -> Self {
            Self {
                py,
                snapshot: Some(snapshot),
                cache: None,
            }
        }

        fn restore(mut self) {
            self.restore_inner();
        }

        fn restore_inner(&mut self) {
            let Some(snapshot) = self.snapshot.take() else {
                return;
            };
            molt_cpython_abi::api::errors::with_preserved_error(|| {
                if let Some((name, previous)) = self.cache.take() {
                    // Revoke synthetic initializer custody before cache teardown
                    // or publication can run callbacks or bootstrap a namespace.
                    // Keep the original snapshot owner through publication: the
                    // prior cache and prior table slot may be different objects.
                    if snapshot.bits != 0 {
                        inc_ref_bits(self.py, snapshot.bits);
                    }
                    restore_module_execution(
                        self.py,
                        Some(ModuleExecutionSnapshot {
                            id: snapshot.id,
                            state: snapshot.state,
                            bits: snapshot.bits,
                        }),
                    );
                    crate::builtins::modules::module_cache_remove(name, None);
                    if let Some(bits) = previous {
                        crate::builtins::modules::module_cache_publish(
                            name,
                            bits,
                            crate::builtins::modules::ModuleCachePublication::Extension,
                        );
                        dec_ref_bits(self.py, bits);
                    }
                    dec_ref_bits(self.py, name);
                }
                // Extension publication projects the cache into the table.
                // Restore the exact prior slot/state, consuming its saved owner.
                restore_module_execution(self.py, Some(snapshot));
            });
        }
    }

    impl Drop for ModuleTestRestore<'_, '_> {
        fn drop(&mut self) {
            self.restore_inner();
        }
    }

    extern "C" fn init_test_io() -> u64 {
        let module = publish_test_module("_io");
        crate::with_gil_entry_nopanic!(py, {
            dec_ref_bits(py, module);
        });
        0
    }

    /// Install the synthetic registry exactly once per test process.  Rows
    /// are pre-sorted; ids are their positions.
    pub(super) fn install_test_registry() {
        static INSTALL: std::sync::Once = std::sync::Once::new();
        INSTALL.call_once(|| {
            // Ids are declaration positions (rows pre-sorted; the builder
            // asserts the order): 0 _io, 1 builtins, 2 g4_alias, 3 g4_cycle_a,
            // 4 g4_cycle_b, 5 g4_fail, 6 g4_noinit, 7 g4_pkg,
            // 8 g4_pkg.sub, 9 g4_src, 10 g4_target, 11 g4_tomb,
            // 12 g4_tomb_ext, 13 g4_z_static_ext_fail, 14 sys.
            let mut builder = BlobBuilder::new();
            builder
                .row(
                    "_io",
                    init_test_io as *const () as usize as u64,
                    None,
                    None,
                    MODULE_KIND_RUNTIME_BUILTIN,
                    0,
                )
                .row("builtins", 0, None, None, MODULE_KIND_RUNTIME_BUILTIN, 0)
                .row("g4_alias", 0, None, Some(10), MODULE_KIND_ALIAS, 0)
                .row(
                    "g4_cycle_a",
                    init_g4_cycle_a as *const () as usize as u64,
                    None,
                    None,
                    MODULE_KIND_SOURCE,
                    0,
                )
                .row(
                    "g4_cycle_b",
                    init_g4_cycle_b as *const () as usize as u64,
                    None,
                    None,
                    MODULE_KIND_SOURCE,
                    0,
                )
                .row(
                    "g4_fail",
                    init_g4_fail as *const () as usize as u64,
                    None,
                    None,
                    MODULE_KIND_SOURCE,
                    0,
                )
                .row("g4_noinit", 0, None, None, MODULE_KIND_SOURCE, 0)
                .row(
                    "g4_pkg",
                    init_g4_pkg as *const () as usize as u64,
                    None,
                    None,
                    MODULE_KIND_SOURCE,
                    0,
                )
                .row(
                    "g4_pkg.sub",
                    init_g4_pkg_sub as *const () as usize as u64,
                    Some(7),
                    None,
                    MODULE_KIND_SOURCE,
                    0,
                )
                .row(
                    "g4_src",
                    init_g4_src as *const () as usize as u64,
                    None,
                    None,
                    MODULE_KIND_SOURCE,
                    0,
                )
                .row(
                    "g4_target",
                    init_g4_target as *const () as usize as u64,
                    None,
                    None,
                    MODULE_KIND_SOURCE,
                    0,
                )
                .row(
                    "g4_tomb",
                    init_g4_tomb as *const () as usize as u64,
                    None,
                    None,
                    MODULE_KIND_SOURCE,
                    0,
                )
                .row(
                    "g4_tomb_ext",
                    init_g4_tomb_ext as *const () as usize as u64,
                    None,
                    None,
                    MODULE_KIND_EXTENSION,
                    MODULE_FLAG_REINIT_RESURRECT,
                )
                .row(
                    "g4_z_static_ext_fail",
                    init_g4_z_static_ext_fail as *const () as usize as u64,
                    None,
                    None,
                    MODULE_KIND_EXTENSION,
                    0,
                )
                .row(
                    "sys",
                    init_test_sys as *const () as usize as u64,
                    None,
                    None,
                    MODULE_KIND_RUNTIME_BUILTIN,
                    0,
                );
            let blob: &'static [u8] = Box::leak(builder.build().into_boxed_slice());
            assert_eq!(molt_module_registry_install(blob.as_ptr()), 0);
        });
    }

    #[test]
    fn synthetic_initializer_scope_restores_cache_and_row_after_unwind() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        install_test_registry();
        crate::with_gil_entry_nopanic!(py, {
            let _outer = ModuleTestRestore::capture(py, "g4_src");
            let module = publish_test_module("g4_src");
            let _module_owner = crate::PtrDropGuard::new(obj_from_bits(module).as_ptr().unwrap());
            let index = test_registry_id("g4_src") as usize;
            let table = module_table(py).unwrap();
            let state = table.states[index].load(Ordering::Acquire);
            let bits = table.slots[index].load(Ordering::Acquire);
            let owner = table.owners[index].load(Ordering::Acquire);
            let cached = legacy_cache_lookup(py, "g4_src");
            let outcome = crate::test_support::catch_expected_unwind(|| {
                let _restore = ModuleTestRestore::capture(py, "g4_src");
                table.states[index].store(STATE_INITIALIZING, Ordering::Release);
                table.owners[index].store(
                    crate::concurrency::current_thread_id().wrapping_add(1),
                    Ordering::Release,
                );
                panic!("synthetic initializer failure");
            });
            assert!(outcome.is_err());
            assert_eq!(table.states[index].load(Ordering::Acquire), state);
            assert_eq!(table.slots[index].load(Ordering::Acquire), bits);
            assert_eq!(table.owners[index].load(Ordering::Acquire), owner);
            assert_eq!(legacy_cache_lookup(py, "g4_src"), cached);
            assert!(!exception_pending(py));
        });
    }

    #[test]
    fn synthetic_builtin_scope_preserves_distinct_cache_and_slot_after_unwind() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        install_test_registry();
        crate::with_gil_entry_nopanic!(py, {
            let _io_restore = ModuleTestRestore::capture(py, "_io");
            let _outer = ModuleTestRestore::capture(py, "builtins");
            let name = crate::attr_name_bits_from_bytes(py, b"builtins").unwrap();
            let _name_owner = crate::PtrDropGuard::new(obj_from_bits(name).as_ptr().unwrap());
            let key = crate::attr_name_bits_from_bytes(py, b"len").unwrap();
            let _key_owner = crate::PtrDropGuard::new(obj_from_bits(key).as_ptr().unwrap());
            let prior_cache = crate::molt_module_new(name);
            let cache_ptr = obj_from_bits(prior_cache).as_ptr().unwrap();
            let _cache_owner = crate::PtrDropGuard::new(cache_ptr);
            let prior_slot = crate::molt_module_new(name);
            let _slot_owner = crate::PtrDropGuard::new(obj_from_bits(prior_slot).as_ptr().unwrap());
            assert_ne!(prior_cache, prior_slot);
            let dict = obj_from_bits(unsafe { crate::module_dict_bits(cache_ptr) })
                .as_ptr()
                .unwrap();
            let user_value = MoltObject::from_int(42).bits();
            unsafe { crate::dict_set_in_place(py, dict, key, user_value) };
            crate::builtins::modules::module_cache_publish(
                name,
                prior_cache,
                crate::builtins::modules::ModuleCachePublication::Extension,
            );
            let id = test_registry_id("builtins");
            module_table_view_replace(py, id, prior_slot);
            assert!(!exception_pending(py));
            let index = id as usize;
            let table = module_table(py).unwrap();
            assert_eq!(table.states[index].load(Ordering::Acquire), STATE_REPLACED);
            assert_eq!(table.owners[index].load(Ordering::Acquire), 0);
            assert_eq!(legacy_cache_lookup(py, "builtins"), Some(prior_cache));
            assert_eq!(
                unsafe { crate::dict_get_in_place(py, dict, key) },
                Some(user_value)
            );

            let outcome = crate::test_support::catch_expected_unwind(|| {
                let _restore = ModuleTestRestore::capture(py, "builtins");
                let displaced = table.slots[index].swap(0, Ordering::AcqRel);
                table.owners[index]
                    .store(crate::concurrency::current_thread_id(), Ordering::Release);
                table.states[index].store(STATE_INITIALIZING, Ordering::Release);
                if displaced != 0 {
                    dec_ref_bits(py, displaced);
                }
                assert!(module_initialization_awaits_publication(py, "builtins"));
                panic!("same-thread builtin initializer failure");
            });
            let payload = outcome.expect_err("the synthetic initializer must unwind");
            assert_eq!(
                payload.downcast_ref::<&str>().copied(),
                Some("same-thread builtin initializer failure"),
                "a setup assertion must not satisfy the unwind oracle"
            );
            assert!(!exception_pending(py));
            assert_eq!(table.states[index].load(Ordering::Acquire), STATE_REPLACED);
            assert_eq!(table.owners[index].load(Ordering::Acquire), 0);
            assert_eq!(table.slots[index].load(Ordering::Acquire), prior_slot);
            assert_eq!(legacy_cache_lookup(py, "builtins"), Some(prior_cache));
            assert_eq!(
                unsafe { crate::dict_get_in_place(py, dict, key) },
                Some(user_value),
                "restoring a retained namespace must preserve the user's builtin replacement"
            );
        });
    }

    #[test]
    fn interpreter_sys_publication_requires_initializer_and_rejects_dictionary_payload() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        install_test_registry();
        crate::with_gil_entry_nopanic!(py, {
            let name = crate::attr_name_bits_from_bytes(py, b"sys").unwrap();
            legacy_cache_del(py, "sys");
            let module = crate::molt_module_new(name);
            let unrelated = crate::molt_module_new(name);
            crate::molt_module_cache_set(name, unrelated);
            assert!(runtime_state(py).interpreter_sys.module(py).is_none());
            let table = module_table(py).unwrap();
            let index = module_id_of("sys").unwrap() as usize;
            table.states[index].store(STATE_INITIALIZING, Ordering::Release);
            table.owners[index].store(crate::concurrency::current_thread_id(), Ordering::Release);
            let dictionary = MoltObject::from_ptr(crate::alloc_dict_with_pairs(py, &[])).bits();
            crate::molt_module_cache_set(name, dictionary);
            assert!(exception_pending(py));
            assert!(runtime_state(py).interpreter_sys.module(py).is_none());
            assert_eq!(table.slots[index].load(Ordering::Acquire), 0);
            assert_eq!(legacy_cache_lookup(py, "sys"), Some(unrelated));
            crate::clear_exception(py);
            crate::molt_module_cache_set(name, module);
            assert!(!exception_pending(py));
            assert_eq!(runtime_state(py).interpreter_sys.module(py), Some(module));
            assert_eq!(
                legacy_cache_lookup(py, "sys"),
                Some(module),
                "initializer replaces the unrelated prepublication"
            );
            legacy_cache_del(py, "sys");
            assert_eq!(runtime_state(py).interpreter_sys.module(py), Some(module));
            for bits in [dictionary, unrelated, module, name] {
                dec_ref_bits(py, bits);
            }
        });
    }

    #[test]
    fn interpreter_sys_republication_and_version_setup_preserve_user_namespace() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        install_test_registry();
        crate::with_gil_entry_nopanic!(py, {
            let name = crate::attr_name_bits_from_bytes(py, b"sys").unwrap();
            legacy_cache_del(py, "sys");
            let id = test_registry_id("sys");
            let sys = module_ensure(py, id);
            assert!(!exception_pending(py));
            assert_eq!(runtime_state(py).interpreter_sys.module(py), Some(sys));
            let initialized = SYS_INIT_RUNS.load(Ordering::SeqCst);
            let sys_ptr = obj_from_bits(sys).as_ptr().unwrap();
            let dict = obj_from_bits(unsafe { crate::module_dict_bits(sys_ptr) })
                .as_ptr()
                .unwrap();
            let version_key = crate::attr_name_bits_from_bytes(py, b"version_info").unwrap();
            let implementation_key =
                crate::attr_name_bits_from_bytes(py, b"implementation").unwrap();
            let path_key = crate::attr_name_bits_from_bytes(py, b"path").unwrap();
            let stdout_key = crate::attr_name_bits_from_bytes(py, b"stdout").unwrap();
            let marker =
                MoltObject::from_ptr(crate::alloc_tuple(py, &[MoltObject::from_int(82).bits()]))
                    .bits();
            unsafe {
                assert!(crate::dict_get_in_place(py, dict, version_key).is_some());
                crate::dict_del_in_place(py, dict, version_key);
                crate::dict_del_in_place(py, dict, stdout_key);
                crate::dict_set_in_place(py, dict, implementation_key, marker);
                crate::dict_set_in_place(py, dict, path_key, marker);
            }
            assert!(!exception_pending(py));

            crate::molt_module_cache_set(name, sys);
            assert!(!exception_pending(py));
            let table = module_table(py).unwrap();
            assert_eq!(
                table.states[id as usize].load(Ordering::Acquire),
                STATE_READY
            );
            assert_eq!(
                detach_cache_publication(py, "sys", Some(marker)),
                0,
                "a different owner's removal cannot detach the ready sys namespace"
            );
            assert_eq!(table.slots[id as usize].load(Ordering::Acquire), sys);
            crate::builtins::modules::module_cache_remove(name, Some(sys));
            assert!(!exception_pending(py));
            assert_eq!(
                module_table(py).unwrap().states[id as usize].load(Ordering::Acquire),
                STATE_TOMBSTONE
            );
            assert_eq!(table.slots[id as usize].load(Ordering::Acquire), 0);
            assert!(legacy_cache_lookup(py, "sys").is_none());
            let reimported = module_ensure(py, id);
            assert!(!exception_pending(py));
            assert_eq!(
                SYS_INIT_RUNS.load(Ordering::SeqCst),
                initialized + 1,
                "reimport must run the registered initializer through module_ensure"
            );
            assert_eq!(
                table.states[id as usize].load(Ordering::Acquire),
                STATE_READY
            );
            assert_eq!(
                reimported, sys,
                "test initializer republishes its retained builtin namespace"
            );

            // Host entry setup can repeat for the same target, but must never
            // replace finalized Python shapes or recreate deleted public keys.
            let (info, _) = crate::object::ops_sys::current_sys_version_info(runtime_state(py));
            let release =
                MoltObject::from_ptr(alloc_string(py, info.releaselevel.as_bytes())).bits();
            let version = crate::molt_sys_version();
            crate::molt_sys_set_version_info(
                MoltObject::from_int(info.major).bits(),
                MoltObject::from_int(info.minor).bits(),
                MoltObject::from_int(info.micro).bits(),
                release,
                MoltObject::from_int(info.serial).bits(),
                version,
            );
            assert!(!exception_pending(py));
            unsafe {
                assert!(crate::dict_get_in_place(py, dict, version_key).is_none());
                assert!(crate::dict_get_in_place(py, dict, stdout_key).is_none());
                assert_eq!(
                    crate::dict_get_in_place(py, dict, implementation_key),
                    Some(marker)
                );
                assert_eq!(crate::dict_get_in_place(py, dict, path_key), Some(marker));
            }
            for bits in [
                version,
                release,
                marker,
                version_key,
                implementation_key,
                path_key,
                stdout_key,
                reimported,
                sys,
                name,
            ] {
                dec_ref_bits(py, bits);
            }
        });
    }

    #[test]
    fn extension_publication_retires_ready_owner_and_preserves_foreign_custody() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        install_test_registry();
        crate::with_gil_entry_nopanic!(py, {
            let _g4_src_restore = ModuleTestRestore::capture(py, "g4_src");
            legacy_cache_del(py, "sys");
            let sys = publish_test_module("sys");
            let id = test_registry_id("g4_src");
            let table = module_table(py).unwrap();
            let idx = id as usize;
            let old = module_ensure(py, id);
            assert!(!exception_pending(py));
            assert_eq!(table.states[idx].load(Ordering::Acquire), STATE_READY);
            let name = crate::attr_name_bits_from_bytes(py, b"g4_src").unwrap();
            let new = crate::builtins::modules::molt_module_new(name);
            crate::builtins::modules::module_cache_publish(
                name,
                new,
                crate::builtins::modules::ModuleCachePublication::Extension,
            );
            assert!(!exception_pending(py));
            assert_eq!(table.slots[idx].load(Ordering::Acquire), new);
            assert_eq!(table.states[idx].load(Ordering::Acquire), STATE_REPLACED);
            let old_ptr = obj_from_bits(old).as_ptr().unwrap();
            assert_eq!(
                unsafe { (*crate::object::header_from_obj_ptr(old_ptr)).ref_count_snapshot() },
                1,
                "public/private/table replacement must leave only the caller owner"
            );
            crate::builtins::modules::module_cache_remove(name, Some(new));
            assert_eq!(table.slots[idx].load(Ordering::Acquire), 0);
            assert_eq!(table.states[idx].load(Ordering::Acquire), STATE_TOMBSTONE);

            inc_ref_bits(py, old);
            table.slots[idx].store(old, Ordering::Release);
            table.states[idx].store(STATE_INITIALIZING, Ordering::Release);
            table.owners[idx].store(
                crate::concurrency::current_thread_id().wrapping_add(1),
                Ordering::Release,
            );
            assert_eq!(publish_extension_result(py, "g4_src", new), 0);
            assert_eq!(table.slots[idx].load(Ordering::Acquire), old);
            assert_eq!(
                table.states[idx].load(Ordering::Acquire),
                STATE_INITIALIZING
            );
            table.owners[idx].store(crate::concurrency::current_thread_id(), Ordering::Release);
            let retired = publish_extension_result(py, "g4_src", new);
            assert_eq!(retired, old);
            assert_eq!(table.slots[idx].load(Ordering::Acquire), new);
            assert_eq!(
                table.states[idx].load(Ordering::Acquire),
                STATE_INITIALIZING
            );
            dec_ref_bits(py, retired);
            let retired = detach_cache_publication(py, "g4_src", Some(new));
            dec_ref_bits(py, retired);
            table.owners[idx].store(0, Ordering::Release);
            table.states[idx].store(STATE_UNINIT, Ordering::Release);
            for bits in [old, new, name, sys] {
                dec_ref_bits(py, bits);
            }
        });
    }
    #[test]
    fn import_outcome_registered_cache_refresh_preserves_initializer_custody() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        install_test_registry();
        crate::with_gil_entry_nopanic!(py, {
            let _g4_src_restore = ModuleTestRestore::capture(py, "g4_src");
            let previous_sys = legacy_cache_lookup(py, "sys");
            if let Some(bits) = previous_sys {
                inc_ref_bits(py, bits);
            }
            legacy_cache_del(py, "sys");
            let sys = publish_test_module("sys");
            let modules_bits = crate::builtins::modules::sys_modules_dict_bits(py, sys).unwrap();
            let modules = obj_from_bits(modules_bits).as_ptr().unwrap();
            let _modules_owner = crate::PtrDropGuard::new(modules);
            let name = crate::attr_name_bits_from_bytes(py, b"g4_src").unwrap();
            let id = test_registry_id("g4_src");
            unsafe { crate::object::ops::dict_del_in_place(py, modules, name) };
            module_table_view_tombstone(py, id);
            let table = module_table(py).unwrap();
            table.states[id as usize].store(STATE_UNINIT, Ordering::Release);
            let previous = publish_test_module("g4_src");
            assert!(unsafe { crate::object::ops::dict_del_in_place(py, modules, name) });
            let runs = INIT_RUNS.load(Ordering::SeqCst);
            let current = module_ensure(py, id);
            assert!(!exception_pending(py));
            assert_ne!(
                current, previous,
                "a public miss must retire a stale private publication"
            );
            assert_eq!(INIT_RUNS.load(Ordering::SeqCst), runs + 1);

            // Same-thread self-import during initialization sees replacement
            // without taking the active initializer's transaction slot/state.
            table.states[id as usize].store(STATE_INITIALIZING, Ordering::Release);
            table.owners[id as usize]
                .store(crate::concurrency::current_thread_id(), Ordering::Release);
            let replacement = MoltObject::from_int(42).bits();
            unsafe { crate::dict_set_in_place(py, modules, name, replacement) };
            assert!(
                matches!(crate::builtins::modules::module_import_attempt(name),
                Ok(crate::builtins::modules::ModuleImportOutcome::Imported(bits)) if bits == replacement)
            );
            assert_eq!(table.slots[id as usize].load(Ordering::Acquire), current);
            assert_eq!(
                table.states[id as usize].load(Ordering::Acquire),
                STATE_INITIALIZING
            );
            table.owners[id as usize].store(0, Ordering::Release);
            table.states[id as usize].store(STATE_READY, Ordering::Release);
            unsafe { crate::dict_set_in_place(py, modules, name, current) };

            // Parent initialization may publish its child's public replacement
            // without invoking child ensure. The subsequent loop observes it.
            for text in ["g4_pkg.sub", "g4_pkg"] {
                let key = crate::attr_name_bits_from_bytes(py, text.as_bytes()).unwrap();
                unsafe { crate::object::ops::dict_del_in_place(py, modules, key) };
                module_table_view_tombstone(py, test_registry_id(text));
                dec_ref_bits(py, key);
            }
            PARENT_CHILD_OVERRIDE.store(replacement, Ordering::Relaxed);
            assert_eq!(
                module_ensure(py, test_registry_id("g4_pkg.sub")),
                replacement
            );
            PARENT_CHILD_OVERRIDE.store(0, Ordering::Relaxed);

            for bits in [previous, current, name] {
                dec_ref_bits(py, bits);
            }
            legacy_cache_del(py, "sys");
            if let Some(bits) = previous_sys {
                legacy_cache_set(py, "sys", bits);
                dec_ref_bits(py, bits);
            }
            dec_ref_bits(py, sys);
            assert!(!exception_pending(py));
        });
    }

    fn pending_exception_text(_py: &PyToken<'_>, expected_kind: &str) -> String {
        assert!(exception_pending(_py), "expected a pending exception");
        let exc_bits = crate::builtins::exceptions::molt_exception_last_pending();
        assert!(
            crate::builtins::exceptions::exception_matches_builtin_name(
                _py,
                exc_bits,
                expected_kind
            ),
            "expected {expected_kind} before clearing its owned exception"
        );
        let text = obj_from_bits(exc_bits)
            .as_ptr()
            .map(|ptr| crate::format_exception_with_traceback(_py, ptr))
            .unwrap_or_default();
        dec_ref_bits(_py, exc_bits);
        let _ = crate::molt_exception_clear();
        text
    }

    fn lookup_test_global(
        _py: &PyToken<'_>,
        module_bits: u64,
        name_bits: u64,
        through_frame: bool,
    ) -> u64 {
        if !through_frame {
            return crate::builtins::modules::molt_module_get_global(module_bits, name_bits);
        }
        let module_ptr = obj_from_bits(module_bits).as_ptr().expect("test module");
        let globals_bits = unsafe { crate::object::layout::module_dict_bits(module_ptr) };
        let builtins_bits =
            crate::builtins::frames::frame_effective_builtins_bits(_py, globals_bits);
        inc_ref_bits(_py, globals_bits);
        inc_ref_bits(_py, builtins_bits);
        crate::builtins::frames::frame_stack_push_owned(_py, 0, globals_bits, builtins_bits, 0);
        // A conflicting module argument proves the active-frame dictionary is
        // the authority for this sibling of the direct-module lookup path.
        let result = crate::builtins::modules::molt_module_get_global(none_bits(), name_bits);
        crate::builtins::frames::frame_stack_pop(_py);
        result
    }

    #[test]
    fn builtin_bootstrap_publication_precedes_recursive_python_and_preserves_deletion() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        install_test_registry();
        crate::with_gil_entry_nopanic!(py, {
            let _io_restore = ModuleTestRestore::capture(py, "_io");
            let _builtins_restore = ModuleTestRestore::capture(py, "builtins");
            legacy_cache_del(py, "builtins");
            let idx = test_registry_id("builtins") as usize;
            let table = module_table(py).expect("table");
            let name = MoltObject::from_ptr(alloc_string(py, b"builtins")).bits();
            let module = crate::molt_module_new(name);
            let ptr = obj_from_bits(module).as_ptr().expect("module");
            let dict = obj_from_bits(unsafe { crate::module_dict_bits(ptr) })
                .as_ptr()
                .unwrap();
            let property_name = MoltObject::from_ptr(alloc_string(py, b"property")).bits();
            assert!(
                unsafe { crate::dict_get_in_place(py, dict, property_name) }.is_none(),
                "a same-named standalone module must not gain bootstrap privilege"
            );
            assert!(!module_initialization_awaits_publication(py, "builtins"));
            table.states[idx].store(STATE_INITIALIZING, Ordering::Release);
            table.owners[idx].store(crate::concurrency::current_thread_id(), Ordering::Release);
            assert!(module_initialization_awaits_publication(py, "builtins"));
            let result = crate::molt_module_cache_set(name, module);
            assert!(
                !exception_pending(py),
                "builtins first publication: {}",
                crate::exception_last_bits_noinc(py)
                    .and_then(|bits| obj_from_bits(bits).as_ptr())
                    .map(|ptr| crate::format_exception_with_traceback(py, ptr))
                    .unwrap_or_default()
            );
            if !is_none_bits(result) {
                dec_ref_bits(py, result);
            }
            assert!(!module_initialization_awaits_publication(py, "builtins"));
            // Before a single Python statement has run, the recursive importer
            // sees every public runtime-backed class and admitted callable.
            for (name, expected) in crate::builtins::classes::public_builtin_classes(py) {
                let key = MoltObject::from_ptr(alloc_string(py, name.as_bytes())).bits();
                assert_eq!(
                    unsafe { crate::dict_get_in_place(py, dict, key) },
                    Some(expected),
                    "{name}"
                );
                dec_ref_bits(py, key);
            }
            let io = module_ensure(py, test_registry_id("_io"));
            assert!(!exception_pending(py));
            let io_ptr = obj_from_bits(io).as_ptr().expect("native provider module");
            let io_dict = obj_from_bits(unsafe { crate::module_dict_bits(io_ptr) })
                .as_ptr()
                .unwrap();
            let open_name = crate::attr_name_bits_from_bytes(py, b"open").unwrap();
            let provider_open = unsafe { crate::dict_get_in_place(py, io_dict, open_name) };
            assert!(provider_open.is_some(), "admitted provider publishes open");
            assert_eq!(
                unsafe { crate::dict_get_in_place(py, dict, open_name) },
                provider_open
            );
            dec_ref_bits(py, open_name);
            dec_ref_bits(py, io);
            let unrelated = crate::molt_module_new(name);
            for builtin in [
                "property",
                "globals",
                "locals",
                "__import__",
                "len",
                "ValueError",
            ] {
                let key = MoltObject::from_ptr(alloc_string(py, builtin.as_bytes())).bits();
                let value = crate::molt_module_get_global(unrelated, key);
                assert!(!exception_pending(py), "recursive importer: {builtin}");
                assert!(!is_none_bits(value));
                dec_ref_bits(py, value);
                dec_ref_bits(py, key);
            }
            for hidden in ["NoneType", "list_iterator", "GenericAlias"] {
                let key = MoltObject::from_ptr(alloc_string(py, hidden.as_bytes())).bits();
                assert!(
                    unsafe { crate::dict_get_in_place(py, dict, key) }.is_none(),
                    "{hidden}"
                );
                dec_ref_bits(py, key);
            }
            table.states[idx].store(STATE_READY, Ordering::Release);
            table.owners[idx].store(0, Ordering::Release);
            for builtin in ["property", "len", "ValueError"] {
                let key = MoltObject::from_ptr(alloc_string(py, builtin.as_bytes())).bits();
                assert!(unsafe { crate::dict_del_in_place(py, dict, key) });
                // Republication is not permission to repair a user mutation.
                let result = crate::molt_module_cache_set(name, module);
                if !is_none_bits(result) {
                    dec_ref_bits(py, result);
                }
                let value = crate::molt_module_get_global(unrelated, key);
                assert!(is_none_bits(value));
                assert!(pending_exception_text(py, "NameError").contains("NameError"));
                assert!(unsafe { crate::dict_get_in_place(py, dict, key) }.is_none());
                dec_ref_bits(py, key);
            }
            for bits in [property_name, unrelated, module, name] {
                dec_ref_bits(py, bits);
            }
        });
    }

    #[test]
    fn builtin_bootstrap_lookup_is_exact_namespace_and_execution_owned() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        install_test_registry();
        crate::with_gil_entry_nopanic!(_py, {
            let _io_restore = ModuleTestRestore::capture(_py, "_io");
            let _builtins_restore = ModuleTestRestore::capture(_py, "builtins");
            let builtins_id = test_registry_id("builtins");
            let table = module_table(_py).expect("table");
            let idx = builtins_id as usize;
            legacy_cache_del(_py, "builtins");
            assert!(!exception_pending(_py));
            let builtins_bits = publish_test_module("builtins");
            let builtins_ptr = obj_from_bits(builtins_bits).as_ptr().expect("builtins");
            // A same-named module is still a distinct namespace: spelling is
            // not sufficient to obtain bootstrap execution privileges.
            let unrelated_ptr = crate::object::builders::alloc_module_obj(_py, unsafe {
                crate::object::layout::module_name_bits(builtins_ptr)
            });
            assert!(!unrelated_ptr.is_null());
            let unrelated_bits = MoltObject::from_ptr(unrelated_ptr).bits();
            let dict_bits = unsafe { crate::object::layout::module_dict_bits(builtins_ptr) };
            let dict_ptr = obj_from_bits(dict_bits).as_ptr().expect("builtins dict");
            let name = alloc_string(_py, b"len");
            assert!(!name.is_null());
            let name_bits = MoltObject::from_ptr(name).bits();
            assert!(!module_execution_owns_initializing_namespace(
                _py,
                "builtins",
                builtins_bits
            ));
            assert!(!module_execution_owns_initializing_namespace(
                _py,
                "absent_row",
                builtins_bits
            ));

            // Adopt the published namespace, then reserve a real re-execution
            // transaction. Reservation alone owns no namespace; publication
            // while initializing owns exactly that slot object. Name lookup
            // remains the captured dictionary; it never synthesizes intrinsics.
            let ready_bits = module_ensure(_py, builtins_id);
            assert_eq!(ready_bits, builtins_bits);
            dec_ref_bits(_py, ready_bits);
            let snapshot = ModuleTestRestore::execution(
                _py,
                begin_module_execution(_py, "builtins")
                    .expect("reserve builtins execution")
                    .expect("builtins registry row"),
            );
            let owner = crate::concurrency::current_thread_id();
            for state in [STATE_EXECUTION_RESERVED, STATE_INITIALIZING] {
                table.states[idx].store(state, Ordering::Release);
                let namespace_published = state == STATE_INITIALIZING;
                if namespace_published {
                    publish_from_cache_set(_py, "builtins", builtins_bits);
                }
                assert_eq!(
                    module_execution_owns_initializing_namespace(_py, "builtins", builtins_bits),
                    namespace_published
                );
                for through_frame in [false, true] {
                    let value = lookup_test_global(_py, builtins_bits, name_bits, through_frame);
                    assert!(is_none_bits(value));
                    assert!(pending_exception_text(_py, "NameError").contains("NameError"));

                    let unrelated =
                        lookup_test_global(_py, unrelated_bits, name_bits, through_frame);
                    assert!(is_none_bits(unrelated));
                    assert!(pending_exception_text(_py, "NameError").contains("NameError"));

                    // Actual captured builtins values supply both the module
                    // namespace and ordinary fallback lookup during bootstrap.
                    let override_bits = MoltObject::from_int(42).bits();
                    unsafe {
                        crate::object::ops::dict_set_in_place(
                            _py,
                            dict_ptr,
                            name_bits,
                            override_bits,
                        )
                    };
                    let value = lookup_test_global(_py, builtins_bits, name_bits, through_frame);
                    assert_eq!(value, override_bits);
                    assert!(!exception_pending(_py));
                    let cached = lookup_test_global(_py, unrelated_bits, name_bits, through_frame);
                    assert_eq!(cached, override_bits);
                    assert!(!exception_pending(_py));
                    unsafe {
                        assert!(crate::object::ops::dict_del_in_place(
                            _py, dict_ptr, name_bits
                        ))
                    };

                    for module in [builtins_bits, unrelated_bits] {
                        let deleted = lookup_test_global(_py, module, name_bits, through_frame);
                        assert!(is_none_bits(deleted));
                        assert!(pending_exception_text(_py, "NameError").contains("NameError"));
                    }

                    table.owners[idx].store(
                        owner.checked_add(1).expect("foreign thread id"),
                        Ordering::Release,
                    );
                    assert!(!module_execution_owns_initializing_namespace(
                        _py,
                        "builtins",
                        builtins_bits
                    ));
                    let foreign = lookup_test_global(_py, builtins_bits, name_bits, through_frame);
                    table.owners[idx].store(owner, Ordering::Release);
                    assert!(is_none_bits(foreign));
                    assert!(pending_exception_text(_py, "NameError").contains("NameError"));
                }
            }
            // A replaced visible cache entry cannot borrow the old table
            // initializer's ownership, even on its thread and under its name.
            {
                let cache = crate::builtins::exceptions::internals::module_cache(_py);
                let mut guard = cache.lock().unwrap();
                inc_ref_bits(_py, unrelated_bits);
                let displaced = guard
                    .insert("builtins".to_string(), unrelated_bits)
                    .expect("cached builtins");
                dec_ref_bits(_py, displaced);
            }
            assert!(module_execution_owns_initializing_namespace(
                _py,
                "builtins",
                builtins_bits
            ));
            assert!(!module_execution_owns_initializing_namespace(
                _py,
                "builtins",
                unrelated_bits
            ));
            for through_frame in [false, true] {
                let replacement = lookup_test_global(_py, unrelated_bits, name_bits, through_frame);
                assert!(is_none_bits(replacement));
                assert!(pending_exception_text(_py, "NameError").contains("NameError"));
            }
            {
                let cache = crate::builtins::exceptions::internals::module_cache(_py);
                let mut guard = cache.lock().unwrap();
                inc_ref_bits(_py, builtins_bits);
                let displaced = guard
                    .insert("builtins".to_string(), builtins_bits)
                    .expect("replacement builtins");
                dec_ref_bits(_py, displaced);
            }
            snapshot.restore();
            assert_eq!(table.states[idx].load(Ordering::Acquire), STATE_READY);
            for state in [STATE_READY, STATE_TOMBSTONE] {
                if state == STATE_TOMBSTONE {
                    module_table_view_tombstone(_py, builtins_id);
                    // Keep a held namespace visible to exercise revocation,
                    // without redefining absent-cache lazy startup policy.
                    legacy_cache_set(_py, "builtins", builtins_bits);
                }
                // Even a stale owner field must not reopen a closed state.
                table.owners[idx].store(owner, Ordering::Release);
                assert!(!module_execution_owns_initializing_namespace(
                    _py,
                    "builtins",
                    builtins_bits
                ));
                for through_frame in [false, true] {
                    let missing = lookup_test_global(_py, builtins_bits, name_bits, through_frame);
                    assert!(is_none_bits(missing));
                    assert!(pending_exception_text(_py, "NameError").contains("NameError"));
                }
                table.owners[idx].store(0, Ordering::Release);
            }
            dec_ref_bits(_py, name_bits);
            dec_ref_bits(_py, builtins_bits);
            dec_ref_bits(_py, unrelated_bits);
        });
    }

    fn assert_g4_ensure_state_machine_transitions(public_cache: bool) {
        crate::with_gil_entry_nopanic!(_py, {
            let _ = crate::molt_exception_clear();
            let sys = public_cache.then(|| publish_test_module("sys"));
            let modules_bits = sys.map(|bits| {
                crate::builtins::modules::sys_modules_dict_bits(_py, bits)
                    .expect("published sys.modules")
            });
            let modules = modules_bits.map(|bits| obj_from_bits(bits).as_ptr().unwrap());
            let _modules_owner = modules.map(crate::PtrDropGuard::new);
            let src_name = crate::attr_name_bits_from_bytes(_py, b"g4_src").unwrap();

            // ── Uninit → Initializing → Ready; init exactly once (I5) ──
            let runs_before = INIT_RUNS.load(Ordering::SeqCst);
            let src_id = test_registry_id("g4_src");
            let first = module_ensure(_py, src_id);
            assert!(!exception_pending(_py));
            assert!(!is_none_bits(first));
            assert_eq!(INIT_RUNS.load(Ordering::SeqCst), runs_before + 1);
            // Hot path: second ensure returns the same object, no re-init.
            let second = module_ensure(_py, src_id);
            assert_eq!(first, second, "Ready hot path must return the same module");
            assert_eq!(
                INIT_RUNS.load(Ordering::SeqCst),
                runs_before + 1,
                "init must run exactly once (alias/dup-ensure cannot re-enter)"
            );
            dec_ref_bits(_py, first);
            dec_ref_bits(_py, second);

            // ── Failure unwind: Initializing → Uninit, retry re-runs ──
            let fail_id = test_registry_id("g4_fail");
            let failed = module_ensure(_py, fail_id);
            assert!(is_none_bits(failed));
            let text = pending_exception_text(_py, "ValueError");
            assert!(text.contains("g4 init failure"), "{text}");
            // The row must be back to Uninit: a retry re-enters init and
            // fails identically instead of returning a phantom module.
            let retry = module_ensure(_py, fail_id);
            assert!(is_none_bits(retry));
            let text = pending_exception_text(_py, "ValueError");
            assert!(text.contains("g4 init failure"), "{text}");

            // ── Static-extension failure unwind: Initializing → Uninit ──
            let ext_fail_id = test_registry_id("g4_z_static_ext_fail");
            let ext_runs_before = EXT_FAIL_RUNS.load(Ordering::SeqCst);
            let ext_failed = module_ensure(_py, ext_fail_id);
            assert!(is_none_bits(ext_failed));
            let text = pending_exception_text(_py, "ImportError");
            assert!(text.contains("ImportError"), "{text}");
            assert!(
                text.contains("static-link PyModuleDef Py_mod_exec slot returned non-zero"),
                "{text}"
            );
            let table = module_table(_py).expect("table");
            assert_eq!(
                table.states[ext_fail_id as usize].load(Ordering::Acquire),
                STATE_UNINIT,
                "failed static extension init must unwind out of Initializing"
            );
            assert_eq!(
                table.owners[ext_fail_id as usize].load(Ordering::Acquire),
                0,
                "failed static extension init must release its ensure owner"
            );
            let ext_retry = module_ensure(_py, ext_fail_id);
            assert!(is_none_bits(ext_retry));
            let text = pending_exception_text(_py, "ImportError");
            assert!(
                text.contains("static-link PyModuleDef Py_mod_exec slot returned non-zero"),
                "{text}"
            );
            assert_eq!(
                EXT_FAIL_RUNS.load(Ordering::SeqCst),
                ext_runs_before + 2,
                "retry after failed static extension init must re-enter ensure, not \
                 observe a wedged Initializing row"
            );

            // ── Same-thread cycle: partial module visible (I6, row 5.4) ──
            let observed = CYCLE_OBSERVED_PARTIAL.load(Ordering::SeqCst);
            let a = module_ensure(_py, test_registry_id("g4_cycle_a"));
            assert!(!exception_pending(_py));
            assert!(!is_none_bits(a));
            assert_eq!(CYCLE_OBSERVED_PARTIAL.load(Ordering::SeqCst), observed + 1);
            dec_ref_bits(_py, a);

            // ── Alias co-publication (design §4.3) ──
            let alias_id = test_registry_id("g4_alias");
            let target_id = test_registry_id("g4_target");
            let via_alias = module_ensure(_py, alias_id);
            assert!(!exception_pending(_py));
            let via_target = module_ensure(_py, target_id);
            assert_eq!(via_alias, via_target, "alias must resolve to its target");
            assert_eq!(
                module_execution_target_name("g4_alias"),
                Some("g4_target"),
                "fresh execution must resolve aliases to the initializer owner"
            );
            assert!(
                legacy_cache_lookup(_py, "g4_alias").is_some(),
                "alias co-publication must reach the store under the alias name"
            );
            let table = module_table(_py).expect("table");
            let snapshot = begin_module_execution(_py, "g4_target")
                .expect("reserve ready row")
                .expect("registry row snapshot");
            assert_eq!(
                table.states[target_id as usize].load(Ordering::Acquire),
                STATE_EXECUTION_RESERVED
            );
            assert_eq!(table.slots[target_id as usize].load(Ordering::Acquire), 0);
            assert_eq!(
                table.owners[target_id as usize].load(Ordering::Acquire),
                crate::concurrency::current_thread_id()
            );
            restore_module_execution(_py, Some(snapshot));
            assert_eq!(
                table.states[target_id as usize].load(Ordering::Acquire),
                STATE_READY
            );
            assert_eq!(
                table.slots[target_id as usize].load(Ordering::Acquire),
                via_target,
                "fresh execution restoration must recover the exact displaced slot"
            );
            assert_eq!(table.owners[target_id as usize].load(Ordering::Acquire), 0);
            dec_ref_bits(_py, via_alias);
            dec_ref_bits(_py, via_target);

            // ── Parent-first + bind_parent_attr (row 5.5) ──
            let sub = module_ensure(_py, test_registry_id("g4_pkg.sub"));
            assert!(!exception_pending(_py));
            let pkg = module_ensure(_py, test_registry_id("g4_pkg"));
            let leaf_ptr = alloc_string(_py, b"sub");
            let leaf_bits = MoltObject::from_ptr(leaf_ptr).bits();
            let bound = crate::builtins::modules::molt_module_get_attr(pkg, leaf_bits);
            assert!(!exception_pending(_py));
            assert_eq!(bound, sub, "child must be bound as parent attribute");
            dec_ref_bits(_py, bound);
            dec_ref_bits(_py, leaf_bits);
            dec_ref_bits(_py, pkg);
            dec_ref_bits(_py, sub);

            // ── No init lane: fail closed with CPython's exact message ──
            let missing = module_ensure(_py, test_registry_id("g4_noinit"));
            assert!(is_none_bits(missing));
            let text = pending_exception_text(_py, "ModuleNotFoundError");
            assert!(
                text.contains("No module named 'g4_noinit'"),
                "registry rows without an init lane fail closed: {text}"
            );

            // ── Tombstone → Source reinit: full re-exec, NEW object (5.3) ──
            let tomb_id = test_registry_id("g4_tomb");
            let before = module_ensure(_py, tomb_id);
            assert!(!exception_pending(_py));
            // The view transition projects a public deletion; it does not
            // itself mutate sys.modules. Remove the real publication first.
            legacy_cache_del(_py, "g4_tomb");
            module_table_view_tombstone(_py, tomb_id);
            let after = module_ensure(_py, tomb_id);
            assert!(!exception_pending(_py));
            assert!(!is_none_bits(after));
            assert_ne!(
                before, after,
                "tombstoned source module must re-execute into a NEW object"
            );
            dec_ref_bits(_py, before);
            dec_ref_bits(_py, after);

            // The registry does not invent extension reinit policy. It runs
            // the admitted initializer; real C phase behavior is tested there.
            let ext_id = test_registry_id("g4_tomb_ext");
            let ext = module_ensure(_py, ext_id);
            assert!(!exception_pending(_py));
            legacy_cache_del(_py, "g4_tomb_ext");
            module_table_view_tombstone(_py, ext_id);
            let resurrect = module_ensure(_py, ext_id);
            assert!(!exception_pending(_py));
            assert!(!is_none_bits(resurrect));
            assert_ne!(ext, resurrect);
            dec_ref_bits(_py, ext);
            dec_ref_bits(_py, resurrect);

            // ── Replaced(obj): pass-through; Replaced(None): halted (5.2) ──
            let src_bits = module_ensure(_py, src_id);
            let replacement = module_ensure(_py, target_id);
            if let Some(modules) = modules {
                unsafe { crate::dict_set_in_place(_py, modules, src_name, replacement) };
            }
            module_table_view_replace(_py, src_id, replacement);
            let via_replace = module_ensure(_py, src_id);
            assert_eq!(
                via_replace, replacement,
                "Replaced(obj) must return the user object as-is (row 5.1)"
            );
            dec_ref_bits(_py, via_replace);
            if let Some(modules) = modules {
                unsafe { crate::dict_set_in_place(_py, modules, src_name, none_bits()) };
            }
            module_table_view_replace(_py, src_id, none_bits());
            let halted = module_ensure(_py, src_id);
            assert!(is_none_bits(halted));
            let text = pending_exception_text(_py, "ModuleNotFoundError");
            assert!(
                text.contains("import of g4_src halted; None in sys.modules"),
                "row 5.2 exact message: {text}"
            );
            dec_ref_bits(_py, replacement);
            dec_ref_bits(_py, src_bits);

            // ── R2.4: a view transition racing the finishing owner wins ──
            // (drive it directly: set Replaced mid-Initializing via the view
            // entry point, then confirm ensure honors the view's state)
            let table = module_table(_py).expect("table");
            assert_eq!(
                table.states[src_id as usize].load(Ordering::Acquire),
                STATE_REPLACED,
                "view transition must not be overwritten by ensure"
            );

            // ── Dynamic dispatch: registry miss falls to the legacy store ──
            let missing = isolate_import_dispatch(_py, "g4_not_a_module");
            assert!(is_none_bits(missing));
            assert!(!exception_pending(_py));
            dec_ref_bits(_py, src_name);
            if let Some(sys) = sys {
                dec_ref_bits(_py, sys);
            }
        });
    }

    #[test]
    fn g4_ensure_state_machine_transitions() {
        // Both cache regimes have the same state-machine contract. Fresh
        // runtime custody prevents earlier tests from selecting the regime.
        for public_cache in [false, true] {
            crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
                install_test_registry();
                assert_g4_ensure_state_machine_transitions(public_cache);
            });
        }
    }

    #[test]
    fn r0_static_extension_init_failure_unwinds_initializing() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        install_test_registry();
        crate::with_gil_entry_nopanic!(_py, {
            let _ = crate::molt_exception_clear();

            let ext_fail_id = test_registry_id("g4_z_static_ext_fail");
            let runs_before = EXT_FAIL_RUNS.load(Ordering::SeqCst);
            let failed = module_ensure(_py, ext_fail_id);
            assert!(is_none_bits(failed));
            let text = pending_exception_text(_py, "ImportError");
            assert!(
                text.contains("static-link PyModuleDef Py_mod_exec slot returned non-zero"),
                "{text}"
            );

            let table = module_table(_py).expect("table");
            assert_eq!(
                table.states[ext_fail_id as usize].load(Ordering::Acquire),
                STATE_UNINIT,
                "failed static extension init must unwind the module row"
            );
            assert_eq!(
                table.owners[ext_fail_id as usize].load(Ordering::Acquire),
                0,
                "failed static extension init must release the ensure owner"
            );

            let retry = module_ensure(_py, ext_fail_id);
            assert!(is_none_bits(retry));
            let text = pending_exception_text(_py, "ImportError");
            assert!(
                text.contains("static-link PyModuleDef Py_mod_exec slot returned non-zero"),
                "{text}"
            );
            assert_eq!(
                EXT_FAIL_RUNS.load(Ordering::SeqCst),
                runs_before + 2,
                "retry must re-enter the extension init path instead of observing \
                 a wedged Initializing row"
            );
        });
    }

    #[test]
    fn shutdown_registry_releases_owned_aliases_and_refuses_reconstruction() {
        install_test_registry();
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(py, {
                let module = publish_test_module("g4_target");
                let target = test_registry_id("g4_target");
                let alias = test_registry_id("g4_alias");
                module_table_view_replace(py, target, module);
                module_table_view_replace(py, alias, module);
                let roots = take_module_roots_for_shutdown(py, runtime_state(py), |name, _| {
                    name == "builtins"
                });
                assert_eq!(roots.iter().filter(|&&bits| bits == module).count(), 2);
                for bits in roots {
                    dec_ref_bits(py, bits);
                }
                let table = module_table(py).unwrap();
                assert_eq!(table.slots[target as usize].load(Ordering::Acquire), 0);
                assert_eq!(table.slots[alias as usize].load(Ordering::Acquire), 0);
                for id in [target, alias] {
                    inc_ref_bits(py, module);
                    let result = module_ensure_with_cache(
                        py,
                        id,
                        Some(crate::builtins::modules::PublicModuleCache::Present(module)),
                    );
                    assert!(is_none_bits(result));
                    assert!(exception_pending(py));
                    crate::clear_exception(py);
                }
                assert!(begin_module_execution(py, "g4_target").is_err());
                crate::clear_exception(py);
                module_table_view_replace(py, target, module);
                module_table_view_tombstone(py, target);
                assert_eq!(
                    table.states[target as usize].load(Ordering::Acquire),
                    STATE_RETIRED
                );
                dec_ref_bits(py, module);
            });
        });
    }

    #[test]
    fn g4_registry_blob_shape_is_validated() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        // Corrupt magic fails closed.
        let mut builder = BlobBuilder::new();
        builder.row("zz_shape", 0, None, None, MODULE_KIND_SOURCE, 0);
        let mut blob = builder.build();
        blob[0] ^= 0xFF;
        assert!(ModuleRegistry::parse(blob.as_ptr()).is_err());
        // Unsorted rows fail closed.
        let mut unsorted = BlobBuilder::new();
        unsorted
            .rows
            .push(("zz_b".to_string(), 0, None, None, MODULE_KIND_SOURCE, 0));
        unsorted
            .rows
            .push(("zz_a".to_string(), 0, None, None, MODULE_KIND_SOURCE, 0));
        let mut names = Vec::new();
        let mut blob = Vec::new();
        blob.extend_from_slice(&MODULE_REGISTRY_MAGIC.to_le_bytes());
        blob.extend_from_slice(&MODULE_REGISTRY_SCHEMA_VERSION.to_le_bytes());
        blob.extend_from_slice(&2u32.to_le_bytes());
        blob.extend_from_slice(&[0u8; 16]);
        let mut spans = Vec::new();
        for (name, ..) in &unsorted.rows {
            spans.push((names.len() as u32, name.len() as u32));
            names.extend_from_slice(name.as_bytes());
        }
        blob.extend_from_slice(&(names.len() as u64).to_le_bytes());
        blob.extend_from_slice(&0u64.to_le_bytes());
        for (off, len) in &spans {
            blob.extend_from_slice(&off.to_le_bytes());
            blob.extend_from_slice(&len.to_le_bytes());
            blob.extend_from_slice(&0u64.to_le_bytes());
            blob.extend_from_slice(&NO_MODULE_ID.to_le_bytes());
            blob.extend_from_slice(&NO_MODULE_ID.to_le_bytes());
            blob.push(MODULE_KIND_SOURCE);
            blob.push(0);
            blob.extend_from_slice(&0u16.to_le_bytes());
            blob.extend_from_slice(&0u32.to_le_bytes());
            blob.extend_from_slice(&0u32.to_le_bytes());
            blob.extend_from_slice(&0u32.to_le_bytes());
        }
        blob.extend_from_slice(&names);
        let err = ModuleRegistry::parse(blob.as_ptr()).expect_err("unsorted must fail");
        assert!(err.contains("sorted-name order"), "{err}");
        // Wrong schema fails closed with a which-artifact diagnostic.
        let mut builder = BlobBuilder::new();
        builder.row("zz_schema", 0, None, None, MODULE_KIND_SOURCE, 0);
        let mut blob = builder.build();
        blob[8..12].copy_from_slice(&(MODULE_REGISTRY_SCHEMA_VERSION + 1).to_le_bytes());
        let err = ModuleRegistry::parse(blob.as_ptr()).expect_err("schema must fail");
        assert!(err.contains("schema"), "{err}");

        // Alias cycles are artifact corruption, not a runtime recursion path.
        let mut aliases = BlobBuilder::new();
        aliases
            .row("zz_alias_a", 0, None, Some(1), MODULE_KIND_ALIAS, 0)
            .row("zz_alias_b", 0, None, Some(0), MODULE_KIND_ALIAS, 0);
        let blob = aliases.build();
        let err = ModuleRegistry::parse(blob.as_ptr()).expect_err("alias cycle must fail");
        assert!(err.contains("does not terminate"), "{err}");
    }

    #[cfg(feature = "l7-attestation-probe")]
    #[test]
    #[ignore = "release import-catalog performance/allocation attestation"]
    fn import_catalog_perf_attestation() {
        use std::hint::black_box;
        use std::time::Instant;

        let _guard = crate::test_support::RuntimeTestTransaction::new();
        install_test_registry();
        crate::with_gil_entry_nopanic!(_py, {
            let _ = crate::molt_exception_clear();
            let registry = module_registry().expect("registry");
            let src_id = test_registry_id("g4_src");
            let warm = module_ensure(_py, src_id);
            assert!(!exception_pending(_py));
            dec_ref_bits(_py, warm);
            let warm_snapshot = begin_module_execution(_py, "g4_src")
                .expect("warm reexec reservation")
                .expect("catalog row");
            restore_module_execution(_py, Some(warm_snapshot));

            const RESOLVE_ITERS: usize = 2_000_000;
            const ENSURE_ITERS: usize = 250_000;
            const REEXEC_ITERS: usize = 250_000;

            let started = Instant::now();
            for _ in 0..RESOLVE_ITERS {
                black_box(registry.id_of(black_box("g4_src")));
            }
            let resolve_ns = started.elapsed().as_nanos() as f64 / RESOLVE_ITERS as f64;

            crate::attestation_probe::reset();
            crate::attestation_probe::set_tracking(true);
            for _ in 0..RESOLVE_ITERS {
                black_box(registry.id_of(black_box("g4_src")));
            }
            crate::attestation_probe::set_tracking(false);
            let resolve_alloc = crate::attestation_probe::snapshot();

            let started = Instant::now();
            for _ in 0..ENSURE_ITERS {
                let bits = black_box(module_ensure(_py, src_id));
                dec_ref_bits(_py, bits);
            }
            let ensure_ns = started.elapsed().as_nanos() as f64 / ENSURE_ITERS as f64;

            crate::attestation_probe::reset();
            crate::attestation_probe::set_tracking(true);
            for _ in 0..ENSURE_ITERS {
                let bits = black_box(module_ensure(_py, src_id));
                dec_ref_bits(_py, bits);
            }
            crate::attestation_probe::set_tracking(false);
            let ensure_alloc = crate::attestation_probe::snapshot();

            let started = Instant::now();
            for _ in 0..REEXEC_ITERS {
                let snapshot = begin_module_execution(_py, "g4_src")
                    .expect("reexec reservation")
                    .expect("catalog row");
                restore_module_execution(_py, Some(snapshot));
            }
            let reexec_ns = started.elapsed().as_nanos() as f64 / REEXEC_ITERS as f64;

            crate::attestation_probe::reset();
            crate::attestation_probe::set_tracking(true);
            for _ in 0..REEXEC_ITERS {
                let snapshot = begin_module_execution(_py, "g4_src")
                    .expect("reexec reservation")
                    .expect("catalog row");
                restore_module_execution(_py, Some(snapshot));
            }
            crate::attestation_probe::set_tracking(false);
            let reexec_alloc = crate::attestation_probe::snapshot();

            let parallel_started = Instant::now();
            std::thread::scope(|scope| {
                for _ in 0..8 {
                    scope.spawn(|| {
                        for _ in 0..250_000 {
                            black_box(registry.id_of(black_box("g4_src")));
                        }
                    });
                }
            });
            let parallel_resolve_ns = parallel_started.elapsed().as_nanos() as f64 / 2_000_000.0;

            assert_eq!(resolve_alloc.allocations, 0, "resolver allocated");
            assert_eq!(ensure_alloc.allocations, 0, "Ready ensure allocated");
            assert_eq!(reexec_alloc.allocations, 0, "reexec reservation allocated");
            eprintln!(
                "IMPORT_CATALOG_ATTESTATION {{\"resolve_ns_per_op\":{resolve_ns:.3},\
                 \"ready_ensure_ns_per_op\":{ensure_ns:.3},\
                 \"reexec_reserve_restore_ns_per_op\":{reexec_ns:.3},\
                 \"parallel_8x_resolve_ns_per_op\":{parallel_resolve_ns:.3},\
                 \"resolver_allocations\":{},\"ready_ensure_allocations\":{},\
                 \"reexec_allocations\":{}}}",
                resolve_alloc.allocations, ensure_alloc.allocations, reexec_alloc.allocations,
            );
        });
    }
}
