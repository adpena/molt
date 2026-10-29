//! One interpreter-owned ordinary Python module dictionary, exposed unchanged
//! as sys.modules, plus typed initialization metadata for the compiled catalog.
//! The canonical contract is import_bedrock_pr2_sys_modules_view_cutover.md.
//!
//! Normal native/WASM/C-API imports consult the same callback-safe dictionary.
//! Arbitrary values are returned; None halts import; deletion cannot resurrect
//! a shadow cache. READY rows retain no object owner. Ordinary initializer slots
//! and explicit loader/runpy contexts retain their own temporary namespaces.
//! Interpreter sys and builtins roles have independent semantic lifetimes;
//! public dictionary mutations never retarget them. Namespace shutdown detaches
//! all module edges before finalizers, in ordinary/sys/builtins cohorts.
//!
//! ModuleRegistry is the sole immutable compiled-name/initializer catalog,
//! written by molt.cli.module_registry and read here. Dynamic module names live
//! in the same ordinary dictionary without requiring a compiled registry row.
//! Native relocations and the WASM ModuleId dispatcher enter this same state
//! machine. No target-specific module storage or protocol is introduced.
use crate::runtime_flags::{self, RuntimeFlag};
use std::cell::Cell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use molt_obj_model::MoltObject;

use crate::PyToken;
#[cfg(test)]
use crate::alloc_string;
use crate::{
    dec_ref_bits, exception_pending, inc_ref_bits, obj_from_bits, raise_exception, runtime_state,
};

// ─── Blob layout (reader side; writer authority: molt.cli.module_registry) ──
// Structural gate: tests/test_module_registry_gates.py asserts these
// constants equal the Python authority's. Bump both in the same arc.

pub(crate) const MODULE_REGISTRY_SCHEMA_VERSION: u32 = 5;
const MODULE_REGISTRY_MAGIC: u64 = u64::from_le_bytes(*b"MOLTMOD5");
const MODULE_REGISTRY_HEADER_BYTES: usize = 56;
const MODULE_REGISTRY_ROW_BYTES: usize = 48;
const NO_MODULE_ID: u32 = u32::MAX;

const MODULE_KIND_SOURCE: u8 = 0;
const MODULE_KIND_EXTENSION: u8 = 1;
const MODULE_KIND_ALIAS: u8 = 2;
const MODULE_KIND_NAMESPACE_PARENT: u8 = 3;
const MODULE_KIND_RUNTIME_BUILTIN: u8 = 4;

#[allow(dead_code)] // consumed by the PR4 extension-snapshot reinit lane
const MODULE_FLAG_REINIT_RESURRECT: u8 = 0x01;
const MODULE_FLAG_PACKAGE: u8 = 0x02;
const MODULE_FLAG_HAS_BODY: u8 = 0x04;
const MODULE_FLAG_HAS_LOCATION: u8 = 0x08;
const MODULE_FLAG_STDLIB_PROVIDER: u8 = 0x10;
const MODULE_FLAG_KNOWN_MASK: u8 = MODULE_FLAG_REINIT_RESURRECT
    | MODULE_FLAG_PACKAGE
    | MODULE_FLAG_HAS_BODY
    | MODULE_FLAG_HAS_LOCATION
    | MODULE_FLAG_STDLIB_PROVIDER;

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

/// Borrowed metadata from the installed immutable registry; no module owner.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CompiledModuleSpec {
    pub(crate) provider_name: &'static str,
    pub(crate) is_stdlib_provider: bool,
    pub(crate) has_body: bool,
    pub(crate) origin: &'static str,
    pub(crate) is_package: bool,
    pub(crate) has_location: bool,
    pub(crate) namespace_portions: Option<NamespacePortions>,
}

/// Ordered UTF-8 directories borrowed from a validated namespace registry row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NamespacePortions(&'static [u8]);

impl NamespacePortions {
    pub(crate) fn iter(self) -> impl Iterator<Item = &'static str> {
        let mut remaining = self.0;
        std::iter::from_fn(move || {
            if remaining.is_empty() {
                return None;
            }
            let len = u32::from_le_bytes(remaining[..4].try_into().unwrap()) as usize;
            let portion = std::str::from_utf8(&remaining[4..4 + len])
                .expect("namespace portions validated at registry install");
            remaining = &remaining[4 + len..];
            Some(portion)
        })
    }
}

#[derive(Debug)]
pub(crate) struct ModuleRegistry {
    base: *const u8,
    count: u32,
    names_off: usize,
    origins_off: usize,
    origins_len: usize,
    portions_off: usize,
    portions_len: usize,
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

    fn portions_of(&self, id: u32) -> NamespacePortions {
        let row = self.row_base(id);
        let off = Self::read_u32(unsafe { row.add(36) }) as usize;
        let len = Self::read_u32(unsafe { row.add(40) }) as usize;
        NamespacePortions(unsafe {
            std::slice::from_raw_parts(self.base.add(self.portions_off + off), len)
        })
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

    fn execution_target_id(&self, mut id: u32) -> u32 {
        loop {
            let row = self.row(id);
            if row.kind != MODULE_KIND_ALIAS {
                return id;
            }
            id = row
                .alias_target
                .expect("alias target validated at registry install");
        }
    }

    /// Immutable description of an executable catalog row, without touching
    /// module state or invoking an initializer. Alias metadata belongs to the
    /// terminal provider. Reserved/no-body rows cannot supply an executable
    /// compiled loader. Namespace rows describe packages without a body.
    pub(crate) fn compiled_spec(&self, name: &str) -> Option<CompiledModuleSpec> {
        self.provider_spec(name)
            .filter(|spec| spec.has_body || spec.namespace_portions.is_some())
    }

    /// Canonical source/provider facts also describe non-executable rows.
    /// Body admission is a separate fact; aliases inherit their terminal owner.
    pub(crate) fn provider_spec(&self, name: &str) -> Option<CompiledModuleSpec> {
        let id = self.execution_target_id(self.id_of(name)?);
        let row = self.row(id);
        Some(CompiledModuleSpec {
            provider_name: self.name_of(id),
            is_stdlib_provider: row.module_flags & MODULE_FLAG_STDLIB_PROVIDER != 0,
            has_body: row.module_flags & MODULE_FLAG_HAS_BODY != 0,
            origin: self.origin_of(id),
            is_package: row.module_flags & MODULE_FLAG_PACKAGE != 0,
            has_location: row.module_flags & MODULE_FLAG_HAS_LOCATION != 0,
            namespace_portions: (row.kind == MODULE_KIND_NAMESPACE_PARENT)
                .then(|| self.portions_of(id)),
        })
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
        let portions_len = usize::try_from(Self::read_u64(unsafe { base.add(48) }))
            .map_err(|_| "module registry portions table exceeds the target address space")?;
        let names_off = usize::try_from(count)
            .ok()
            .and_then(|count| count.checked_mul(MODULE_REGISTRY_ROW_BYTES))
            .and_then(|rows_len| MODULE_REGISTRY_HEADER_BYTES.checked_add(rows_len))
            .ok_or("module registry row table exceeds the target address space")?;
        let origins_off = names_off
            .checked_add(names_len)
            .ok_or("module registry name table offset overflow")?;
        let portions_off = origins_off
            .checked_add(origins_len)
            .ok_or("module registry origin table offset overflow")?;
        let total_len = portions_off
            .checked_add(portions_len)
            .ok_or("module registry portions table offset overflow")?;
        base.addr()
            .checked_add(total_len)
            .ok_or("module registry address range overflow")?;
        let registry = Self {
            base,
            count,
            names_off,
            origins_off,
            origins_len,
            portions_off,
            portions_len,
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
            let portion_off = Self::read_u32(unsafe { row_base.add(36) }) as usize;
            let portion_len = Self::read_u32(unsafe { row_base.add(40) }) as usize;
            if portion_off
                .checked_add(portion_len)
                .is_none_or(|end| end > registry.portions_len)
            {
                return Err(format!(
                    "module registry row {id} portions span exceeds the portions table"
                ));
            }
            let mut portions = registry.portions_of(id).0;
            while !portions.is_empty() {
                let Some(length) = portions.get(..4) else {
                    return Err(format!("module registry row {id} truncated portion length"));
                };
                let length = u32::from_le_bytes(length.try_into().unwrap()) as usize;
                portions = &portions[4..];
                let Some(portion) = portions.get(..length) else {
                    return Err(format!("module registry row {id} truncated portion"));
                };
                if portion.is_empty() || std::str::from_utf8(portion).is_err() {
                    return Err(format!(
                        "module registry row {id} portion is empty or not UTF-8"
                    ));
                }
                portions = &portions[length..];
            }
            if row.kind == MODULE_KIND_NAMESPACE_PARENT {
                if portion_len == 0
                    || origin_len != 0
                    || row.init_ptr != 0
                    || row.alias_target.is_some()
                    || row.module_flags != MODULE_FLAG_PACKAGE
                {
                    return Err(format!(
                        "module registry namespace row {id} requires portions and no origin, location or body"
                    ));
                }
            } else if portion_len != 0 {
                return Err(format!(
                    "module registry non-namespace row {id} carries portions"
                ));
            }
            if row.module_flags & !MODULE_FLAG_KNOWN_MASK != 0 {
                return Err(format!(
                    "module registry row {id} has unknown flags 0x{:02x}",
                    row.module_flags
                ));
            }
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
    let id = registry.execution_target_id(registry.id_of(name)?);
    Some(registry.name_of(id))
}

/// Catalog admission for fresh execution. `None` means no registry is
/// installed (direct-link tests/embedding); `Some(false)` is an authoritative
/// no-body row and must not enter the importer or allocate transaction state.
pub(crate) fn module_execution_target_has_body(name: &str) -> Option<bool> {
    let registry = module_registry()?;
    let id = registry.execution_target_id(registry.id_of(name)?);
    Some(registry.row(id).module_flags & MODULE_FLAG_HAS_BODY != 0)
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

/// The interpreter's builtins dictionary is a semantic role, independent of
/// the mutable modules dictionary. Active frames may bind another namespace.
pub(crate) struct InterpreterBuiltinsNamespace(AtomicU64);

impl InterpreterBuiltinsNamespace {
    pub(crate) fn new() -> Self {
        Self(AtomicU64::new(0))
    }

    fn publish(&self, py: &PyToken<'_>, module: u64) {
        let Some(pointer) = obj_from_bits(module).as_ptr() else {
            return;
        };
        if unsafe { crate::object_type_id(pointer) } != crate::TYPE_ID_MODULE {
            return;
        }
        let dictionary = unsafe { crate::module_dict_bits(pointer) };
        inc_ref_bits(py, dictionary);
        if self
            .0
            .compare_exchange(0, dictionary, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            dec_ref_bits(py, dictionary);
        }
    }

    pub(crate) fn take_for_shutdown(&self, _py: &PyToken<'_>) -> Option<u64> {
        let bits = self.0.swap(none_bits(), Ordering::AcqRel);
        (bits != 0 && !is_none_bits(bits)).then_some(bits)
    }
}

#[cfg(test)]
pub(crate) fn publish_interpreter_builtins_for_test(py: &PyToken<'_>, module: u64) {
    runtime_state(py).interpreter_builtins.publish(py, module);
}

pub(crate) fn interpreter_builtins_dictionary(py: &PyToken<'_>) -> Option<u64> {
    crate::gil_assert();
    let bits = runtime_state(py)
        .interpreter_builtins
        .0
        .load(Ordering::Acquire);
    (bits != 0 && !is_none_bits(bits)).then_some(bits)
}

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
/// bridge. RuntimeTestTransaction escrows and restores this role, its table row
/// and its public `sys.modules['sys']` entry.
#[cfg(test)]
pub(crate) fn publish_interpreter_sys_for_test(py: &PyToken<'_>, bits: u64) -> u64 {
    tests::install_test_registry();
    let _execution = ModuleExecutionGuard::acquire(py, "sys").expect("sys test execution");
    let table = module_table(py).unwrap();
    let index = module_id_of("sys").unwrap() as usize;
    let previous = table.initialization().slots[index].swap(0, Ordering::AcqRel);
    let prior_spec = table.initialization().specs[index].swap(0, Ordering::AcqRel);
    table.initialization().states[index].store(STATE_INITIALIZING, Ordering::Release);
    let name = crate::attr_name_bits_from_bytes(py, b"sys").unwrap();
    let result = crate::builtins::modules::molt_module_cache_set(name, bits);
    dec_ref_bits(py, name);
    table.initialization().states[index].store(STATE_READY, Ordering::Release);
    let completed = table.initialization().slots[index].swap(0, Ordering::AcqRel);
    let completed_spec = table.initialization().specs[index].swap(prior_spec, Ordering::AcqRel);
    if completed_spec != 0 && !is_none_bits(completed_spec) {
        dec_ref_bits(py, completed_spec);
    }
    if completed != 0 {
        dec_ref_bits(py, completed);
    }
    if previous != 0 {
        dec_ref_bits(py, previous);
    }
    result
}

/// One test transaction's custody of the whole `sys` bootstrap: the
/// interpreter roles, the `sys` table row and the public `sys.modules['sys']`
/// entry. The sys initializer publishes the role and the entry together, so
/// detaching only the role would leave a state production never reaches (a
/// public `sys` with no bootstrap), where public-cache precedence returns the
/// entry without running the initializer and every importlib entry point
/// fails to find `sys.modules` (HF-540).
#[cfg(test)]
pub(crate) struct InterpreterSysTestSnapshot {
    defaults: crate::builtins::platform::ImportDefaultsTestSnapshot,
    namespace: u64,
    builtins: u64,
    table: Option<(u8, u64, u64)>,
    /// The owned public `sys` entry, `None` when the canonical dictionary
    /// has none (or does not exist yet).
    public: Option<u64>,
}

#[cfg(test)]
impl InterpreterSysTestSnapshot {
    pub(crate) fn detach(py: &PyToken<'_>) -> Self {
        let state = runtime_state(py);
        let defaults = crate::builtins::platform::ImportDefaultsTestSnapshot::detach(py);
        let namespace = state.interpreter_sys.0.swap(0, Ordering::AcqRel);
        let builtins = state.interpreter_builtins.0.swap(0, Ordering::AcqRel);
        let table = state.module_table.get().and_then(|table| {
            let index = module_id_of("sys")? as usize;
            Some((
                table.initialization().states[index].swap(STATE_UNINIT, Ordering::AcqRel),
                table.initialization().slots[index].swap(0, Ordering::AcqRel),
                table.initialization().specs[index].swap(0, Ordering::AcqRel),
            ))
        });
        let public = existing_module_dictionary(py).and_then(|modules| {
            let key = crate::attr_name_bits_from_bytes(py, b"sys").expect("sys name");
            let detached = unsafe {
                crate::object::ops::dict_del_deferred(py, crate::ptr_from_bits(modules), key)
            }
            .expect("detach the public sys entry");
            dec_ref_bits(py, key);
            detached.map(|entry| {
                let [key, value] = entry.into_owned_storage();
                dec_ref_bits(py, key);
                value
            })
        });
        Self {
            defaults,
            namespace,
            builtins,
            table,
            public,
        }
    }

    pub(crate) fn restore(self, py: &PyToken<'_>) {
        let state = runtime_state(py);
        let current = state
            .interpreter_sys
            .0
            .swap(self.namespace, Ordering::AcqRel);
        let current_builtins = state
            .interpreter_builtins
            .0
            .swap(self.builtins, Ordering::AcqRel);
        let current_table = state.module_table.get().and_then(|table| {
            let index = module_id_of("sys")? as usize;
            let (status, bits, spec) = self.table.unwrap_or((STATE_UNINIT, 0, 0));
            table.initialization().states[index].store(status, Ordering::Release);
            Some((
                table.initialization().slots[index].swap(bits, Ordering::AcqRel),
                table.initialization().specs[index].swap(spec, Ordering::AcqRel),
            ))
        });
        // The test's own entry is released with the other fresh owners below.
        let key = crate::attr_name_bits_from_bytes(py, b"sys").expect("sys name");
        let current_public = existing_module_dictionary(py).and_then(|modules| {
            let modules = crate::ptr_from_bits(modules);
            match self.public {
                Some(bits) => {
                    unsafe { crate::object::ops::dict_set_deferred(py, modules, key, bits) }
                        .map(Some)
                }
                None => unsafe { crate::object::ops::dict_del_deferred(py, modules, key) },
            }
            .expect("restore the public sys entry")
        });
        dec_ref_bits(py, key);
        // Restore the entire interpreter role before retiring any fresh owner:
        // list/module finalizers must see matching sys and bootstrap custody.
        self.defaults.restore(py);
        drop(current_public);
        for bits in [
            self.public,
            Some(current),
            Some(current_builtins),
            current_table.map(|entry| entry.0),
            current_table.map(|entry| entry.1),
        ]
        .into_iter()
        .flatten()
        {
            if bits != 0 && !is_none_bits(bits) {
                dec_ref_bits(py, bits);
            }
        }
    }
}

pub(crate) struct ModuleTable {
    /// The one interpreter module dictionary, also exposed as sys.modules.
    /// It exists independently of the optional compiled-module registry.
    dictionary: AtomicU64,
    initialization: OnceLock<ModuleInitialization>,
    execution: OnceLock<Mutex<ModuleExecutionLocks>>,
}

struct ModuleInitialization {
    states: Box<[AtomicU8]>,
    slots: Box<[AtomicU64]>,
    /// Original specs retained only through their active initializer's finally.
    /// Zero awaits compiler admission; None marks admission in progress.
    specs: Box<[AtomicU64]>,
}

impl ModuleTable {
    fn new() -> Self {
        Self {
            dictionary: AtomicU64::new(0),
            initialization: OnceLock::new(),
            execution: OnceLock::new(),
        }
    }

    fn initialization(&self) -> &ModuleInitialization {
        self.initialization.get_or_init(|| {
            let n = module_registry().expect("compiled module registry").count() as usize;
            ModuleInitialization {
                states: (0..n).map(|_| AtomicU8::new(STATE_UNINIT)).collect(),
                slots: (0..n).map(|_| AtomicU64::new(0)).collect(),
                specs: (0..n).map(|_| AtomicU64::new(0)).collect(),
            }
        })
    }
}

/// One transient synchronization authority for every admitted module name.
/// Records retain no Python object, namespace or spec. Clone/admit/retire all
/// occur under this mutex; callbacks and GIL-released waits never hold it.
#[derive(Default)]
struct ModuleExecutionLocks {
    active: HashMap<Arc<str>, Arc<ModuleExecutionLock>>,
    waiting: HashMap<u64, Arc<ModuleExecutionLock>>,
    retired: bool,
}

struct ModuleExecutionLock {
    name: Arc<str>,
    owner: AtomicU64,
    depth: AtomicUsize,
    waiters: AtomicUsize,
    wake: Condvar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModuleLockFailure {
    Cycle,
    Exception,
}

impl ModuleLockFailure {
    /// All body/discovery lock admissions propagate the same internal Python
    /// exception. Only _lock_unlock_module-style consumers accept a partial.
    pub(crate) fn into_exception(self, py: &PyToken<'_>, name: &str) -> u64 {
        match self {
            Self::Cycle => raise_exception::<_>(
                py,
                "_DeadlockError",
                &format!("deadlock detected by module lock for {name:?}"),
            ),
            Self::Exception => none_bits(),
        }
    }
}

/// Admission itself pins record identity, including before ownership and on
/// rejected/unwound waiters. Every such exit runs the same retirement path.
pub(crate) struct ModuleExecutionGuard {
    table: &'static ModuleTable,
    record: Arc<ModuleExecutionLock>,
    thread: u64,
    acquired: bool,
}

impl ModuleExecutionGuard {
    pub(crate) fn acquire(py: &PyToken<'_>, name: &str) -> Result<Self, ModuleLockFailure> {
        if exception_pending(py) || module_dictionary(py).is_none() {
            return Err(ModuleLockFailure::Exception);
        }
        let table = runtime_state(py).module_table.get().unwrap();
        let thread = crate::concurrency::current_thread_id();
        let record = {
            let mut locks = table
                .execution
                .get_or_init(Default::default)
                .lock()
                .unwrap();
            if locks.retired {
                drop(locks);
                raise_exception::<()>(py, "ImportError", "module execution locks are retired");
                return Err(ModuleLockFailure::Exception);
            }
            if let Some(record) = locks.active.get(name) {
                Arc::clone(record)
            } else {
                let name: Arc<str> = Arc::from(name);
                let record = Arc::new(ModuleExecutionLock {
                    name: Arc::clone(&name),
                    owner: AtomicU64::new(0),
                    depth: AtomicUsize::new(0),
                    waiters: AtomicUsize::new(0),
                    wake: Condvar::new(),
                });
                locks.active.insert(name, Arc::clone(&record));
                record
            }
        };
        let mut guard = Self {
            table,
            record,
            thread,
            acquired: false,
        };
        loop {
            {
                let mut locks = table.execution.get().unwrap().lock().unwrap();
                if locks.retired {
                    drop(locks);
                    drop(guard);
                    raise_exception::<()>(py, "ImportError", "module execution locks are retired");
                    return Err(ModuleLockFailure::Exception);
                }
                let owner = guard.record.owner.load(Ordering::Acquire);
                if owner == 0 || owner == thread {
                    let Some(depth) = guard.record.depth.load(Ordering::Relaxed).checked_add(1)
                    else {
                        drop(locks);
                        drop(guard);
                        raise_exception::<()>(
                            py,
                            "OverflowError",
                            "module execution lock recursion exhausted",
                        );
                        return Err(ModuleLockFailure::Exception);
                    };
                    locks.waiting.remove(&thread);
                    guard.record.owner.store(thread, Ordering::Release);
                    guard.record.depth.store(depth, Ordering::Relaxed);
                    guard.acquired = true;
                    return Ok(guard);
                }
                locks.waiting.insert(thread, Arc::clone(&guard.record));
                // The graph has one outgoing edge per waiting thread. Floyd's
                // walk is linear without allocating a visited set under contention.
                let next = |owner| {
                    locks
                        .waiting
                        .get(&owner)
                        .map_or(0, |record| record.owner.load(Ordering::Acquire))
                };
                let (mut slow, mut fast) = (owner, owner);
                loop {
                    if slow == thread || fast == thread {
                        return Err(ModuleLockFailure::Cycle);
                    }
                    slow = next(slow);
                    fast = next(next(fast));
                    if slow == 0 || fast == 0 || slow == fast {
                        // A distinct cycle cannot be caused by this admission.
                        // Its owners retain the same progress obligation.
                        break;
                    }
                }
            }
            // The record Arc and wait edge survive GIL release. Recheck the
            // predicate under the SAME owner mutex before sleeping: a release
            // between the previous observation and here cannot lose its wake.
            let release = crate::concurrency::GilReleaseGuard::suspend();
            let mut locks = table.execution.get().unwrap().lock().unwrap();
            if !locks.retired && {
                let owner = guard.record.owner.load(Ordering::Acquire);
                owner != 0 && owner != thread
            } {
                guard.record.waiters.fetch_add(1, Ordering::Relaxed);
                locks = guard
                    .record
                    .wake
                    .wait_while(locks, |locks| {
                        !locks.retired && {
                            let owner = guard.record.owner.load(Ordering::Acquire);
                            owner != 0 && owner != thread
                        }
                    })
                    .unwrap();
                guard.record.waiters.fetch_sub(1, Ordering::Relaxed);
            }
            drop(locks);
            // Never reacquire the GIL while holding the execution manager.
            drop(release);
        }
    }
}

impl Drop for ModuleExecutionGuard {
    fn drop(&mut self) {
        let mut locks = self.table.execution.get().unwrap().lock().unwrap();
        if locks
            .waiting
            .get(&self.thread)
            .is_some_and(|record| Arc::ptr_eq(record, &self.record))
        {
            locks.waiting.remove(&self.thread);
        }
        if self.acquired {
            debug_assert_eq!(self.record.owner.load(Ordering::Acquire), self.thread);
            let depth = self.record.depth.fetch_sub(1, Ordering::Relaxed);
            debug_assert_ne!(depth, 0);
            if depth == 1 {
                self.record.owner.store(0, Ordering::Release);
                if self.record.waiters.load(Ordering::Relaxed) != 0 {
                    self.record.wake.notify_all();
                }
            }
        }
        // Map + this final admission are the only remaining references. An
        // admitted waiter always has another Arc before this decision.
        if self.record.owner.load(Ordering::Acquire) == 0
            && Arc::strong_count(&self.record) == 2
            && locks
                .active
                .get(self.record.name.as_ref())
                .is_some_and(|record| Arc::ptr_eq(record, &self.record))
        {
            locks.active.remove(self.record.name.as_ref());
        }
    }
}

fn execution_owner(table: &ModuleTable, name: &str) -> u64 {
    table
        .execution
        .get()
        .and_then(|locks| {
            locks
                .lock()
                .unwrap()
                .active
                .get(name)
                .map(|record| record.owner.load(Ordering::Acquire))
        })
        .unwrap_or(0)
}

fn initializer_owner(table: &ModuleTable, id: usize) -> u64 {
    module_registry().map_or(0, |registry| {
        execution_owner(table, registry.name_of(id as u32))
    })
}

/// The C _lock_unlock_module boundary accepts partial initialization on a
/// detected cycle. Python first enters its separate scoped lock admission.
pub(crate) fn wait_for_module_execution(py: &PyToken<'_>, name: &str) -> Result<(), u64> {
    match ModuleExecutionGuard::acquire(py, name) {
        Ok(guard) => {
            drop(guard);
            Ok(())
        }
        Err(ModuleLockFailure::Cycle) => Ok(()), // _lock_unlock_module partial acceptance
        Err(ModuleLockFailure::Exception) => Err(none_bits()),
    }
}

/// Borrow the canonical dictionary under the runtime execution lease. Allocation
/// failure never publishes a sentinel as a successful dictionary generation.
pub(crate) fn module_dictionary(py: &PyToken<'_>) -> Option<u64> {
    crate::gil_assert();
    let table = runtime_state(py).module_table.get_or_init(ModuleTable::new);
    let current = table.dictionary.load(Ordering::Acquire);
    if current != 0 {
        if is_none_bits(current) {
            raise_exception::<u64>(
                py,
                "RuntimeError",
                "interpreter module dictionary is retired",
            );
            return None;
        }
        return Some(current);
    }
    let pointer = crate::alloc_dict_with_pairs(py, &[]);
    if pointer.is_null() {
        return None;
    }
    let bits = MoltObject::from_ptr(pointer).bits();
    match table
        .dictionary
        .compare_exchange(0, bits, Ordering::AcqRel, Ordering::Acquire)
    {
        Ok(_) => Some(bits),
        Err(installed) => {
            dec_ref_bits(py, bits);
            (!is_none_bits(installed)).then_some(installed)
        }
    }
}

/// A non-allocating borrow for shutdown and metadata enumeration. Callers may
/// not retain backing references across Python callbacks.
pub(crate) fn existing_module_dictionary(py: &PyToken<'_>) -> Option<u64> {
    crate::gil_assert();
    let bits = runtime_state(py)
        .module_table
        .get()?
        .dictionary
        .load(Ordering::Acquire);
    (bits != 0 && !is_none_bits(bits)).then_some(bits)
}

/// An owned dictionary lookup. The ordinary callback-safe dictionary protocol
/// remains authoritative even for string subclasses and colliding user keys.
pub(crate) fn module_lookup(py: &PyToken<'_>, name: &str) -> Result<Option<u64>, u64> {
    let Some(dictionary) = existing_module_dictionary(py) else {
        return Ok(None);
    };
    let Some(key) = crate::attr_name_bits_from_bytes(py, name.as_bytes()) else {
        return Err(none_bits());
    };
    let found = unsafe { crate::dict_get_in_place(py, crate::ptr_from_bits(dictionary), key) };
    let owned = found.map(|value| value.inspect(|bits| inc_ref_bits(py, *bits)));
    dec_ref_bits(py, key);
    owned.map_err(|()| none_bits())
}

#[cfg(test)]
pub(crate) fn replace_module_owned_for_test(
    py: &PyToken<'_>,
    name: &str,
    value: Option<u64>,
) -> Option<u64> {
    let dictionary = module_dictionary(py).expect("canonical module dictionary");
    let key = crate::attr_name_bits_from_bytes(py, name.as_bytes()).unwrap();
    let previous = module_lookup(py, name).expect("module lookup");
    if let Some(value) = value {
        assert!(unsafe {
            crate::dict_set_in_place(py, crate::ptr_from_bits(dictionary), key, value)
        });
        dec_ref_bits(py, value);
    } else {
        unsafe {
            crate::dict_del_in_place(py, crate::ptr_from_bits(dictionary), key)
                .expect("module removal");
        }
    }
    dec_ref_bits(py, key);
    previous
}

/// Retire the dictionary root after the module shutdown fixed point. Its
/// contents were detached before any namespace finalizers were invoked.
fn retire_execution_locks(table: &ModuleTable) {
    if let Some(execution) = table.execution.get() {
        let mut locks = execution.lock().unwrap();
        locks.retired = true;
        for record in locks.active.values() {
            if record.waiters.load(Ordering::Relaxed) != 0 {
                record.wake.notify_all();
            }
        }
        locks.waiting.clear();
        locks.active.clear();
    }
}

pub(crate) fn retire_module_dictionary(py: &PyToken<'_>) {
    if let Some(table) = runtime_state(py).module_table.get() {
        retire_execution_locks(table);
        let bits = table.dictionary.swap(none_bits(), Ordering::AcqRel);
        if bits != 0 && !is_none_bits(bits) {
            dec_ref_bits(py, bits);
        }
    }
}

/// Retain the current public role publication for ordered namespace shutdown.
pub(crate) fn retain_shutdown_namespace(
    py: &PyToken<'_>,
    _state: &crate::state::RuntimeState,
    name: &str,
) -> Option<u64> {
    crate::gil_assert();
    let dictionary = existing_module_dictionary(py)?;
    // Teardown classifies actual stored Unicode-name rows, not Python key
    // equality. A colliding user key must not run code or raise while core
    // namespace cohorts are being captured. Semantic roles are pinned apart.
    let bits =
        unsafe { crate::builtins::containers::dict_entries(crate::ptr_from_bits(dictionary)) }
            .iter()
            .filter(|row| row.hash.is_some())
            .find(|row| {
                crate::string_obj_to_owned(obj_from_bits(row.key)).as_deref() == Some(name)
            })?
            .value;
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
    let Some(table) = state.module_table.get() else {
        return Vec::new();
    };
    let mut detached = Vec::new();
    if let (Some(initialization), Some(registry)) = (table.initialization.get(), module_registry())
    {
        for (index, slot) in initialization.slots.iter().enumerate() {
            let bits = slot.load(Ordering::Acquire);
            if keep(registry.name_of(index as u32), bits) {
                continue;
            }
            table.initialization().states[index].store(STATE_RETIRED, Ordering::Release);
            let bits = slot.swap(0, Ordering::AcqRel);
            if bits != 0 {
                detached.push(bits);
            }
            let spec = initialization.specs[index].swap(0, Ordering::AcqRel);
            if spec != 0 && !is_none_bits(spec) {
                detached.push(spec);
            }
        }
    }
    // Remove exact retained keys using stored hashes, without callbacks or
    // allocations in the dictionary. Pin all removed values before key release.
    if let Some(bits) = existing_module_dictionary(_py) {
        let pointer = crate::ptr_from_bits(bits);
        let selected = unsafe { crate::builtins::containers::dict_entries(pointer) }
            .iter()
            .filter(|row| row.hash.is_some())
            .filter_map(|row| {
                let name = crate::string_obj_to_owned(obj_from_bits(row.key));
                (!keep(name.as_deref().unwrap_or(""), row.value))
                    .then_some((row.key, row.hash.unwrap().get()))
            })
            .collect::<Vec<_>>();
        let mut keys = Vec::with_capacity(selected.len());
        for (key, hash) in selected {
            if let Some(removed) = unsafe {
                crate::object::ops::dict_remove_identity_with_hash_deferred(_py, pointer, key, hash)
            } {
                let [key, value] = removed.into_owned_storage();
                keys.push(key);
                detached.push(value);
            }
        }
        for key in keys {
            dec_ref_bits(_py, key);
        }
    }
    detached
}

fn module_table(_py: &PyToken<'_>) -> Option<&'static ModuleTable> {
    module_registry()?;
    Some(
        runtime_state(_py)
            .module_table
            .get_or_init(ModuleTable::new),
    )
}

fn none_bits() -> u64 {
    MoltObject::none().bits()
}

fn is_none_bits(bits: u64) -> bool {
    obj_from_bits(bits).is_none()
}

#[cfg(test)]
fn publication_for_test(py: &PyToken<'_>, name: &str) -> Option<u64> {
    let bits = module_lookup(py, name).expect("canonical module lookup")?;
    dec_ref_bits(py, bits);
    Some(bits)
}

#[cfg(test)]
fn publish_for_test(py: &PyToken<'_>, name: &str, bits: u64) {
    let name_bits = crate::attr_name_bits_from_bytes(py, name.as_bytes()).unwrap();
    let _ = crate::builtins::modules::molt_module_cache_set(name_bits, bits);
    dec_ref_bits(py, name_bits);
}

#[cfg(test)]
fn remove_for_test(py: &PyToken<'_>, name: &str) {
    let name_bits = crate::attr_name_bits_from_bytes(py, name.as_bytes()).unwrap();
    let _ = crate::builtins::modules::molt_module_cache_del(name_bits);
    dec_ref_bits(py, name_bits);
}

/// A first-publication slot owned by the current initializer, never a
/// same-named cached object, private execution, or another thread's transaction.
fn initializing_publication_slot(_py: &PyToken<'_>, name: &str) -> Option<&'static AtomicU64> {
    let id = module_id_of(name)?;
    let table = module_table(_py)?;
    let idx = id as usize;
    (table.initialization().states[idx].load(Ordering::Acquire) == STATE_INITIALIZING
        && initializer_owner(table, idx) == crate::concurrency::current_thread_id()
        && table.initialization().slots[idx].load(Ordering::Acquire) == 0)
        .then_some(&table.initialization().slots[idx])
}

pub(crate) fn module_initialization_awaits_publication(_py: &PyToken<'_>, name: &str) -> bool {
    initializing_publication_slot(_py, name).is_some()
}

/// Retain the exact namespace only while its initializer owns publication.
pub(crate) fn publish_initializing_namespace(_py: &PyToken<'_>, name: &str, bits: u64) {
    if bits == 0 || is_none_bits(bits) {
        return;
    }
    let Some(id) = module_id_of(name) else {
        return;
    };
    let Some(table) = module_table(_py) else {
        return;
    };
    let index = id as usize;
    if table.initialization().states[index].load(Ordering::Acquire) != STATE_INITIALIZING
        || initializer_owner(table, index) != crate::concurrency::current_thread_id()
    {
        return;
    }
    let slot = &table.initialization().slots[index];
    if slot.load(Ordering::Acquire) == bits {
        return;
    }
    if name == "sys" && !runtime_state(_py).interpreter_sys.publish(_py, bits) {
        return;
    }
    if name == "builtins" {
        runtime_state(_py).interpreter_builtins.publish(_py, bits);
    }
    inc_ref_bits(_py, bits);
    let displaced = slot.swap(bits, Ordering::AcqRel);
    if displaced != 0 {
        molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(_py, displaced));
    }
}

/// Reconcile an extension result with transient initialization custody.
/// A foreign initializer retains its owner.
/// Return any displaced transaction owner for release after publication.
pub(crate) fn publish_extension_result(py: &PyToken<'_>, name: &str, bits: u64) -> u64 {
    let Some(id) = module_id_of(name) else {
        return 0;
    };
    let Some(table) = module_table(py) else {
        return 0;
    };
    let idx = id as usize;
    match table.initialization().states[idx].load(Ordering::Acquire) {
        STATE_INITIALIZING => {
            if initializer_owner(table, idx) != crate::concurrency::current_thread_id() {
                return 0;
            }
            inc_ref_bits(py, bits);
            table.initialization().slots[idx].swap(bits, Ordering::AcqRel)
        }
        _ => module_table_view_replace_deferred(py, id, bits),
    }
}

/// Detach only the current initializer's temporary publication owner.
/// Quiescent rows own no module or public-membership fact: the next ensure's
/// ordinary dictionary observation reconciles absence without a second probe.
/// Foreign initializers retain their transaction custody.
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
    let state = table.initialization().states[idx].load(Ordering::Acquire);
    if state != STATE_INITIALIZING
        || initializer_owner(table, idx) != crate::concurrency::current_thread_id()
    {
        return 0;
    }
    match expected {
        Some(bits) => table.initialization().slots[idx]
            .compare_exchange(bits, 0, Ordering::AcqRel, Ordering::Acquire)
            .unwrap_or(0),
        None => table.initialization().slots[idx].swap(0, Ordering::AcqRel),
    }
}

// ─── Reconcile initialization metadata after ordinary dictionary observation ───

/// Test-only eager release of the production deferred metadata transition.
/// Production publication releases the displaced owner after namespace updates.
#[cfg(test)]
fn module_table_view_replace(_py: &PyToken<'_>, id: u32, bits: u64) {
    let previous = module_table_view_replace_deferred(_py, id, bits);
    if previous != 0 {
        dec_ref_bits(_py, previous);
    }
}

fn module_table_view_replace_deferred(_py: &PyToken<'_>, id: u32, _bits: u64) -> u64 {
    let Some(table) = module_table(_py) else {
        return 0;
    };
    let idx = id as usize;
    if table.initialization().states[idx].load(Ordering::Acquire) == STATE_RETIRED {
        return 0;
    }
    let previous = table.initialization().slots[idx].swap(0, Ordering::AcqRel);
    table.initialization().states[idx].store(STATE_REPLACED, Ordering::Release);
    previous
}

/// `del sys.modules[name]` over a registry name → `Tombstone`.
/// Called by normal ensure after observing the public cache; also tested directly.
pub(crate) fn module_table_view_tombstone(_py: &PyToken<'_>, id: u32) {
    let Some(table) = module_table(_py) else {
        return;
    };
    let idx = id as usize;
    if table.initialization().states[idx].load(Ordering::Acquire) == STATE_RETIRED {
        return;
    }
    table.initialization().states[idx].store(STATE_TOMBSTONE, Ordering::Release);
    let previous = table.initialization().slots[idx].swap(0, Ordering::AcqRel);
    // The dictionary mutation is already authoritative. Only a displaced
    // initializer owner can remain here; no shadow store is consulted.
    if previous != 0 {
        dec_ref_bits(_py, previous);
    }
}

// ─── ensure: the only state-transition owner (design §4.3) ──────────────────

/// Temporary ordinary initializer namespace/spec escrow. Synchronization
/// remains with its scoped name lease and is never moved into row metadata.
pub(crate) struct ModuleExecutionSnapshot {
    id: u32,
    state: u8,
    bits: u64,
    spec: u64,
}

/// Move actual active initializer owners into a nested ordinary-import escrow.
/// This never changes public dictionary membership or invents a name lock.
fn detach_initializer(table: &'static ModuleTable, id: u32) -> ModuleExecutionSnapshot {
    let index = id as usize;
    ModuleExecutionSnapshot {
        id,
        state: table.initialization().states[index].load(Ordering::Acquire),
        bits: table.initialization().slots[index].swap(0, Ordering::AcqRel),
        spec: table.initialization().specs[index].swap(0, Ordering::AcqRel),
    }
}

/// Restore the exact ordinary initializer state displaced by nested import.
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
        if snapshot.spec != 0 && !is_none_bits(snapshot.spec) {
            dec_ref_bits(_py, snapshot.spec);
        }
        return;
    };
    let idx = snapshot.id as usize;
    let fresh = table.initialization().slots[idx].swap(snapshot.bits, Ordering::AcqRel);
    let fresh_spec = table.initialization().specs[idx].swap(snapshot.spec, Ordering::AcqRel);
    table.initialization().states[idx].store(snapshot.state, Ordering::Release);
    if fresh != 0 {
        dec_ref_bits(_py, fresh);
    }
    if fresh_spec != 0 && !is_none_bits(fresh_spec) {
        dec_ref_bits(_py, fresh_spec);
    }
}

static ENSURE_DEPTH: AtomicUsize = AtomicUsize::new(0);
const ENSURE_MAX_DEPTH: usize = 4096;

pub(crate) struct ImportDepthGuard;

pub(crate) fn enter_import_recursion(py: &PyToken<'_>) -> Option<ImportDepthGuard> {
    if ENSURE_DEPTH.fetch_add(1, Ordering::Relaxed) >= ENSURE_MAX_DEPTH {
        ENSURE_DEPTH.fetch_sub(1, Ordering::Relaxed);
        raise_exception::<()>(
            py,
            "RecursionError",
            "module ensure recursion limit exceeded (registry cycle?)",
        );
        None
    } else {
        Some(ImportDepthGuard)
    }
}

impl Drop for ImportDepthGuard {
    fn drop(&mut self) {
        ENSURE_DEPTH.fetch_sub(1, Ordering::Relaxed);
    }
}

thread_local! {
    // Borrowed cold execution identity, not a namespace owner or import clock.
    static CURRENT_INITIALIZER: Cell<Option<(usize, u32, InitializerKind)>> = const { Cell::new(None) };
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InitializerKind {
    Import,
    Execution,
}

struct InitializerContext(Option<(usize, u32, InitializerKind)>);

impl InitializerContext {
    fn enter(py: &PyToken<'_>, id: u32, kind: InitializerKind) -> Self {
        let runtime = runtime_state(py) as *const _ as usize;
        Self(CURRENT_INITIALIZER.with(|current| current.replace(Some((runtime, id, kind)))))
    }
}

impl Drop for InitializerContext {
    fn drop(&mut self) {
        CURRENT_INITIALIZER.with(|current| current.set(self.0));
    }
}

/// Cold body context identifies private publication. Ordinary cache lookup
/// never consults it, so imports performed by the body remain public.
pub(crate) fn executing_private_body(py: &PyToken<'_>, name: &str) -> bool {
    CURRENT_INITIALIZER
        .with(Cell::get)
        .is_some_and(|(runtime, id, kind)| {
            kind == InitializerKind::Execution
                && runtime == runtime_state(py) as *const _ as usize
                && module_registry().is_some_and(|registry| registry.name_of(id) == name)
        })
}

/// Explicit loader/runpy entry to the same typed initializer dispatch. It
/// owns no global import row or name lock; ExecutionContext retains its module.
pub(crate) fn execute_private_body(py: &PyToken<'_>, name: &str) -> Result<(), u64> {
    if exception_pending(py) {
        return Err(none_bits());
    }
    let Some(registry) = module_registry() else {
        return Ok(());
    };
    let Some(id) = registry.id_of(name) else {
        return Ok(());
    };
    let row = registry.row(id);
    if module_table(py)
        .and_then(|table| table.initialization.get())
        .is_some_and(|initialization| {
            initialization.states[id as usize].load(Ordering::Acquire) == STATE_RETIRED
        })
    {
        return Err(raise_exception::<_>(
            py,
            "ImportError",
            "module namespace has been retired during runtime shutdown",
        ));
    }
    if row.module_flags & MODULE_FLAG_HAS_BODY == 0 {
        return Ok(());
    }
    let Some(_depth) = enter_import_recursion(py) else {
        return Err(none_bits());
    };
    let _context = InitializerContext::enter(py, id, InitializerKind::Execution);
    dispatch_module_body(py, registry, id, row.init_ptr);
    if exception_pending(py) {
        Err(none_bits())
    } else {
        Ok(())
    }
}

/// Fresh registered Python spec loads join the same ordinary row/namespace
/// owner as compiled import. Direct exec_module never creates this scope.
pub(crate) struct RegisteredSpecImport<'a, 'py> {
    py: &'a PyToken<'py>,
    table: &'static ModuleTable,
    id: u32,
    outer: Option<ModuleExecutionSnapshot>,
    _context: InitializerContext,
    _depth: ImportDepthGuard,
    finished: bool,
}

impl<'a, 'py> RegisteredSpecImport<'a, 'py> {
    pub(crate) fn begin(py: &'a PyToken<'py>, name: &str) -> Result<Option<Self>, u64> {
        let Some(id) = module_id_of(name) else {
            return Ok(None);
        };
        let Some(table) = module_table(py) else {
            return Err(none_bits());
        };
        let Some(depth) = enter_import_recursion(py) else {
            return Err(none_bits());
        };
        let index = id as usize;
        let owner = crate::concurrency::current_thread_id();
        let outer = loop {
            let state = table.initialization().states[index].load(Ordering::Acquire);
            if state == STATE_RETIRED {
                return Err(raise_exception::<_>(
                    py,
                    "ImportError",
                    "module namespace has been retired during runtime shutdown",
                ));
            }
            if state == STATE_INITIALIZING {
                // The caller already holds the all-name execution lease, so
                // a live outer initializer here necessarily belongs to it.
                debug_assert_eq!(initializer_owner(table, index), owner);
                break Some(detach_initializer(table, id));
            }
            if table.initialization().states[index]
                .compare_exchange(
                    state,
                    STATE_INITIALIZING,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                break None;
            }
        };
        table.initialization().specs[index].store(none_bits(), Ordering::Release);
        Ok(Some(Self {
            py,
            table,
            id,
            outer,
            _context: InitializerContext::enter(py, id, InitializerKind::Import),
            _depth: depth,
            finished: false,
        }))
    }

    pub(crate) fn admit(&self, name: &str, module: u64, spec: u64) {
        inc_ref_bits(self.py, spec);
        self.table.initialization().specs[self.id as usize].store(spec, Ordering::Release);
        publish_initializing_namespace(self.py, name, module);
    }

    pub(crate) fn finish(mut self) {
        finish_import_state(self.py, self.table, self.id);
        molt_cpython_abi::api::errors::with_preserved_error(|| {
            restore_module_execution(self.py, self.outer.take())
        });
        self.finished = true;
    }
}

impl Drop for RegisteredSpecImport<'_, '_> {
    fn drop(&mut self) {
        if !self.finished {
            // Admission/True-setter failure has no captured spec; no False
            // setter is invented. Always release the row and any outer escrow.
            finish_import_state(self.py, self.table, self.id);
            molt_cpython_abi::api::errors::with_preserved_error(|| {
                restore_module_execution(self.py, self.outer.take());
            });
        }
    }
}

pub(crate) fn set_spec_initializing(py: &PyToken<'_>, spec: u64, initializing: bool) {
    let Some(key) = crate::attr_name_bits_from_bytes(py, b"_initializing") else {
        return;
    };
    let _ = crate::builtins::attributes::mutate_attr_name(
        py,
        spec,
        key,
        crate::builtins::attr::DescriptorMutation::Set(MoltObject::from_bool(initializing).bits()),
        molt_cpython_abi::hooks::AttributeMutation::Normal,
    );
    dec_ref_bits(py, key);
}

/// The compiler explicitly supplies the actual spec SSA value. Ordinary
/// setattr cannot acquire this transaction role. Metadata assignment and its
/// existing explicit-loader overrides remain owned by module_set_attr.
#[unsafe(no_mangle)]
pub extern "C" fn molt_module_initialization_spec(module: u64, spec: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if exception_pending(py) {
            return none_bits();
        }
        let context = CURRENT_INITIALIZER
            .with(Cell::get)
            .filter(|(runtime, _, kind)| {
                *runtime == runtime_state(py) as *const _ as usize
                    && *kind == InitializerKind::Import
            });
        let ordinary = context.map(|(_, id, _)| id);
        let table = ordinary.and_then(|_| module_table(py));
        if let (Some(id), Some(table)) = (ordinary, table) {
            let index = id as usize;
            if table.initialization().states[index].load(Ordering::Acquire) != STATE_INITIALIZING
                || initializer_owner(table, index) != crate::concurrency::current_thread_id()
                || table.initialization().slots[index].load(Ordering::Acquire) != module
            {
                return raise_exception::<_>(
                    py,
                    "SystemError",
                    "compiler spec has no matching initializer namespace",
                );
            }
            if table.initialization().specs[index].load(Ordering::Acquire) != 0 {
                return raise_exception::<_>(
                    py,
                    "RuntimeError",
                    "initializer spec was already admitted",
                );
            }
            // An admission marker prevents callback reentry from capturing a
            // different spec. The exact namespace still owns bootstrap waiting
            // until metadata admission finishes; the marker retains no object.
            table.initialization().specs[index].store(none_bits(), Ordering::Release);
        }
        let spec = crate::builtins::exceptions::ExceptionValue::pin(py, spec);
        let Some(key) = crate::attr_name_bits_from_bytes(py, b"__spec__") else {
            return none_bits();
        };
        crate::builtins::modules::molt_module_set_attr(module, key, spec.bits());
        dec_ref_bits(py, key);
        if let Some(id) = ordinary
            && !exception_pending(py)
        {
            set_spec_initializing(py, spec.bits(), true);
            if !exception_pending(py) {
                table.unwrap().initialization().specs[id as usize]
                    .store(spec.into_bits(), Ordering::Release);
            }
        }
        none_bits()
    })
}

fn finish_initializing_spec(py: &PyToken<'_>, table: &'static ModuleTable, id: u32) {
    let spec = table.initialization().specs[id as usize].load(Ordering::Acquire);
    if spec == 0 || is_none_bits(spec) {
        return;
    }
    finish_spec_initializing(py, spec);
}

/// The original spec, independently of module.__spec__ replacement, owns the
/// finally setter. Pin it across callbacks and select its error through the
/// shared exception cleanup authority.
pub(crate) fn finish_spec_initializing(py: &PyToken<'_>, spec: u64) {
    let spec = crate::builtins::exceptions::ExceptionValue::pin(py, spec);
    let saved = exception_pending(py).then(|| crate::builtins::exceptions::take_raised(py));
    set_spec_initializing(py, spec.bits(), false);
    crate::builtins::exceptions::finish_exception_cleanup(
        py,
        saved,
        crate::builtins::exceptions::ExceptionCleanupPolicy::Chain,
    );
}

/// Read the original spec at each observable publication-name cutpoint. A
/// spec's mutable name never selects its initializer or synchronization owner.
pub(crate) fn spec_name<'a, 'py>(
    py: &'a PyToken<'py>,
    spec: u64,
) -> Result<crate::builtins::exceptions::ExceptionValue<'a, 'py>, u64> {
    let key = crate::state::cache::intern_runtime_static_name(py, b"name");
    if key == 0 {
        return Err(none_bits());
    }
    let bits =
        crate::builtins::attributes::get_attr_name(py, spec, key).map_err(|()| none_bits())?;
    Ok(crate::builtins::exceptions::ExceptionValue::adopt(py, bits))
}

/// Compiler cleanup carries the immutable catalog name; only its active
/// ordinary initializer may translate that into the retained spec's live name.
/// Explicit dictionary deletion and extension rollback keep their supplied key.
pub(crate) fn ordinary_failure_name<'a, 'py>(
    py: &'a PyToken<'py>,
    name: &str,
) -> Result<Option<crate::builtins::exceptions::ExceptionValue<'a, 'py>>, u64> {
    let context = CURRENT_INITIALIZER
        .with(Cell::get)
        .filter(|(runtime, id, kind)| {
            *runtime == runtime_state(py) as *const _ as usize
                && *kind == InitializerKind::Import
                && module_registry().is_some_and(|registry| registry.name_of(*id) == name)
        });
    let Some((_, id, _)) = context else {
        return Ok(None);
    };
    let Some(table) = module_table(py) else {
        return Ok(None);
    };
    let spec = table.initialization().specs[id as usize].load(Ordering::Acquire);
    if spec == 0 || is_none_bits(spec) {
        return Ok(None);
    }
    // Keep the original spec alive if its name getter reenters an initializer.
    let spec = crate::builtins::exceptions::ExceptionValue::pin(py, spec);
    spec_name(py, spec.bits()).map(Some)
}

#[derive(Clone, Copy)]
pub(crate) enum PublicationName {
    Key(u64),
    Spec(u64),
}
impl PublicationName {
    fn read<'a, 'py>(
        self,
        py: &'a PyToken<'py>,
    ) -> Result<crate::builtins::exceptions::ExceptionValue<'a, 'py>, u64> {
        match self {
            Self::Key(key) => Ok(crate::builtins::exceptions::ExceptionValue::pin(py, key)),
            Self::Spec(spec) => spec_name(py, spec),
        }
    }
}

/// One pop/reinsert completion, including two distinct live spec.name reads
/// required by _load_unlocked/_exec. Callers pin their original spec and module.
pub(crate) fn complete_publication(
    py: &PyToken<'_>,
    dictionary: u64,
    name: PublicationName,
) -> Result<u64, u64> {
    let _spec = match name {
        PublicationName::Spec(spec) => {
            Some(crate::builtins::exceptions::ExceptionValue::pin(py, spec))
        }
        PublicationName::Key(_) => None,
    };
    let pop_key = name.read(py)?;
    let final_bits = crate::object::ops_dict::molt_dict_pop(
        dictionary,
        pop_key.bits(),
        none_bits(),
        MoltObject::from_int(0).bits(),
    );
    if exception_pending(py) {
        return Err(none_bits());
    }
    let final_value = crate::builtins::exceptions::ExceptionValue::adopt(py, final_bits);
    let set_key = name.read(py)?;
    let retired = unsafe {
        crate::object::ops::dict_set_deferred(
            py,
            crate::ptr_from_bits(dictionary),
            set_key.bits(),
            final_value.bits(),
        )
    }
    .map_err(|()| none_bits())?;
    drop(retired);
    Ok(final_value.into_bits())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModuleImportPolicy {
    Python,
    CImport,
    CGetModule,
}

pub(crate) enum CachedModuleOutcome {
    Return(u64),
    Reload,
    Missing,
}

/// Consume one public observation, keeping it owned across every metadata
/// callback and the existing foreign-owner wait. Callers retain their actual
/// dictionary: Python importlib may use a rebound sys.modules attribute.
pub(crate) fn resolve_cached_module(
    py: &PyToken<'_>,
    name: &str,
    name_bits: Option<u64>,
    dictionary: u64,
    observed: u64,
    policy: ModuleImportPolicy,
) -> Result<CachedModuleOutcome, u64> {
    use crate::builtins::exceptions::ExceptionValue;
    let observed = ExceptionValue::adopt(py, observed);
    let none_blocker = || {
        let key = match name_bits {
            Some(bits) => ExceptionValue::pin(py, bits),
            None => {
                let Some(bits) = crate::attr_name_bits_from_bytes(py, name.as_bytes()) else {
                    return none_bits();
                };
                ExceptionValue::adopt(py, bits)
            }
        };
        crate::builtins::exceptions::raise_module_not_found(
            py,
            format!("import of {name} halted; None in sys.modules").as_bytes(),
            key.bits(),
        )
    };
    if is_none_bits(observed.bits()) {
        return if policy == ModuleImportPolicy::CGetModule {
            Ok(CachedModuleOutcome::Return(observed.into_bits()))
        } else {
            Err(none_blocker())
        };
    }
    let row = module_id_of(name).and_then(|id| module_table(py).map(|table| (id, table)));
    let bootstrap = row.is_some_and(|(id, table)| {
        let index = id as usize;
        let spec = table.initialization().specs[index].load(Ordering::Acquire);
        table.initialization().states[index].load(Ordering::Acquire) == STATE_INITIALIZING
            && table.initialization().slots[index].load(Ordering::Acquire) == observed.bits()
            && (spec == 0 || is_none_bits(spec))
    });
    let busy = bootstrap
        || crate::builtins::platform::public_module_is_initializing(py, observed.bits(), policy)?;
    if busy {
        if policy == ModuleImportPolicy::Python {
            // _find_and_load observes the public row while its first name lock
            // is held. A cycle at this admission propagates _DeadlockError;
            // only the subsequent _lock_unlock_module accepts a partial.
            let execution = ModuleExecutionGuard::acquire(py, name)
                .map_err(|failure| failure.into_exception(py, name))?;
            let key = match name_bits {
                Some(bits) => ExceptionValue::pin(py, bits),
                None => ExceptionValue::adopt(
                    py,
                    crate::attr_name_bits_from_bytes(py, name.as_bytes()).ok_or_else(none_bits)?,
                ),
            };
            let current = unsafe {
                crate::dict_get_in_place(py, crate::ptr_from_bits(dictionary), key.bits())
            }
            .map_err(|()| none_bits())?
            .map(|bits| ExceptionValue::pin(py, bits));
            drop(key);
            // Assignment to Python's module local releases the former public
            // observation under this first lock, before its second wait.
            drop(observed);
            drop(execution);
            let Some(current) = current else {
                return Ok(CachedModuleOutcome::Reload);
            };
            wait_for_module_execution(py, name)?;
            return if is_none_bits(current.bits()) {
                Err(none_blocker())
            } else {
                Ok(CachedModuleOutcome::Return(current.into_bits()))
            };
        }
        wait_for_module_execution(py, name)?;
    }
    // Latest primary 3.13.16 and later revalidate identity after the public
    // probe in import.c and _bootstrap._find_and_load. Older 3.13.12/3.14.3
    // retained false-path observations and are regression references only.
    let current_policy = crate::object::ops_sys::runtime_target_at_least(py, 3, 13);
    if current_policy {
        let allocated = if name_bits.is_none() {
            Some(ExceptionValue::adopt(
                py,
                crate::attr_name_bits_from_bytes(py, name.as_bytes()).ok_or_else(none_bits)?,
            ))
        } else {
            None
        };
        let key = name_bits.unwrap_or_else(|| allocated.as_ref().unwrap().bits());
        let current =
            unsafe { crate::dict_get_in_place(py, crate::ptr_from_bits(dictionary), key) }
                .map_err(|()| none_bits())?
                .map(|bits| ExceptionValue::pin(py, bits));
        if current.as_ref().map(ExceptionValue::bits) != Some(observed.bits()) {
            return Ok(if policy == ModuleImportPolicy::CGetModule {
                CachedModuleOutcome::Missing
            } else {
                CachedModuleOutcome::Reload
            });
        }
    }
    Ok(CachedModuleOutcome::Return(observed.into_bits()))
}

/// Whether this thread owns execution of this exact published module object.
/// Explicit execution has no global initialization row. Public membership or a
/// same-named standalone module cannot acquire ordinary initializer privileges.
pub(crate) fn module_execution_owns_initializing_namespace(
    _py: &PyToken<'_>,
    name: &str,
    module_bits: u64,
) -> bool {
    if module_bits == 0 || is_none_bits(module_bits) {
        return false;
    }
    initializing_namespace_bits(_py, name) == Some(module_bits)
}

fn initializing_namespace_bits(_py: &PyToken<'_>, name: &str) -> Option<u64> {
    let id = module_id_of(name)?;
    let table = module_table(_py)?;
    let idx = id as usize;
    if !matches!(
        table.initialization().states[idx].load(Ordering::Acquire),
        STATE_INITIALIZING
    ) || initializer_owner(table, idx) != crate::concurrency::current_thread_id()
    {
        return None;
    }
    let bits = table.initialization().slots[idx].load(Ordering::Acquire);
    (bits != 0 && !is_none_bits(bits)).then_some(bits)
}

/// The globals projection of the same execution-owned namespace. This grants
/// no privilege to another module with the same spelling or dictionary shape.
pub(crate) fn module_execution_owns_initializing_globals(
    _py: &PyToken<'_>,
    name: &str,
    globals_bits: u64,
) -> bool {
    initializing_namespace_bits(_py, name)
        .and_then(|bits| obj_from_bits(bits).as_ptr())
        .is_some_and(|ptr| unsafe {
            crate::object_type_id(ptr) == crate::TYPE_ID_MODULE
                && crate::module_dict_bits(ptr) == globals_bits
        })
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
    let Some(_depth) = enter_import_recursion(_py) else {
        if let Some(crate::builtins::modules::PublicModuleCache::Present(bits)) = observed {
            dec_ref_bits(_py, bits);
        }
        return none_bits();
    };
    module_ensure_inner(_py, registry, id, observed)
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
    if table.initialization().states[idx].load(Ordering::Acquire) == STATE_RETIRED {
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
    let mut execution = None;
    loop {
        // Normal compiled and dynamic imports observe the same dictionary.
        // Explicit loader/runpy execution uses its temporary namespace instead.
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
        match public {
            crate::builtins::modules::PublicModuleCache::Present(bits) => {
                let Some(dictionary) = module_dictionary(_py) else {
                    dec_ref_bits(_py, bits);
                    return none_bits();
                };
                return match resolve_cached_module(
                    _py,
                    registry.name_of(id),
                    None,
                    dictionary,
                    bits,
                    ModuleImportPolicy::CImport,
                ) {
                    Ok(CachedModuleOutcome::Return(bits)) => bits,
                    Ok(CachedModuleOutcome::Reload) => module_ensure(_py, id),
                    Ok(CachedModuleOutcome::Missing) => unreachable!("ordinary import may reload"),
                    Err(error) => error,
                };
            }
            crate::builtins::modules::PublicModuleCache::Missing => {
                let state = table.initialization().states[idx].load(Ordering::Acquire);
                if !parent_ready && state != STATE_INITIALIZING {
                    if !ensure_parent_ready(_py, &row) {
                        return none_bits();
                    }
                    parent_ready = true;
                    continue;
                }
                if execution.is_none() {
                    match ModuleExecutionGuard::acquire(_py, registry.name_of(id)) {
                        Ok(guard) => {
                            execution = Some(guard);
                            continue;
                        }
                        Err(failure) => {
                            return failure.into_exception(_py, registry.name_of(id));
                        }
                    }
                }
                // Completed initialization metadata cannot revive an absent
                // dictionary entry; re-enter the existing initializer admission.
                if matches!(state, STATE_READY | STATE_REPLACED) {
                    module_table_view_tombstone(_py, id);
                    if exception_pending(_py) {
                        return none_bits();
                    }
                    continue;
                }
                false
            }
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
        match table.initialization().states[idx].load(Ordering::Acquire) {
            STATE_READY | STATE_REPLACED => {
                module_table_view_tombstone(_py, id);
                continue;
            }
            STATE_INITIALIZING => {
                // Missing publication admits a fresh recursive ordinary
                // import under the already-held reentrant name lease.
                debug_assert_eq!(initializer_owner(table, idx), self_tid);
                let outer = detach_initializer(table, id);
                let result = run_init_transaction(_py, registry, table, id, row.init_ptr);
                restore_module_execution(_py, Some(outer));
                return result;
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
                if table.initialization().states[idx]
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
                if table.initialization().states[idx]
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
    // Namespace rows own no initializer. A bootstrap import of a compiled
    // child still creates its namespace parent through the public spec protocol.
    let registry = module_registry().expect("compiled parent registry");
    let parent_bits = if registry.row(parent).kind == MODULE_KIND_NAMESPACE_PARENT {
        let Some(name) = crate::attr_name_bits_from_bytes(_py, registry.name_of(parent).as_bytes())
        else {
            return false;
        };
        let bits = crate::builtins::modules::molt_module_import(name);
        dec_ref_bits(_py, name);
        bits
    } else {
        module_ensure(_py, parent)
    };
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
/// into `Initializing` while holding the canonical name execution lease.
fn trace_ensure() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_MODULE_ENSURE)
}

fn run_init_transaction(
    py: &PyToken<'_>,
    registry: &'static ModuleRegistry,
    table: &'static ModuleTable,
    id: u32,
    init_ptr: u64,
) -> u64 {
    let _context = InitializerContext::enter(py, id, InitializerKind::Import);
    let result = run_init_transaction_body(py, registry, table, id, init_ptr);
    finish_import_state(py, table, id);
    if exception_pending(py) {
        dec_ref_bits(py, result);
        return none_bits();
    }
    if let Err(error) = bind_parent_attr(py, registry, table, id, result) {
        dec_ref_bits(py, result);
        return error;
    }
    result
}

/// One terminal owner release for compiled and spec-driven ordinary loads.
fn finish_import_state(py: &PyToken<'_>, table: &'static ModuleTable, id: u32) {
    finish_initializing_spec(py, table, id);
    let index = id as usize;
    let _ = table.initialization().states[index].compare_exchange(
        STATE_INITIALIZING,
        if exception_pending(py) {
            STATE_UNINIT
        } else {
            STATE_READY
        },
        Ordering::AcqRel,
        Ordering::Acquire,
    );
    let spec = table.initialization().specs[index].swap(0, Ordering::AcqRel);
    if spec != 0 && !is_none_bits(spec) {
        molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, spec));
    }
    let namespace = table.initialization().slots[index].swap(0, Ordering::AcqRel);
    if namespace != 0 {
        molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, namespace));
    }
}

/// Sole native/WASM body invocation; callers own distinct publication roles.
fn dispatch_module_body(_py: &PyToken<'_>, _registry: &ModuleRegistry, id: u32, init_ptr: u64) {
    // Module bodies do not have the ordinary function-entry prologue. Their
    // explicit raises may clear only their own handlers, preserving the caller
    // that catches a failed recursive import and may then use bare raise.
    let _baseline = crate::call::ExceptionBaselineGuard::enter_at_current_depth(_py);
    let name = _registry.name_of(id);
    // Named loaders execute the same admitted provider body, including
    // SourceFileLoader aliases/alternate filesystem paths. The role is a
    // catalog fact; the fixed program-entry row is a distinct direct-source
    // capability rather than an advertised named stdlib provider.
    if _registry.row(id).module_flags & MODULE_FLAG_STDLIB_PROVIDER != 0
        && let Some(absence) = crate::builtins::platform::known_import_absence(_py, name)
    {
        crate::builtins::platform::raise_known_import_absence(_py, absence);
        return;
    }

    // MODULE_INIT_TABLE dispatch: MODULE_CACHE_SET publishes the dictionary
    // entry and retains a temporary initializer owner before the body runs.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(init_target) = crate::provenance::abi::function_ptr(init_ptr) else {
            return raise_exception::<()>(
                _py,
                "ImportError",
                &format!("module '{name}' initializer exceeds the active address space"),
            );
        };
        if init_target.is_null() {
            return raise_exception::<()>(
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
}

fn run_init_transaction_body(
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
        let previous = table.initialization().slots[idx].swap(0, Ordering::AcqRel);
        if previous != 0 {
            if let Some(key) = crate::attr_name_bits_from_bytes(_py, name.as_bytes()) {
                // This is the ordinary import-body failure boundary, matching
                // _load_unlocked's name deletion even after nested publication.
                // Extension construction rollback has separate exact custody.
                crate::builtins::modules::module_cache_remove(key, None);
                dec_ref_bits(_py, key);
            }
            dec_ref_bits(_py, previous);
        }
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
        let Some(key) = crate::attr_name_bits_from_bytes(_py, name.as_bytes()) else {
            return none_bits();
        };
        let error = crate::builtins::exceptions::raise_module_not_found(
            _py,
            format!("No module named '{name}'").as_bytes(),
            key,
        );
        dec_ref_bits(_py, key);
        return error;
    }

    dispatch_module_body(_py, registry, id, init_ptr);

    if exception_pending(_py) {
        unwind(_py);
        return none_bits();
    }

    let bits = table.initialization().slots[idx].load(Ordering::Acquire);
    if bits == 0 {
        unwind(_py);
        return raise_exception::<_>(
            _py,
            "ImportError",
            &format!("module '{name}' init completed without publishing a module object"),
        );
    }
    // Keep the initializer alive through final dictionary callbacks.
    let initializer = crate::builtins::exceptions::ExceptionValue::pin(_py, bits);
    let completed = (|| {
        let dictionary = module_dictionary(_py).ok_or_else(none_bits)?;
        let key = crate::attr_name_bits_from_bytes(_py, name.as_bytes()).ok_or_else(none_bits)?;
        let key = crate::builtins::exceptions::ExceptionValue::adopt(_py, key);
        let spec = table.initialization().specs[idx].load(Ordering::Acquire);
        let name = if spec == 0 || is_none_bits(spec) {
            PublicationName::Key(key.bits())
        } else {
            PublicationName::Spec(spec)
        };
        complete_publication(_py, dictionary, name)
    })();
    let published = table.initialization().slots[idx].swap(0, Ordering::AcqRel);
    if published != 0 {
        dec_ref_bits(_py, published);
    }
    drop(initializer);
    match completed {
        Ok(bits) | Err(bits) => bits,
    }
}

/// Fresh-load completion owns the one observable parent-attribute publication.
fn bind_parent_attr(
    _py: &PyToken<'_>,
    registry: &'static ModuleRegistry,
    _table: &'static ModuleTable,
    id: u32,
    bits: u64,
) -> Result<(), u64> {
    let row = registry.row(id);
    let Some(parent) = row.parent else {
        return Ok(());
    };
    let Some(parent_bits) = module_lookup(_py, registry.name_of(parent))? else {
        return Ok(());
    };
    let _parent_owner = obj_from_bits(parent_bits)
        .as_ptr()
        .map(crate::PtrDropGuard::new);
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
/// alias name is published in the one dictionary, with no completed slot owner.
fn publish_alias(_py: &PyToken<'_>, registry: &'static ModuleRegistry, id: u32, bits: u64) {
    let Some(table) = module_table(_py) else {
        return;
    };
    let idx = id as usize;
    if matches!(
        table.initialization().states[idx].load(Ordering::Acquire),
        STATE_READY | STATE_RETIRED
    ) {
        return;
    }
    let name = registry.name_of(id);
    let Some(dictionary) = module_dictionary(_py) else {
        return;
    };
    let Some(key) = crate::attr_name_bits_from_bytes(_py, name.as_bytes()) else {
        return;
    };
    let retired = unsafe {
        crate::object::ops::dict_set_deferred(_py, crate::ptr_from_bits(dictionary), key, bits)
    };
    dec_ref_bits(_py, key);
    if let Ok(retired) = retired {
        table.initialization().states[idx].store(STATE_READY, Ordering::Release);
        drop(retired);
    }
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
#[path = "module_table/catalog_tests.rs"]
mod catalog_tests;

#[cfg(test)]
#[path = "module_table/tests.rs"]
pub(crate) mod tests;
