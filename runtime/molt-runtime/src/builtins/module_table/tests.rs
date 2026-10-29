use super::*;
use crate::BuiltinExc;
use std::sync::atomic::AtomicU64 as TestCounter;

/// One synthetic registry row, in the Python layout writer's column order.
type BlobRow = (String, u64, Option<u32>, Option<u32>, u8, u8);

/// Mirror of the Python layout authority (`molt.cli.module_registry`),
/// used to build a synthetic registry blob for the state-machine tests.
/// Divergence from the real writer fails `ModuleRegistry::parse`.
pub(super) struct BlobBuilder {
    rows: Vec<BlobRow>,
}

impl BlobBuilder {
    pub(super) fn new() -> Self {
        Self { rows: Vec::new() }
    }

    pub(super) fn row(
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

    pub(super) fn build(&self) -> Vec<u8> {
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
        blob.extend_from_slice(&0u64.to_le_bytes());
        assert_eq!(blob.len(), MODULE_REGISTRY_HEADER_BYTES);
        for ((_, init_ptr, parent, alias, kind, flags), (off, len)) in self.rows.iter().zip(&spans)
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

fn publish_test_module(name: &str, supplied_spec: Option<u64>) -> u64 {
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
        use crate::builtins::exceptions::ExceptionValue;
        // Model one compiler metadata admission. A supplied private-body
        // spec must be selected here, before the one-shot runpy projection.
        let spec = match supplied_spec {
            Some(spec) => ExceptionValue::pin(_py, spec),
            None => ExceptionValue::adopt(
                _py,
                crate::builtins::types::alloc_module_spec(
                    _py,
                    name_bits,
                    none_bits(),
                    none_bits(),
                    MoltObject::from_bool(false).bits(),
                )
                .expect("fixture uses actual runtime ModuleSpec producer"),
            ),
        };
        molt_module_initialization_spec(module_bits, spec.bits());
        assert!(!exception_pending(_py));
        drop(spec);
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
        let module = if let Some(module) = crate::builtins::modules::interpreter_sys_module(py) {
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

static PRIVATE_BODY_SPEC: AtomicU64 = AtomicU64::new(0);
struct PrivateBodySpecScope;
impl Drop for PrivateBodySpecScope {
    fn drop(&mut self) {
        PRIVATE_BODY_SPEC.store(0, Ordering::SeqCst);
    }
}

static PRIVATE_SELF_IMPORT: AtomicU64 = AtomicU64::new(0);
static PRIVATE_SELF_RESULT: AtomicU64 = AtomicU64::new(0);

struct PrivateSelfScope<'a, 'py>(&'a PyToken<'py>);
impl Drop for PrivateSelfScope<'_, '_> {
    fn drop(&mut self) {
        PRIVATE_SELF_IMPORT.store(0, Ordering::SeqCst);
        let bits = PRIVATE_SELF_RESULT.swap(0, Ordering::SeqCst);
        if bits != 0 {
            dec_ref_bits(self.0, bits);
        }
    }
}

extern "C" fn init_g4_src() -> u64 {
    INIT_RUNS.fetch_add(1, Ordering::SeqCst);
    crate::with_gil_entry_nopanic!(_py, {
        let spec = PRIVATE_BODY_SPEC.load(Ordering::SeqCst);
        let supplied_spec = (spec != 0 && executing_private_body(_py, "g4_src")).then_some(spec);
        let bits = publish_test_module("g4_src", supplied_spec);
        if PRIVATE_SELF_IMPORT.load(Ordering::SeqCst) != 0 && executing_private_body(_py, "g4_src")
        {
            let imported = module_ensure(_py, test_registry_id("g4_src"));
            if !exception_pending(_py) {
                PRIVATE_SELF_RESULT.store(imported, Ordering::SeqCst);
            }
        }
        dec_ref_bits(_py, bits);
    });
    0
}

extern "C" fn init_g4_fail() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(_py, BuiltinExc::ValueError, "g4 init failure")
    })
}

extern "C" fn init_g4_z_static_ext_fail() -> u64 {
    EXT_FAIL_RUNS.fetch_add(1, Ordering::SeqCst);
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(
            _py,
            BuiltinExc::ImportError,
            "g4_z_static_ext_fail: static-link PyModuleDef Py_mod_exec slot \
                 returned non-zero",
        )
    })
}

extern "C" fn init_g4_pkg() -> u64 {
    let bits = publish_test_module("g4_pkg", None);
    crate::with_gil_entry_nopanic!(_py, {
        let replacement = PARENT_CHILD_OVERRIDE.load(Ordering::Relaxed);
        if replacement != 0 {
            let sys = publication_for_test(_py, "sys").unwrap();
            let modules_bits = crate::builtins::modules::sys_modules_dict_bits(_py, sys).unwrap();
            let modules = obj_from_bits(modules_bits).as_ptr().unwrap();
            let _modules_owner = crate::PtrDropGuard::new(modules);
            let child = crate::attr_name_bits_from_bytes(_py, b"g4_pkg.sub").unwrap();
            unsafe { assert!(crate::dict_set_in_place(_py, modules, child, replacement)) };
            dec_ref_bits(_py, child);
        }
        dec_ref_bits(_py, bits);
    });
    0
}

extern "C" fn init_g4_pkg_sub() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        // Parent-first must have completed g4_pkg before this body runs.
        assert!(publication_for_test(_py, "g4_pkg").is_some());
    });
    let bits = publish_test_module("g4_pkg.sub", None);
    crate::with_gil_entry_nopanic!(_py, {
        dec_ref_bits(_py, bits);
    });
    0
}

extern "C" fn init_g4_target() -> u64 {
    let bits = publish_test_module("g4_target", None);
    crate::with_gil_entry_nopanic!(_py, {
        dec_ref_bits(_py, bits);
    });
    0
}

extern "C" fn init_g4_tomb() -> u64 {
    let bits = publish_test_module("g4_tomb", None);
    crate::with_gil_entry_nopanic!(_py, {
        dec_ref_bits(_py, bits);
    });
    0
}

extern "C" fn init_g4_tomb_ext() -> u64 {
    let bits = publish_test_module("g4_tomb_ext", None);
    crate::with_gil_entry_nopanic!(_py, {
        dec_ref_bits(_py, bits);
    });
    0
}

extern "C" fn init_g4_cycle_a() -> u64 {
    let bits = publish_test_module("g4_cycle_a", None);
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
    let bits = publish_test_module("g4_cycle_b", None);
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

struct TestExecutionOwner {
    table: &'static ModuleTable,
    record: Arc<ModuleExecutionLock>,
    owner: u64,
    depth: usize,
}
impl TestExecutionOwner {
    fn set(table: &'static ModuleTable, id: usize, owner: u64) -> Self {
        let name = module_registry().unwrap().name_of(id as u32);
        let mut locks = table
            .execution
            .get_or_init(Default::default)
            .lock()
            .unwrap();
        let record = Arc::clone(locks.active.entry(Arc::from(name)).or_insert_with(|| {
            Arc::new(ModuleExecutionLock {
                name: Arc::from(name),
                owner: AtomicU64::new(0),
                depth: AtomicUsize::new(0),
                waiters: AtomicUsize::new(0),
                wake: Condvar::new(),
            })
        }));
        let previous = record.owner.swap(owner, Ordering::AcqRel);
        let depth = record.depth.swap(usize::from(owner != 0), Ordering::AcqRel);
        Self {
            table,
            record,
            owner: previous,
            depth,
        }
    }
}
impl Drop for TestExecutionOwner {
    fn drop(&mut self) {
        let mut locks = self.table.execution.get().unwrap().lock().unwrap();
        self.record.depth.store(self.depth, Ordering::Release);
        self.record.owner.store(self.owner, Ordering::Release);
        if self.owner == 0
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

// Test scopes borrow stable rows, retaining their displaced slot through
// the production execution snapshot. Synthetic initializer owners must not
// survive an assertion unwind and strand another test in the import wait.
pub(crate) struct ModuleTestRestore<'a, 'py> {
    py: &'a PyToken<'py>,
    snapshot: Option<ModuleExecutionSnapshot>,
    cache: Option<(u64, Option<u64>)>,
    execution: Option<ModuleExecutionGuard>,
}

impl<'a, 'py> ModuleTestRestore<'a, 'py> {
    fn capture(py: &'a PyToken<'py>, name: &str) -> Self {
        let id = test_registry_id(name);
        let table = module_table(py).expect("test module table");
        let index = id as usize;
        let owner = initializer_owner(table, index);
        assert_eq!(owner, 0);
        let state = table.initialization().states[index].load(Ordering::Acquire);
        assert!(!matches!(state, STATE_INITIALIZING));
        let name_bits = crate::attr_name_bits_from_bytes(py, name.as_bytes()).unwrap();
        let previous = publication_for_test(py, name);
        if let Some(bits) = previous {
            inc_ref_bits(py, bits);
        }
        let bits = table.initialization().slots[index].load(Ordering::Acquire);
        if bits != 0 {
            inc_ref_bits(py, bits);
        }
        let spec = table.initialization().specs[index].load(Ordering::Acquire);
        if spec != 0 && !is_none_bits(spec) {
            inc_ref_bits(py, spec);
        }
        Self {
            py,
            snapshot: Some(ModuleExecutionSnapshot {
                id,
                state,
                bits,
                spec,
            }),
            cache: Some((name_bits, previous)),
            execution: None,
        }
    }

    fn execution(py: &'a PyToken<'py>, snapshot: ModuleExecutionSnapshot) -> Self {
        Self {
            py,
            snapshot: Some(snapshot),
            cache: None,
            execution: None,
        }
    }

    pub(crate) fn fresh(py: &'a PyToken<'py>, name: &str) -> (Self, u64) {
        let scope = Self::capture(py, name);
        remove_for_test(py, name);
        let id = test_registry_id(name);
        module_table_view_tombstone(py, id);
        let module = module_ensure(py, id);
        assert!(!exception_pending(py));
        (scope, module)
    }

    pub(crate) fn initializing(py: &'a PyToken<'py>, name: &str, module: u64) -> Self {
        let id = test_registry_id(name);
        let table = module_table(py).unwrap();
        let mut scope = Self::execution(py, detach_initializer(table, id));
        scope.execution = Some(ModuleExecutionGuard::acquire(py, name).expect("initializer lease"));
        table.initialization().states[id as usize].store(STATE_INITIALIZING, Ordering::Release);
        publish_initializing_namespace(py, name, module);
        assert!(module_execution_owns_initializing_namespace(
            py, name, module
        ));
        scope
    }

    fn restore(mut self) {
        self.restore_inner();
    }

    fn release_execution(&mut self) {
        drop(self.execution.take());
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
                if snapshot.spec != 0 && !is_none_bits(snapshot.spec) {
                    inc_ref_bits(self.py, snapshot.spec);
                }
                restore_module_execution(
                    self.py,
                    Some(ModuleExecutionSnapshot {
                        id: snapshot.id,
                        state: snapshot.state,
                        bits: snapshot.bits,
                        spec: snapshot.spec,
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
            // Publication callbacks can change transaction metadata.
            // Restore the exact prior slot/state, consuming its saved owner.
            restore_module_execution(self.py, Some(snapshot));
        });
        self.release_execution();
    }
}

impl Drop for ModuleTestRestore<'_, '_> {
    fn drop(&mut self) {
        self.restore_inner();
    }
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn raw_module_body_dispatch_preserves_caller_exception_baseline_and_error() {
    extern "C" fn changes_baseline_and_raises() -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            crate::exception_stack_baseline_set(0);
            let exception = crate::builtins::exceptions::ExceptionValue::adopt(
                py,
                MoltObject::from_ptr(crate::alloc_exception(
                    py,
                    BuiltinExc::ValueError,
                    "body error",
                ))
                .bits(),
            );
            crate::builtins::exceptions::molt_raise(exception.bits())
        })
    }
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    install_test_registry();
    crate::with_gil_entry_nopanic!(py, {
        let _baseline = crate::call::ExceptionBaselineGuard::new();
        crate::exception_stack_baseline_set(3);
        dispatch_module_body(
            py,
            module_registry().unwrap(),
            test_registry_id("g4_src"),
            changes_baseline_and_raises as *const () as usize as u64,
        );
        assert_eq!(crate::exception_stack_baseline_get(), 3);
        assert!(exception_pending(py));
        let exception = crate::builtins::exceptions::ExceptionValue::adopt(
            py,
            crate::molt_exception_last_pending(),
        );
        assert!(crate::builtins::exceptions::exception_matches_kind(
            py,
            exception.bits(),
            crate::BuiltinExc::ValueError,
        ));
        assert_eq!(
            crate::builtins::exceptions::format_exception_message(
                py,
                obj_from_bits(exception.bits()).as_ptr().unwrap(),
            ),
            "body error"
        );
        crate::clear_exception(py);
    });
}

#[test]
fn execution_locks_share_reentrancy_and_retire_registered_and_arbitrary_names() {
    eprintln!(
        "module execution layout bytes: table={} manager={} record={}",
        std::mem::size_of::<ModuleTable>(),
        std::mem::size_of::<ModuleExecutionLocks>(),
        std::mem::size_of::<ModuleExecutionLock>()
    );
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    install_test_registry();
    crate::with_gil_entry_nopanic!(py, {
        for name in ["g4_src", "_molt_arbitrary_execution_lock"] {
            let outer = ModuleExecutionGuard::acquire(py, name).unwrap();
            let table = outer.table;
            let inner = ModuleExecutionGuard::acquire(py, name).unwrap();
            assert!(Arc::ptr_eq(&outer.record, &inner.record));
            assert_eq!(outer.record.depth.load(Ordering::Relaxed), 2);
            drop(inner);
            assert_eq!(outer.record.depth.load(Ordering::Relaxed), 1);
            // An exhausted recursive acquisition must neither wrap nor
            // publish a second owner, and its abandoned admission retires.
            outer.record.depth.store(usize::MAX, Ordering::Relaxed);
            assert!(matches!(
                ModuleExecutionGuard::acquire(py, name),
                Err(ModuleLockFailure::Exception)
            ));
            assert!(exception_pending(py));
            crate::clear_exception(py);
            assert_eq!(outer.record.depth.load(Ordering::Relaxed), usize::MAX);
            outer.record.depth.store(1, Ordering::Relaxed);
            drop(outer);
            let locks = table.execution.get().unwrap().lock().unwrap();
            assert!(
                !locks.active.contains_key(name),
                "completed names leave no synchronization record"
            );
            assert!(locks.waiting.is_empty());
        }
    });
}

#[test]
fn completed_public_false_probe_bypasses_execution_lock_admission() {
    let transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        transaction.with_target_python_minor(py, 12, || {
            let name = crate::attr_name_bits_from_bytes(py, b"_molt_completed_no_lock").unwrap();
            let module = MoltObject::from_ptr(crate::alloc_module_obj(py, name)).bits();
            let spec = unsafe {
                crate::call_callable0(py, crate::builtins::types::simplenamespace_class(py))
            };
            let key = crate::attr_name_bits_from_bytes(py, b"__spec__").unwrap();
            crate::builtins::modules::molt_module_set_attr(module, key, spec);
            set_spec_initializing(py, spec, false);
            let dictionary = module_dictionary(py).unwrap();
            let table = runtime_state(py).module_table.get().unwrap();
            let manager = table.execution.get_or_init(Default::default);
            manager.lock().unwrap().retired = true;
            inc_ref_bits(py, module);
            let completed = resolve_cached_module(
                py,
                "_molt_completed_no_lock",
                Some(name),
                dictionary,
                module,
                ModuleImportPolicy::Python,
            );
            // A True probe supplies the negative control: the same retired
            // authority is observed and raises, rather than bypassed globally.
            set_spec_initializing(py, spec, true);
            inc_ref_bits(py, module);
            let initializing = resolve_cached_module(
                py,
                "_molt_completed_no_lock",
                Some(name),
                dictionary,
                module,
                ModuleImportPolicy::Python,
            );
            let failed = exception_pending(py);
            crate::clear_exception(py);
            manager.lock().unwrap().retired = false;
            assert!(matches!(completed, Ok(CachedModuleOutcome::Return(value)) if value == module));
            if let Ok(CachedModuleOutcome::Return(value)) = completed {
                dec_ref_bits(py, value);
            }
            assert!(initializing.is_err() && failed);
            assert!(
                !manager
                    .lock()
                    .unwrap()
                    .active
                    .contains_key("_molt_completed_no_lock")
            );
            for bits in [name, module, spec, key] {
                dec_ref_bits(py, bits);
            }
        });
    });
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn arbitrary_execution_locks_detect_cycles_and_retire_owner_and_waiter_admissions() {
    use std::sync::mpsc;
    use std::time::Duration;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let _ = module_dictionary(py).unwrap();
        let table = runtime_state(py).module_table.get().unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let mut workers = Vec::new();
        let mut starts = Vec::new();
        for (own, other) in [
            ("_molt_lock_cycle_left", "_molt_lock_cycle_right"),
            ("_molt_lock_cycle_right", "_molt_lock_cycle_left"),
        ] {
            let ready = ready_tx.clone();
            let done = done_tx.clone();
            let (start_tx, start_rx) = mpsc::channel();
            starts.push(start_tx);
            workers.push(std::thread::spawn(move || {
                crate::with_gil_entry_nopanic!(worker_py, {
                    let owner = ModuleExecutionGuard::acquire(worker_py, own).unwrap();
                    ready.send(()).unwrap();
                    let started = {
                        let _release = crate::concurrency::GilReleaseGuard::suspend();
                        start_rx.recv_timeout(Duration::from_secs(2)).is_ok()
                    };
                    let result = if started {
                        match ModuleExecutionGuard::acquire(worker_py, other) {
                            Ok(guard) => {
                                drop(guard);
                                0
                            }
                            Err(ModuleLockFailure::Cycle) => 1,
                            Err(ModuleLockFailure::Exception) => {
                                crate::clear_exception(worker_py);
                                2
                            }
                        }
                    } else {
                        3
                    };
                    drop(owner);
                    done.send(result).unwrap();
                });
            }));
        }
        let (ready, results);
        {
            let _release = crate::concurrency::GilReleaseGuard::suspend();
            ready = ready_rx.recv_timeout(Duration::from_secs(2)).is_ok()
                && ready_rx.recv_timeout(Duration::from_secs(2)).is_ok();
            for start in starts {
                let _ = start.send(());
            }
            results = [
                done_rx.recv_timeout(Duration::from_secs(2)),
                done_rx.recv_timeout(Duration::from_secs(2)),
            ];
            if results.iter().any(Result::is_err) {
                // Synchronization-only cancellation unblocks a defective
                // waiter before assertion; no Python edge is touched here.
                let mut locks = table.execution.get().unwrap().lock().unwrap();
                locks.retired = true;
                for record in locks.active.values() {
                    record.wake.notify_all();
                }
            }
            for worker in workers {
                worker.join().expect("execution-lock worker terminated");
            }
        }
        let mut locks = table.execution.get().unwrap().lock().unwrap();
        locks.retired = false;
        assert!(
            ready,
            "both distinct names were owned before cross-admission"
        );
        let mut outcomes = results.map(|result| result.expect("bounded cycle completion"));
        outcomes.sort();
        assert_eq!(
            outcomes,
            [0, 1],
            "one cycle rejection releases its owner for the other waiter"
        );
        assert!(!locks.active.contains_key("_molt_lock_cycle_left"));
        assert!(!locks.active.contains_key("_molt_lock_cycle_right"));
        assert!(
            locks.waiting.is_empty(),
            "rejected admissions leave no wait edge or map-only record"
        );
    });
}

static BUSY_OBSERVATION_TARGET: AtomicU64 = AtomicU64::new(0);
static BUSY_OBSERVATION_KEY: AtomicU64 = AtomicU64::new(0);
static BUSY_OBSERVATION_FINALIZED: AtomicU64 = AtomicU64::new(0);
static BUSY_OBSERVATION_OWNED: AtomicU64 = AtomicU64::new(0);

struct BusyObservationScope;
impl Drop for BusyObservationScope {
    fn drop(&mut self) {
        BUSY_OBSERVATION_TARGET.store(0, Ordering::SeqCst);
        BUSY_OBSERVATION_KEY.store(0, Ordering::SeqCst);
    }
}

extern "C" fn replace_busy_observation(_self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let dictionary = module_dictionary(py).unwrap();
        assert!(unsafe {
            crate::dict_set_in_place(
                py,
                crate::ptr_from_bits(dictionary),
                BUSY_OBSERVATION_KEY.load(Ordering::SeqCst),
                MoltObject::from_int(42).bits(),
            )
        });
        MoltObject::from_bool(true).bits()
    })
}

extern "C" fn finalize_busy_observation(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if self_bits == BUSY_OBSERVATION_TARGET.load(Ordering::SeqCst) {
            BUSY_OBSERVATION_FINALIZED.fetch_add(1, Ordering::SeqCst);
            let table = runtime_state(py).module_table.get().unwrap();
            BUSY_OBSERVATION_OWNED.store(
                u64::from(
                    execution_owner(table, "_molt_busy_observation")
                        == crate::concurrency::current_thread_id(),
                ),
                Ordering::SeqCst,
            );
        }
        none_bits()
    })
}

#[test]
fn python_busy_reobservation_retires_old_value_under_first_execution_lock() {
    use crate::builtins::exceptions::ExceptionValue;
    use crate::builtins::functions::{alloc_runtime_function_obj, runtime_fn_addr};
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let _scope = BusyObservationScope;
        let name = ExceptionValue::adopt(
            py,
            crate::attr_name_bits_from_bytes(py, b"_molt_busy_observation").unwrap(),
        );
        let class = ExceptionValue::adopt(py, crate::molt_class_new(name.bits()));
        crate::molt_class_set_base(class.bits(), crate::builtin_classes(py).object);
        for (method, symbol, target) in [
            (
                b"__bool__".as_slice(),
                "replace_busy_observation",
                replace_busy_observation as *const (),
            ),
            (
                b"__del__".as_slice(),
                "finalize_busy_observation",
                finalize_busy_observation as *const (),
            ),
        ] {
            let key =
                ExceptionValue::adopt(py, crate::attr_name_bits_from_bytes(py, method).unwrap());
            let function = ExceptionValue::adopt(
                py,
                MoltObject::from_ptr(alloc_runtime_function_obj(
                    py,
                    runtime_fn_addr(symbol, target),
                    1,
                ))
                .bits(),
            );
            crate::molt_set_attr_name(class.bits(), key.bits(), function.bits());
        }
        let pointer = obj_from_bits(class.bits()).as_ptr().unwrap();
        unsafe {
            crate::object::class_finish_definition(py, pointer).unwrap();
        }
        let size = unsafe { crate::object::layout::class_cached_layout_size(pointer).unwrap() };
        let instance = || {
            let value = crate::object::builders::alloc_class_instance(py, size, class.bits());
            unsafe {
                crate::object::gc::gc_publish_initialized(
                    py,
                    obj_from_bits(value).as_ptr().unwrap(),
                );
            }
            ExceptionValue::adopt(py, value)
        };
        let original = instance();
        let flag = instance();
        let spec = ExceptionValue::adopt(py, unsafe {
            crate::call_callable0(py, crate::builtins::types::simplenamespace_class(py))
        });
        for (receiver, spelling, value) in [
            (spec.bits(), b"_initializing".as_slice(), flag.bits()),
            (original.bits(), b"__spec__".as_slice(), spec.bits()),
        ] {
            let key =
                ExceptionValue::adopt(py, crate::attr_name_bits_from_bytes(py, spelling).unwrap());
            crate::molt_set_attr_name(receiver, key.bits(), value);
        }
        let dictionary = module_dictionary(py).unwrap();
        assert!(unsafe {
            crate::dict_set_in_place(
                py,
                crate::ptr_from_bits(dictionary),
                name.bits(),
                original.bits(),
            )
        });
        BUSY_OBSERVATION_TARGET.store(original.bits(), Ordering::SeqCst);
        BUSY_OBSERVATION_KEY.store(name.bits(), Ordering::SeqCst);
        BUSY_OBSERVATION_FINALIZED.store(0, Ordering::SeqCst);
        BUSY_OBSERVATION_OWNED.store(0, Ordering::SeqCst);
        // The public dictionary and the consumed observation are the last
        // original owners. The flag callback retires the dictionary edge.
        let outcome = resolve_cached_module(
            py,
            "_molt_busy_observation",
            Some(name.bits()),
            dictionary,
            original.into_bits(),
            ModuleImportPolicy::Python,
        );
        assert!(
            matches!(outcome, Ok(CachedModuleOutcome::Return(value)) if value == MoltObject::from_int(42).bits())
        );
        assert_eq!(BUSY_OBSERVATION_FINALIZED.load(Ordering::SeqCst), 1);
        assert_eq!(BUSY_OBSERVATION_OWNED.load(Ordering::SeqCst), 1);
        assert!(
            unsafe { crate::dict_del_in_place(py, crate::ptr_from_bits(dictionary), name.bits()) }
                .unwrap()
        );
        assert!(!exception_pending(py));
    });
}

#[test]
fn deadlock_exception_payload_projects_one_internal_runtime_error_identity() {
    use crate::builtins::exceptions::ExceptionValue;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let _provider = crate::test_support::NativeProviderTestNamespace::new(py, "builtins");
        let class =
            crate::builtins::exceptions::exception_class_bits(py, BuiltinExc::DeadlockError);
        assert_ne!(class, 0);
        let base = crate::builtins::exceptions::exception_class_bits(py, BuiltinExc::RuntimeError);
        assert!(crate::issubclass_bits(class, base));
        let name = ExceptionValue::adopt(
            py,
            crate::attr_name_bits_from_bytes(py, b"_DeadlockError").unwrap(),
        );
        for _ in 0..2 {
            let payload = ExceptionValue::adopt(py, crate::molt_importlib_frozen_payload());
            assert!(!exception_pending(py));
            assert_eq!(
                unsafe {
                    crate::dict_get_in_place(py, crate::ptr_from_bits(payload.bits()), name.bits())
                }
                .unwrap(),
                Some(class)
            );
        }
        let key = ExceptionValue::adopt(
            py,
            crate::attr_name_bits_from_bytes(py, b"__module__").unwrap(),
        );
        let origin = ExceptionValue::adopt(
            py,
            crate::builtins::attributes::get_attr_name(py, class, key.bits()).unwrap(),
        );
        assert_eq!(
            crate::string_obj_to_owned(obj_from_bits(origin.bits())).as_deref(),
            Some("_frozen_importlib")
        );
        let builtins = interpreter_builtins_dictionary(py).unwrap();
        for internal in [
            crate::BuiltinExc::CancelledError,
            crate::BuiltinExc::UnsupportedOperation,
            crate::BuiltinExc::DeadlockError,
            crate::BuiltinExc::PickleError,
            crate::BuiltinExc::PicklingError,
            crate::BuiltinExc::UnpicklingError,
            crate::BuiltinExc::SocketGaiError,
            crate::BuiltinExc::SocketHError,
            crate::BuiltinExc::StatisticsError,
        ] {
            let name = ExceptionValue::adopt(
                py,
                crate::attr_name_bits_from_bytes(py, internal.name().as_bytes()).unwrap(),
            );
            let expected = crate::builtins::exceptions::exception_class_bits(py, internal);
            let lookup = ExceptionValue::adopt(py, crate::molt_builtin_class_lookup(name.bits()));
            assert_eq!(
                lookup.bits(),
                expected,
                "real stdlib class lookup remains admitted"
            );
            assert_eq!(
                unsafe {
                    crate::dict_get_in_place(py, crate::ptr_from_bits(builtins), name.bits())
                }
                .unwrap(),
                None,
                "{internal} has no builtins namespace binding"
            );
        }
        assert!(!exception_pending(py));
    });
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn cached_cycle_policy_propagates_python_and_accepts_c_entry_partials() {
    use crate::builtins::exceptions::ExceptionValue;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    let transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let dictionary = module_dictionary(py).unwrap();
        let name = ExceptionValue::adopt(
            py,
            crate::attr_name_bits_from_bytes(py, b"_molt_policy_cycle_left").unwrap(),
        );
        let module = ExceptionValue::adopt(
            py,
            MoltObject::from_ptr(crate::alloc_module_obj(py, name.bits())).bits(),
        );
        let spec = ExceptionValue::adopt(py, unsafe {
            crate::call_callable0(py, crate::builtins::types::simplenamespace_class(py))
        });
        let spec_key = ExceptionValue::adopt(
            py,
            crate::attr_name_bits_from_bytes(py, b"__spec__").unwrap(),
        );
        crate::builtins::modules::molt_module_set_attr(module.bits(), spec_key.bits(), spec.bits());
        set_spec_initializing(py, spec.bits(), true);
        assert!(!exception_pending(py));
        assert!(unsafe {
            crate::dict_set_in_place(
                py,
                crate::ptr_from_bits(dictionary),
                name.bits(),
                module.bits(),
            )
        });
        for minor in 12..=15 {
            transaction.with_target_python_minor(py, minor, || {
                for policy in [
                    ModuleImportPolicy::Python,
                    ModuleImportPolicy::CImport,
                    ModuleImportPolicy::CGetModule,
                ] {
                    let right =
                        ModuleExecutionGuard::acquire(py, "_molt_policy_cycle_right").unwrap();
                    let table = right.table;
                    let record = Arc::clone(&right.record);
                    let (ready_tx, ready_rx) = mpsc::channel();
                    let (done_tx, done_rx) = mpsc::channel();
                    let worker = std::thread::spawn(move || {
                        crate::with_gil_entry_nopanic!(worker_py, {
                            let owner =
                                ModuleExecutionGuard::acquire(worker_py, "_molt_policy_cycle_left")
                                    .unwrap();
                            ready_tx.send(()).unwrap();
                            let crossed = ModuleExecutionGuard::acquire(
                                worker_py,
                                "_molt_policy_cycle_right",
                            );
                            let succeeded = crossed.is_ok();
                            drop(crossed);
                            crate::clear_exception(worker_py);
                            drop(owner);
                            done_tx.send(succeeded).unwrap();
                        });
                    });
                    let parked;
                    {
                        let _release = crate::concurrency::GilReleaseGuard::suspend();
                        let ready = ready_rx.recv_timeout(Duration::from_secs(2)).is_ok();
                        let deadline = Instant::now() + Duration::from_secs(2);
                        while ready
                            && record.waiters.load(Ordering::Relaxed) == 0
                            && Instant::now() < deadline
                        {
                            std::thread::yield_now();
                        }
                        parked = ready && record.waiters.load(Ordering::Relaxed) == 1;
                    }
                    // This witness is not an admitted lock participant.
                    // End its spectator pin before the final guard can retire.
                    drop(record);
                    let mut valid = false;
                    if parked {
                        // The actual foreign waiter owns left and is parked
                        // on right. This thread owns right, so its left
                        // admission necessarily closes the real wait cycle.
                        inc_ref_bits(py, module.bits());
                        let outcome = resolve_cached_module(
                            py,
                            "_molt_policy_cycle_left",
                            Some(name.bits()),
                            dictionary,
                            module.bits(),
                            policy,
                        );
                        valid = match (policy, outcome) {
                            (ModuleImportPolicy::Python, Err(_)) => {
                                let pending = exception_pending(py);
                                let error = ExceptionValue::adopt(
                                    py,
                                    crate::builtins::exceptions::molt_exception_last_pending(),
                                );
                                let expected = crate::builtins::exceptions::exception_class_bits(
                                    py,
                                    BuiltinExc::DeadlockError,
                                );
                                pending
                                    && obj_from_bits(error.bits()).as_ptr().is_some_and(
                                        |pointer| unsafe {
                                            crate::object_class_bits(pointer) == expected
                                        },
                                    )
                            }
                            (
                                ModuleImportPolicy::CImport | ModuleImportPolicy::CGetModule,
                                Ok(CachedModuleOutcome::Return(value)),
                            ) => {
                                let same = value == module.bits() && !exception_pending(py);
                                dec_ref_bits(py, value);
                                same
                            }
                            (_, Ok(CachedModuleOutcome::Return(value))) => {
                                dec_ref_bits(py, value);
                                false
                            }
                            _ => false,
                        };
                        crate::clear_exception(py);
                    }
                    drop(right);
                    let done;
                    {
                        let _release = crate::concurrency::GilReleaseGuard::suspend();
                        done = done_rx.recv_timeout(Duration::from_secs(2));
                        if done.is_err() {
                            retire_execution_locks(table);
                        }
                        worker.join().expect("cycle policy worker terminated");
                    }
                    let mut locks = table.execution.get().unwrap().lock().unwrap();
                    locks.retired = false;
                    let clean = locks.active.is_empty() && locks.waiting.is_empty();
                    drop(locks);
                    assert!(parked, "real foreign wait edge before policy admission");
                    assert!(valid, "minor={minor} policy={policy:?}");
                    assert!(done.unwrap());
                    assert!(clean, "every cycle admission and waiter record retires");
                }
            });
        }
        assert!(
            unsafe { crate::dict_del_in_place(py, crate::ptr_from_bits(dictionary), name.bits()) }
                .unwrap()
        );
        assert!(!exception_pending(py));
    });
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn execution_lock_retirement_wakes_parked_arbitrary_name_waiter() {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let owner = ModuleExecutionGuard::acquire(py, "_molt_retiring_execution_lock").unwrap();
        let table = owner.table;
        let record = Arc::clone(&owner.record);
        let (done_tx, done_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            crate::with_gil_entry_nopanic!(worker_py, {
                let failed = matches!(
                    ModuleExecutionGuard::acquire(worker_py, "_molt_retiring_execution_lock"),
                    Err(ModuleLockFailure::Exception)
                );
                let pending = exception_pending(worker_py);
                crate::clear_exception(worker_py);
                done_tx.send((failed, pending)).unwrap();
            });
        });
        let (parked, completion);
        {
            let _release = crate::concurrency::GilReleaseGuard::suspend();
            let deadline = Instant::now() + Duration::from_secs(2);
            while record.waiters.load(Ordering::Relaxed) == 0 && Instant::now() < deadline {
                std::thread::yield_now();
            }
            parked = record.waiters.load(Ordering::Relaxed) == 1 && done_rx.try_recv().is_err();
            // Exercise the actual shutdown authority, without retiring its
            // independent Python dictionary/namespace roots in this fixture.
            retire_execution_locks(table);
            completion = done_rx.recv_timeout(Duration::from_secs(2));
            worker.join().expect("retired execution waiter terminated");
        }
        drop(owner);
        let mut locks = table.execution.get().unwrap().lock().unwrap();
        locks.retired = false;
        assert!(parked, "waiter parked without polling before retirement");
        assert_eq!(completion.unwrap(), (true, true));
        assert!(locks.active.is_empty() && locks.waiting.is_empty());
        assert_eq!(record.waiters.load(Ordering::Relaxed), 0);
        assert_eq!(record.owner.load(Ordering::Acquire), 0);
        assert_eq!(
            Arc::strong_count(&record),
            1,
            "no retired map/wait edge retains the record"
        );
    });
}

static COMPLETION_MUTATION: AtomicU64 = AtomicU64::new(0);

static RECURSIVE_MODE: AtomicU64 = AtomicU64::new(0);
static RECURSIVE_RUNS: AtomicU64 = AtomicU64::new(0);
static RECURSIVE_OUTER: AtomicU64 = AtomicU64::new(0);
static RECURSIVE_NESTED: AtomicU64 = AtomicU64::new(0);
static RECURSIVE_FAILURE: AtomicU64 = AtomicU64::new(0);
static RECURSIVE_RESTORED: AtomicU64 = AtomicU64::new(0);
static RECURSIVE_OUTER_REFS: AtomicU64 = AtomicU64::new(0);

struct RecursiveFixtureOwners<'a, 'py>(&'a PyToken<'py>);

impl Drop for RecursiveFixtureOwners<'_, '_> {
    fn drop(&mut self) {
        RECURSIVE_FAILURE.store(0, Ordering::SeqCst);
        for slot in [&RECURSIVE_OUTER, &RECURSIVE_NESTED] {
            let bits = slot.swap(0, Ordering::SeqCst);
            if bits != 0 {
                dec_ref_bits(self.0, bits);
            }
        }
    }
}

extern "C" fn init_g4_z_recursive() -> u64 {
    let generation = RECURSIVE_RUNS.fetch_add(1, Ordering::SeqCst) + 1;
    let original = publish_test_module("g4_z_recursive", None);
    crate::with_gil_entry_nopanic!(py, {
        let mode = RECURSIVE_MODE.load(Ordering::SeqCst);
        if generation == 1 {
            // Transfer the allocation owner to the bounded fixture scope,
            // independently of public dictionary/initializer custody.
            RECURSIVE_OUTER.store(original, Ordering::SeqCst);
            let original_pointer = obj_from_bits(original).as_ptr().unwrap();
            RECURSIVE_OUTER_REFS.store(
                u64::from(unsafe {
                    (*crate::header_from_obj_ptr(original_pointer)).ref_count_snapshot()
                }),
                Ordering::SeqCst,
            );
            let name = crate::attr_name_bits_from_bytes(py, b"g4_z_recursive").unwrap();
            let dictionary = module_dictionary(py).unwrap();
            if mode != 0 {
                unsafe {
                    drop(
                        crate::object::ops::dict_del_deferred(
                            py,
                            crate::ptr_from_bits(dictionary),
                            name,
                        )
                        .unwrap(),
                    );
                }
            }
            let entry_depth = crate::builtins::exceptions::exception_stack_depth();
            crate::builtins::exceptions::exception_stack_push();
            let nested = module_ensure(py, test_registry_id("g4_z_recursive"));
            assert_eq!(
                crate::builtins::exceptions::exception_stack_depth(),
                entry_depth + 1
            );
            RECURSIVE_RESTORED.store(
                u64::from(module_execution_owns_initializing_namespace(
                    py,
                    "g4_z_recursive",
                    original,
                )),
                Ordering::SeqCst,
            );
            if exception_pending(py) {
                let expected = RECURSIVE_FAILURE.load(Ordering::SeqCst);
                crate::builtins::exceptions::exception_context_set(py, expected);
                let active = crate::builtins::exceptions::molt_exception_active();
                assert_eq!(
                    active, expected,
                    "a caller can bare-reraise a failed nested import"
                );
                dec_ref_bits(py, active);
                if mode == 2 {
                    crate::clear_exception(py);
                }
            } else {
                RECURSIVE_NESTED.store(nested, Ordering::SeqCst);
                if mode >= 4 {
                    if mode >= 5 {
                        let replacement = if mode == 5 {
                            MoltObject::from_int(42).bits()
                        } else {
                            none_bits()
                        };
                        unsafe {
                            assert!(crate::dict_set_in_place(
                                py,
                                crate::ptr_from_bits(dictionary),
                                name,
                                replacement,
                            ));
                        }
                    }
                    crate::molt_exception_set_last(RECURSIVE_FAILURE.load(Ordering::SeqCst));
                }
            }
            crate::builtins::exceptions::exception_stack_pop(py);
            assert_eq!(
                crate::builtins::exceptions::exception_stack_depth(),
                entry_depth
            );
            dec_ref_bits(py, name);
        } else {
            if matches!(mode, 2 | 3) {
                // Execute the compiled RAISE cleanup contract rather than
                // bypassing it by setting only the pending exception.
                crate::builtins::exceptions::molt_exception_stack_clear();
                crate::molt_exception_set_last(RECURSIVE_FAILURE.load(Ordering::SeqCst));
            }
            dec_ref_bits(py, original);
        }
    });
    0
}

extern "C" fn init_g4_z_success_mutation() -> u64 {
    let original = publish_test_module("g4_z_success_mutation", None);
    crate::with_gil_entry_nopanic!(py, {
        let dictionary = module_dictionary(py).unwrap();
        let pointer = crate::ptr_from_bits(dictionary);
        let name = crate::attr_name_bits_from_bytes(py, b"g4_z_success_mutation").unwrap();
        let nested = crate::attr_name_bits_from_bytes(py, b"_molt_completion_nested").unwrap();
        unsafe {
            assert!(crate::dict_set_in_place(
                py,
                pointer,
                nested,
                MoltObject::from_int(7).bits()
            ));
            match COMPLETION_MUTATION.load(Ordering::SeqCst) {
                0 => assert!(crate::dict_set_in_place(
                    py,
                    pointer,
                    name,
                    MoltObject::from_int(42).bits()
                )),
                1 => assert!(crate::dict_set_in_place(py, pointer, name, none_bits())),
                2 => assert!(crate::dict_del_in_place(py, pointer, name).unwrap()),
                _ => {}
            }
        }
        for bits in [original, name, nested] {
            dec_ref_bits(py, bits);
        }
    });
    0
}

extern "C" fn init_test_io() -> u64 {
    let module = publish_test_module("_io", None);
    crate::with_gil_entry_nopanic!(py, {
        dec_ref_bits(py, module);
    });
    0
}

/// Install the synthetic registry exactly once per test process.  Rows
/// are pre-sorted; ids are their positions.
pub(crate) fn install_test_registry() {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        // Ids are declaration positions (rows pre-sorted; the builder
        // asserts the order): 0 _io, 1 builtins, 2 g4_alias, 3 g4_cycle_a,
        // 4 g4_cycle_b, 5 g4_fail, 6 g4_noinit, 7 g4_pkg,
        // 8 g4_pkg.sub, 9 g4_src, 10 g4_target, 11 g4_tomb,
        // 12 g4_tomb_ext, 13 g4_z_recursive, 14 g4_z_static_ext_fail,
        // 15 g4_z_success_mutation, 16 sys.
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
                "g4_z_recursive",
                init_g4_z_recursive as *const () as usize as u64,
                None,
                None,
                MODULE_KIND_SOURCE,
                0,
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
                "g4_z_success_mutation",
                init_g4_z_success_mutation as *const () as usize as u64,
                None,
                None,
                MODULE_KIND_SOURCE,
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
        let module = publish_test_module("g4_src", None);
        let _module_owner = crate::PtrDropGuard::new(obj_from_bits(module).as_ptr().unwrap());
        let index = test_registry_id("g4_src") as usize;
        let table = module_table(py).unwrap();
        let state = table.initialization().states[index].load(Ordering::Acquire);
        let bits = table.initialization().slots[index].load(Ordering::Acquire);
        let owner = initializer_owner(table, index);
        let cached = publication_for_test(py, "g4_src");
        let outcome = crate::test_support::catch_expected_unwind(|| {
            let _restore = ModuleTestRestore::capture(py, "g4_src");
            table.initialization().states[index].store(STATE_INITIALIZING, Ordering::Release);
            let _owner_fixture_1 = TestExecutionOwner::set(
                table,
                index,
                crate::concurrency::current_thread_id().wrapping_add(1),
            );
            panic!("synthetic initializer failure");
        });
        assert!(outcome.is_err());
        assert_eq!(
            table.initialization().states[index].load(Ordering::Acquire),
            state
        );
        assert_eq!(
            table.initialization().slots[index].load(Ordering::Acquire),
            bits
        );
        assert_eq!(initializer_owner(table, index), owner);
        assert_eq!(publication_for_test(py, "g4_src"), cached);
        assert!(!exception_pending(py));
    });
}

#[test]
fn completed_module_has_one_dictionary_owner_and_no_ready_slot_owner() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    install_test_registry();
    crate::with_gil_entry_nopanic!(py, {
        let _restore = ModuleTestRestore::capture(py, "g4_src");
        let id = test_registry_id("g4_src");
        remove_for_test(py, "g4_src");
        module_table_view_tombstone(py, id);
        let module = module_ensure(py, id);
        assert!(!exception_pending(py));
        let pointer = obj_from_bits(module).as_ptr().unwrap();
        let refs = || unsafe { (*crate::header_from_obj_ptr(pointer)).ref_count_snapshot() };
        let table = module_table(py).unwrap();
        assert_eq!(
            table.initialization().states[id as usize].load(Ordering::Acquire),
            STATE_READY
        );
        assert_eq!(
            table.initialization().slots[id as usize].load(Ordering::Acquire),
            0,
            "completed initialization does not retain a duplicate module owner"
        );
        let baseline = refs();
        let observed = module_lookup(py, "g4_src").unwrap().unwrap();
        assert_eq!(observed, module);
        assert_eq!(
            refs(),
            baseline + 1,
            "lookup transfers one independent owner"
        );
        dec_ref_bits(py, observed);
        remove_for_test(py, "g4_src");
        assert_eq!(
            refs(),
            baseline - 1,
            "deletion releases the sole dictionary publication edge"
        );
        assert!(module_lookup(py, "g4_src").unwrap().is_none());
        let replacement = module_ensure(py, id);
        assert!(!exception_pending(py));
        assert_ne!(
            replacement, module,
            "public deletion reruns the initializer rather than adopting a stale slot"
        );
        dec_ref_bits(py, replacement);
        dec_ref_bits(py, module);
    });
}

#[test]
fn identity_mismatched_rollback_does_not_retain_replacement_observations() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    install_test_registry();
    crate::with_gil_entry_nopanic!(py, {
        let _restore = ModuleTestRestore::capture(py, "g4_src");
        remove_for_test(py, "g4_src");
        let id = test_registry_id("g4_src");
        module_table_view_tombstone(py, id);
        let original = module_ensure(py, id);
        let name = crate::attr_name_bits_from_bytes(py, b"g4_src").unwrap();
        let replacement = MoltObject::from_ptr(crate::alloc_module_obj(py, name)).bits();
        inc_ref_bits(py, replacement);
        let old = replace_module_owned_for_test(py, "g4_src", Some(replacement)).unwrap();
        dec_ref_bits(py, old);
        let pointer = obj_from_bits(replacement).as_ptr().unwrap();
        let owners = || unsafe { (*crate::header_from_obj_ptr(pointer)).ref_count_snapshot() };
        let before = owners();
        for _ in 0..8 {
            crate::builtins::modules::module_cache_remove(name, Some(original));
            assert_eq!(
                owners(),
                before,
                "rollback releases its owned replacement observation"
            );
        }
        remove_for_test(py, "g4_src");
        assert_eq!(owners(), before - 1);
        for bits in [name, replacement, original] {
            dec_ref_bits(py, bits);
        }
        assert!(!exception_pending(py));
    });
}

#[test]
fn explicit_private_body_self_import_reads_public_missing_present_and_none() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    install_test_registry();
    crate::with_gil_entry_nopanic!(py, {
        for mode in 0..3 {
            let (_restore, public) = ModuleTestRestore::fresh(py, "g4_src");
            let _self_scope = PrivateSelfScope(py);
            let name = crate::attr_name_bits_from_bytes(py, b"g4_src").unwrap();
            let target = MoltObject::from_ptr(crate::alloc_module_obj(py, name)).bits();
            let dictionary = module_dictionary(py).unwrap();
            if mode == 0 {
                assert!(
                    unsafe { crate::dict_del_in_place(py, crate::ptr_from_bits(dictionary), name) }
                        .unwrap()
                );
            } else if mode == 2 {
                assert!(unsafe {
                    crate::dict_set_in_place(
                        py,
                        crate::ptr_from_bits(dictionary),
                        name,
                        none_bits(),
                    )
                });
            }
            let before = INIT_RUNS.load(Ordering::SeqCst);
            PRIVATE_SELF_IMPORT.store(1, Ordering::SeqCst);
            let loader = crate::builtins::types::molt_importlib_compiled_loader();
            let outcome =
                crate::builtins::types::molt_importlib_compiled_loader_exec_module(loader, target);
            assert!(is_none_bits(outcome));
            let failed = exception_pending(py);
            let nested = PRIVATE_SELF_RESULT.load(Ordering::SeqCst);
            if mode == 2 {
                assert!(failed);
                assert!(
                    pending_exception_text(py, crate::BuiltinExc::ModuleNotFoundError)
                        .contains("None in sys.modules")
                );
                assert_eq!(nested, 0);
            } else {
                assert!(!failed, "private body failed");
                assert_ne!(
                    nested, target,
                    "ordinary import must not return private execution namespace"
                );
                let cached = module_lookup(py, "g4_src").unwrap().unwrap();
                assert_eq!(cached, nested);
                if mode == 1 {
                    assert_eq!(nested, public);
                }
                dec_ref_bits(py, cached);
            }
            assert_eq!(
                INIT_RUNS.load(Ordering::SeqCst) - before,
                if mode == 0 { 2 } else { 1 }
            );
            let table = module_table(py).unwrap();
            let index = test_registry_id("g4_src") as usize;
            assert_eq!(
                table.initialization().slots[index].load(Ordering::Acquire),
                0
            );
            assert_eq!(initializer_owner(table, index), 0);
            for bits in [name, target, public, loader] {
                dec_ref_bits(py, bits);
            }
        }
    });
}

#[test]
fn compiled_load_module_preseeds_exact_namespace_under_ordinary_owner() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    install_test_registry();
    crate::with_gil_entry_nopanic!(py, {
        let (_restore, previous) = ModuleTestRestore::fresh(py, "g4_src");
        let _self_scope = PrivateSelfScope(py);
        remove_for_test(py, "g4_src");
        let name = crate::attr_name_bits_from_bytes(py, b"g4_src").unwrap();
        let loader = crate::builtins::types::molt_importlib_compiled_loader();
        let before = INIT_RUNS.load(Ordering::SeqCst);
        PRIVATE_SELF_IMPORT.store(1, Ordering::SeqCst);
        let loaded =
            crate::builtins::types::molt_importlib_compiled_loader_load_module(loader, name);
        assert!(!exception_pending(py));
        assert_eq!(
            INIT_RUNS.load(Ordering::SeqCst),
            before + 1,
            "ordinary spec preseed admits self import without a second initializer"
        );
        assert_eq!(loaded, PRIVATE_SELF_RESULT.load(Ordering::SeqCst));
        let public = module_lookup(py, "g4_src").unwrap().unwrap();
        assert_eq!(public, loaded);
        let key = crate::attr_name_bits_from_bytes(py, b"__spec__").unwrap();
        let spec = crate::builtins::attributes::get_attr_name(py, loaded, key).unwrap();
        assert_eq!(
            crate::builtins::platform::spec_is_initializing(py, spec, ModuleImportPolicy::Python),
            Ok(false)
        );
        let table = module_table(py).unwrap();
        let index = test_registry_id("g4_src") as usize;
        assert_eq!(
            table.initialization().states[index].load(Ordering::Acquire),
            STATE_READY
        );
        assert_eq!(initializer_owner(table, index), 0);
        assert_eq!(
            table.initialization().slots[index].load(Ordering::Acquire),
            0
        );
        assert_eq!(
            table.initialization().specs[index].load(Ordering::Acquire),
            0
        );
        for bits in [previous, name, loader, loaded, public, key, spec] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn private_script_and_runpy_cached_projection_preserve_versioned_init_globals() {
    use crate::builtins::exceptions::ExceptionValue;
    use crate::builtins::modules::{ExecutionMetadata, execute_compiled_module};
    let transaction = crate::test_support::RuntimeTestTransaction::new();
    install_test_registry();
    crate::with_gil_entry_nopanic!(py, {
        let cached = crate::attr_name_bits_from_bytes(py, b"__cached__").unwrap();
        let spec = ExceptionValue::adopt(py, unsafe {
            crate::call_callable0(py, crate::builtins::types::simplenamespace_class(py))
        });
        for (name, value) in [
            (b"cached".as_slice(), MoltObject::from_int(431).bits()),
            (b"loader".as_slice(), none_bits()),
        ] {
            let key = crate::attr_name_bits_from_bytes(py, name).unwrap();
            crate::builtins::attributes::mutate_attr_name(
                py,
                spec.bits(),
                key,
                crate::builtins::attr::DescriptorMutation::Set(value),
                molt_cpython_abi::hooks::AttributeMutation::Normal,
            )
            .unwrap();
            dec_ref_bits(py, key);
        }
        let initial = ExceptionValue::adopt(
            py,
            MoltObject::from_ptr(crate::alloc_dict_with_pairs(
                py,
                &[cached, MoltObject::from_int(99).bits()],
            ))
            .bits(),
        );
        let _spec_scope = PrivateBodySpecScope;
        PRIVATE_BODY_SPEC.store(spec.bits(), Ordering::SeqCst);
        for minor in 12..=15 {
            transaction.with_target_python_minor(py, minor, || {
                for script in [false, true] {
                    let metadata = if script {
                        ExecutionMetadata::ScriptFile("/virtual/control.py".into())
                    } else {
                        ExecutionMetadata::Module { argv0: None }
                    };
                    let module = execute_compiled_module(
                        py,
                        "g4_src",
                        "_molt_execution_cached",
                        Some(initial.bits()),
                        false,
                        metadata,
                    )
                    .unwrap_or_else(|error| {
                        panic!("private metadata execution failed: {}", error.into_bits())
                    });
                    let module = ExceptionValue::adopt(py, module);
                    let spec_key = crate::attr_name_bits_from_bytes(py, b"__spec__").unwrap();
                    let observed_spec =
                        crate::builtins::attributes::get_attr_name(py, module.bits(), spec_key)
                            .unwrap();
                    assert_eq!(
                        observed_spec,
                        if script { none_bits() } else { spec.bits() },
                        "the actual body admits its selected spec exactly once"
                    );
                    dec_ref_bits(py, observed_spec);
                    dec_ref_bits(py, spec_key);
                    let value =
                        crate::builtins::attributes::get_attr_name(py, module.bits(), cached)
                            .unwrap();
                    let expected = if minor >= 15 {
                        MoltObject::from_int(99).bits()
                    } else if script {
                        none_bits()
                    } else {
                        MoltObject::from_int(431).bits()
                    };
                    assert_eq!(value, expected, "minor={minor} script={script}");
                    dec_ref_bits(py, value);
                    assert!(!exception_pending(py));
                }
            });
        }
        dec_ref_bits(py, cached);
    });
}

#[test]
fn explicit_extension_publication_and_rollback_keep_own_custody_inside_private_body() {
    use crate::builtins::modules;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    install_test_registry();
    crate::with_gil_entry_nopanic!(py, {
        let (_restore, initial) = ModuleTestRestore::fresh(py, "g4_src");
        let id = test_registry_id("g4_src");
        let name = crate::attr_name_bits_from_bytes(py, b"g4_src").unwrap();
        let extension = MoltObject::from_ptr(crate::alloc_module_obj(py, name)).bits();
        let _context = InitializerContext::enter(py, id, InitializerKind::Execution);
        crate::cpython_abi_hooks::with_test_module_publication(py, name, extension, || {
            let observed = module_lookup(py, "g4_src").unwrap().unwrap();
            assert_eq!(observed, extension);
            dec_ref_bits(py, observed);
            modules::module_cache_remove(name, None);
            let observed = module_lookup(py, "g4_src").unwrap().unwrap();
            assert_eq!(
                observed, extension,
                "private compiler cleanup preserves public entry"
            );
            dec_ref_bits(py, observed);
        });
        assert!(
            module_lookup(py, "g4_src").unwrap().is_none(),
            "actual ModulePublication rollback remains admitted"
        );
        for bits in [name, initial, extension] {
            dec_ref_bits(py, bits);
        }
        assert!(!exception_pending(py));
    });
}

#[test]
fn explicit_spec_producer_owns_original_through_snapshot_and_metadata_replacement() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    install_test_registry();
    crate::with_gil_entry_nopanic!(py, {
        let (_restore, module) = ModuleTestRestore::fresh(py, "g4_src");
        let _initializing = ModuleTestRestore::initializing(py, "g4_src", module);
        let id = test_registry_id("g4_src");
        let index = id as usize;
        let table = module_table(py).unwrap();
        let spec =
            unsafe { crate::call_callable0(py, crate::builtins::types::simplenamespace_class(py)) };
        let replacement =
            unsafe { crate::call_callable0(py, crate::builtins::types::simplenamespace_class(py)) };
        let key = crate::attr_name_bits_from_bytes(py, b"__spec__").unwrap();
        let flag = crate::attr_name_bits_from_bytes(py, b"_initializing").unwrap();
        crate::builtins::modules::molt_module_set_attr(module, key, replacement);
        assert_eq!(
            table.initialization().specs[index].load(Ordering::Acquire),
            0,
            "generic attribute spelling cannot acquire compiler provenance"
        );
        let _context = InitializerContext::enter(py, id, InitializerKind::Import);
        molt_module_initialization_spec(module, spec);
        assert!(!exception_pending(py));
        assert_eq!(
            table.initialization().specs[index].load(Ordering::Acquire),
            spec
        );
        assert_eq!(
            crate::builtins::platform::spec_is_initializing(py, spec, ModuleImportPolicy::Python),
            Ok(true)
        );
        let snapshot = detach_initializer(table, id);
        assert_eq!(
            table.initialization().specs[index].load(Ordering::Acquire),
            0
        );
        assert_eq!(snapshot.spec, spec);
        restore_module_execution(py, Some(snapshot));
        crate::builtins::modules::molt_module_set_attr(module, key, replacement);
        set_spec_initializing(py, replacement, true);
        finish_initializing_spec(py, table, id);
        assert!(!exception_pending(py));
        assert_eq!(
            crate::builtins::platform::spec_is_initializing(py, spec, ModuleImportPolicy::Python),
            Ok(false)
        );
        assert_eq!(
            crate::builtins::platform::spec_is_initializing(
                py,
                replacement,
                ModuleImportPolicy::Python
            ),
            Ok(true),
            "finally changes the captured original, not current module metadata"
        );
        let retained = table.initialization().specs[index].swap(0, Ordering::AcqRel);
        assert_eq!(retained, spec);
        dec_ref_bits(py, retained);
        for bits in [spec, replacement, key, flag, module] {
            dec_ref_bits(py, bits);
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn foreign_wait_uses_observed_public_spec_and_preserves_completed_replacements() {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    install_test_registry();
    crate::with_gil_entry_nopanic!(py, {
        for mode in 0..6 {
            let (_restore, module) = ModuleTestRestore::fresh(py, "g4_src");
            let mut _initializing = ModuleTestRestore::initializing(py, "g4_src", module);
            let id = test_registry_id("g4_src");
            let table = module_table(py).unwrap();
            let index = id as usize;
            let name = crate::attr_name_bits_from_bytes(py, b"g4_src").unwrap();
            let spec = unsafe {
                crate::call_callable0(py, crate::builtins::types::simplenamespace_class(py))
            };
            let key = crate::attr_name_bits_from_bytes(py, b"__spec__").unwrap();
            let (published, blocks) = match mode {
                0 => (module, true), // pre-metadata bootstrap phase
                1 | 2 => {
                    set_spec_initializing(py, spec, mode == 1);
                    crate::builtins::modules::molt_module_set_attr(module, key, spec);
                    inc_ref_bits(py, spec);
                    table.initialization().specs[index].store(spec, Ordering::Release);
                    (module, mode == 1)
                }
                3 => {
                    let nested = crate::builtins::modules::molt_module_new(name);
                    set_spec_initializing(py, spec, false);
                    crate::builtins::modules::molt_module_set_attr(nested, key, spec);
                    (nested, false)
                }
                4 => (MoltObject::from_int(42).bits(), false),
                _ => (none_bits(), false),
            };
            let dictionary = module_dictionary(py).unwrap();
            unsafe {
                assert!(crate::dict_set_in_place(
                    py,
                    crate::ptr_from_bits(dictionary),
                    name,
                    published
                ));
            }
            let (started_tx, started_rx) = mpsc::channel();
            let (done_tx, done_rx) = mpsc::channel();
            let worker = std::thread::spawn(move || {
                crate::with_gil_entry_nopanic!(worker_py, {
                    started_tx
                        .send(crate::concurrency::current_thread_id())
                        .unwrap();
                    let value = module_ensure(worker_py, id);
                    let failed = exception_pending(worker_py);
                    if failed {
                        crate::clear_exception(worker_py);
                    }
                    done_tx.send((value, failed)).unwrap();
                });
            });
            let result;
            {
                let _release = crate::concurrency::GilReleaseGuard::suspend();
                let started = started_rx.recv_timeout(Duration::from_secs(2));
                let mut waited = false;
                if blocks && let Ok(tid) = started.as_ref() {
                    let deadline = Instant::now() + Duration::from_secs(2);
                    while Instant::now() < deadline {
                        waited = table
                            .execution
                            .get()
                            .unwrap()
                            .lock()
                            .unwrap()
                            .waiting
                            .get(tid)
                            .is_some_and(|record| record.waiters.load(Ordering::Relaxed) != 0);
                        if waited {
                            break;
                        }
                        std::thread::yield_now();
                    }
                }
                if blocks || started.is_err() {
                    _initializing.release_execution();
                    table.initialization().states[index].store(STATE_READY, Ordering::Release);
                }
                result = done_rx.recv_timeout(Duration::from_secs(2));
                // Release a stuck waiter before reporting the bounded test
                // failure, so no worker or runtime lease escapes the fixture.
                _initializing.release_execution();
                table.initialization().states[index].store(STATE_READY, Ordering::Release);
                worker.join().expect("foreign importer terminated");
                assert!(started.is_ok(), "foreign importer started");
                assert!(
                    !blocks || waited,
                    "original active namespace must enter owner wait"
                );
            }
            let (value, failed) =
                result.expect("completed replacement must not wait for outer owner");
            assert_eq!(failed, mode == 5);
            assert_eq!(value, published);
            if !failed {
                dec_ref_bits(py, value);
            }
            if mode == 3 {
                dec_ref_bits(py, published);
            }
            for bits in [name, key, spec, module] {
                dec_ref_bits(py, bits);
            }
        }
    });
}

#[test]
fn recursive_public_deletion_reenters_initializer_and_restores_outer_custody() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    install_test_registry();
    crate::with_gil_entry_nopanic!(py, {
        let text = "g4_z_recursive";
        let _restore = ModuleTestRestore::capture(py, text);
        let id = test_registry_id(text);
        let table = module_table(py).unwrap();
        // Source-backed CPython oracle: present / deleted / caught-inner /
        // uncaught-inner / outer-failure / replaced-outer / None-outer.
        for mode in 0..7 {
            remove_for_test(py, text);
            module_table_view_tombstone(py, id);
            RECURSIVE_MODE.store(mode, Ordering::SeqCst);
            RECURSIVE_RUNS.store(0, Ordering::SeqCst);
            RECURSIVE_RESTORED.store(0, Ordering::SeqCst);
            raise_exception::<()>(py, BuiltinExc::ValueError, "recursive initializer failure");
            let error = crate::builtins::exceptions::molt_exception_last_pending();
            let error = crate::builtins::exceptions::ExceptionValue::adopt(py, error);
            crate::clear_exception(py);
            let _owners = RecursiveFixtureOwners(py);
            RECURSIVE_FAILURE.store(error.bits(), Ordering::SeqCst);
            let result = module_ensure(py, id);
            assert_eq!(
                RECURSIVE_RUNS.load(Ordering::SeqCst),
                if mode == 0 { 1 } else { 2 }
            );
            assert_eq!(
                RECURSIVE_RESTORED.load(Ordering::SeqCst),
                1,
                "the outer execution resumes with its exact namespace owner"
            );
            let outer = RECURSIVE_OUTER.load(Ordering::SeqCst);
            let nested = RECURSIVE_NESTED.load(Ordering::SeqCst);
            if mode <= 1 {
                assert!(!exception_pending(py));
                assert_eq!(result, nested);
                assert_eq!(result == outer, mode == 0);
                let public = module_lookup(py, text).unwrap().unwrap();
                assert_eq!(public, nested);
                dec_ref_bits(py, public);
            } else {
                assert!(is_none_bits(result));
                assert!(exception_pending(py));
                let observed = crate::builtins::exceptions::molt_exception_last_pending();
                if mode == 2 {
                    assert!(crate::builtins::exceptions::exception_matches_kind(
                        py,
                        observed,
                        crate::BuiltinExc::KeyError
                    ));
                } else {
                    assert_eq!(
                        observed,
                        error.bits(),
                        "cleanup preserves the initializer's exact exception"
                    );
                }
                dec_ref_bits(py, observed);
                crate::clear_exception(py);
                assert!(module_lookup(py, text).unwrap().is_none());
                if mode >= 4 {
                    assert_ne!(nested, 0);
                    assert_ne!(nested, outer);
                }
            }
            assert_eq!(
                table.initialization().slots[id as usize].load(Ordering::Acquire),
                0,
                "both initializer transactions release their temporary slot owner"
            );
            assert_eq!(initializer_owner(table, id as usize), 0);
            dec_ref_bits(py, result);
            remove_for_test(py, text);
            let original_pointer = obj_from_bits(outer).as_ptr().unwrap();
            let remaining =
                unsafe { (*crate::header_from_obj_ptr(original_pointer)).ref_count_snapshot() };
            assert_eq!(
                u64::from(remaining),
                RECURSIVE_OUTER_REFS.load(Ordering::SeqCst) - 2 + u64::from(mode == 0),
                "public and transaction owners retire; only fixture/ambient owners remain"
            );
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn successful_initializer_returns_final_publication_and_moves_it_after_nested_imports() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    install_test_registry();
    crate::with_gil_entry_nopanic!(py, {
        let text = "g4_z_success_mutation";
        let _restore = ModuleTestRestore::capture(py, text);
        let id = test_registry_id(text);
        let prior_nested = replace_module_owned_for_test(py, "_molt_completion_nested", None);
        for mode in 0..4 {
            COMPLETION_MUTATION.store(mode, Ordering::SeqCst);
            remove_for_test(py, text);
            module_table_view_tombstone(py, id);
            let result = module_ensure(py, id);
            if mode == 2 {
                assert!(exception_pending(py));
                let error = crate::builtins::exceptions::exception_last_bits_noinc(py).unwrap();
                assert!(crate::builtins::exceptions::exception_matches_kind(
                    py,
                    error,
                    crate::BuiltinExc::KeyError
                ));
                let message = pending_exception_text(py, crate::BuiltinExc::KeyError);
                assert!(message.contains(text));
                assert!(module_lookup(py, text).unwrap().is_none());
            } else {
                assert!(!exception_pending(py));
                if mode == 0 {
                    assert_eq!(obj_from_bits(result).as_int(), Some(42));
                }
                if mode == 1 {
                    assert!(is_none_bits(result));
                }
                if mode == 3 {
                    assert!(obj_from_bits(result).as_ptr().is_some());
                }
                let published = module_lookup(py, text).unwrap().unwrap();
                assert_eq!(result, published);
                dec_ref_bits(py, published);
                let dictionary = module_dictionary(py).unwrap();
                let keys = unsafe {
                    crate::builtins::containers::dict_entries(crate::ptr_from_bits(dictionary))
                }
                .iter()
                .filter(|row| row.hash.is_some())
                .map(|row| crate::string_obj_to_owned(obj_from_bits(row.key)))
                .collect::<Vec<_>>();
                assert_eq!(
                    keys.last(),
                    Some(&Some(text.to_owned())),
                    "successful completion follows nested publication in dictionary order"
                );
            }
            assert_eq!(
                module_table(py).unwrap().initialization().slots[id as usize]
                    .load(Ordering::Acquire),
                0
            );
            dec_ref_bits(py, result);
            remove_for_test(py, "_molt_completion_nested");
        }
        let displaced = replace_module_owned_for_test(py, "_molt_completion_nested", prior_nested);
        if let Some(bits) = displaced {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn sys_bootstrap_exposes_existing_dictionary_without_replay() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    install_test_registry();
    crate::with_gil_entry_nopanic!(py, {
        let _restore = ModuleTestRestore::capture(py, "g4_src");
        let before = module_dictionary(py).unwrap();
        let module = publish_test_module("g4_src", None);
        let sys_name = crate::attr_name_bits_from_bytes(py, b"sys").unwrap();
        let sys = crate::molt_module_new(sys_name);
        publish_interpreter_sys_for_test(py, sys);
        let exposed = crate::builtins::modules::sys_modules_dict_bits(py, sys).unwrap();
        assert_eq!(
            before, exposed,
            "bootstrap exposes the original store, with no copy/replay"
        );
        let found = module_lookup(py, "g4_src").unwrap().unwrap();
        assert_eq!(found, module);
        let modules_name = crate::attr_name_bits_from_bytes(py, b"modules").unwrap();
        let replacement = MoltObject::from_ptr(crate::alloc_dict_with_pairs(py, &[])).bits();
        let namespace = unsafe { crate::module_dict_bits(crate::ptr_from_bits(sys)) };
        unsafe {
            assert!(crate::dict_set_in_place(
                py,
                crate::ptr_from_bits(namespace),
                modules_name,
                replacement
            ));
        }
        let imported = module_ensure(py, test_registry_id("g4_src"));
        assert_eq!(
            imported, module,
            "compiled ensure uses the interpreter store after public attribute rebinding"
        );
        dec_ref_bits(py, imported);
        unsafe {
            assert!(
                crate::dict_del_in_place(py, crate::ptr_from_bits(namespace), modules_name)
                    .unwrap()
            );
        }
        let imported = module_ensure(py, test_registry_id("g4_src"));
        assert_eq!(
            imported, module,
            "attribute deletion does not retarget compiled imports"
        );
        dec_ref_bits(py, imported);
        dec_ref_bits(py, replacement);
        dec_ref_bits(py, modules_name);
        for bits in [found, module, exposed, sys, sys_name] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn interpreter_sys_publication_requires_initializer_and_rejects_dictionary_payload() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    install_test_registry();
    crate::with_gil_entry_nopanic!(py, {
        let name = crate::attr_name_bits_from_bytes(py, b"sys").unwrap();
        remove_for_test(py, "sys");
        let module = crate::molt_module_new(name);
        let unrelated = crate::molt_module_new(name);
        crate::molt_module_cache_set(name, unrelated);
        assert!(runtime_state(py).interpreter_sys.module(py).is_none());
        let table = module_table(py).unwrap();
        let index = module_id_of("sys").unwrap() as usize;
        table.initialization().states[index].store(STATE_INITIALIZING, Ordering::Release);
        let _owner_fixture_2 =
            TestExecutionOwner::set(table, index, crate::concurrency::current_thread_id());
        let dictionary = MoltObject::from_ptr(crate::alloc_dict_with_pairs(py, &[])).bits();
        crate::molt_module_cache_set(name, dictionary);
        assert!(exception_pending(py));
        assert!(runtime_state(py).interpreter_sys.module(py).is_none());
        assert_eq!(
            table.initialization().slots[index].load(Ordering::Acquire),
            0
        );
        assert_eq!(publication_for_test(py, "sys"), Some(unrelated));
        crate::clear_exception(py);
        crate::molt_module_cache_set(name, module);
        assert!(!exception_pending(py));
        assert_eq!(runtime_state(py).interpreter_sys.module(py), Some(module));
        assert_eq!(
            publication_for_test(py, "sys"),
            Some(module),
            "initializer replaces the unrelated prepublication"
        );
        remove_for_test(py, "sys");
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
        remove_for_test(py, "sys");
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
        let implementation_key = crate::attr_name_bits_from_bytes(py, b"implementation").unwrap();
        let path_key = crate::attr_name_bits_from_bytes(py, b"path").unwrap();
        let stdout_key = crate::attr_name_bits_from_bytes(py, b"stdout").unwrap();
        let marker =
            MoltObject::from_ptr(crate::alloc_tuple(py, &[MoltObject::from_int(82).bits()])).bits();
        unsafe {
            assert!(
                crate::dict_get_in_place(py, dict, version_key)
                    .expect("dictionary lookup succeeds")
                    .is_some()
            );
            crate::dict_del_in_place(py, dict, version_key).expect("operation succeeds");
            crate::dict_del_in_place(py, dict, stdout_key).expect("operation succeeds");
            assert!(crate::dict_set_in_place(
                py,
                dict,
                implementation_key,
                marker
            ));
            assert!(crate::dict_set_in_place(py, dict, path_key, marker));
        }
        assert!(!exception_pending(py));

        crate::molt_module_cache_set(name, sys);
        assert!(!exception_pending(py));
        let table = module_table(py).unwrap();
        assert_eq!(
            table.initialization().states[id as usize].load(Ordering::Acquire),
            STATE_READY
        );
        assert_eq!(
            detach_cache_publication(py, "sys", Some(marker)),
            0,
            "a different owner's removal cannot detach the ready sys namespace"
        );
        assert_eq!(
            table.initialization().slots[id as usize].load(Ordering::Acquire),
            0
        );
        crate::builtins::modules::module_cache_remove(name, Some(sys));
        assert!(!exception_pending(py));
        assert_eq!(
            module_table(py).unwrap().initialization().states[id as usize].load(Ordering::Acquire),
            STATE_READY,
            "public deletion does not mirror membership into quiescent metadata"
        );
        assert_eq!(
            table.initialization().slots[id as usize].load(Ordering::Acquire),
            0
        );
        assert!(publication_for_test(py, "sys").is_none());
        let reimported = module_ensure(py, id);
        assert!(!exception_pending(py));
        assert_eq!(
            SYS_INIT_RUNS.load(Ordering::SeqCst),
            initialized + 1,
            "reimport must run the registered initializer through module_ensure"
        );
        assert_eq!(
            table.initialization().states[id as usize].load(Ordering::Acquire),
            STATE_READY
        );
        assert_eq!(
            reimported, sys,
            "test initializer republishes its retained builtin namespace"
        );

        // Host entry setup can repeat for the same target, but must never
        // replace finalized Python shapes or recreate deleted public keys.
        let (info, _) = crate::object::ops_sys::current_sys_version_info(runtime_state(py));
        let release = MoltObject::from_ptr(alloc_string(py, info.releaselevel.as_bytes())).bits();
        crate::molt_sys_set_version_info(
            MoltObject::from_int(info.major).bits(),
            MoltObject::from_int(info.minor).bits(),
            MoltObject::from_int(info.micro).bits(),
            release,
            MoltObject::from_int(info.serial).bits(),
        );
        assert!(!exception_pending(py));
        unsafe {
            assert!(
                crate::dict_get_in_place(py, dict, version_key)
                    .expect("dictionary lookup succeeds")
                    .is_none()
            );
            assert!(
                crate::dict_get_in_place(py, dict, stdout_key)
                    .expect("dictionary lookup succeeds")
                    .is_none()
            );
            assert_eq!(
                crate::dict_get_in_place(py, dict, implementation_key)
                    .expect("dictionary lookup succeeds"),
                Some(marker)
            );
            assert_eq!(
                crate::dict_get_in_place(py, dict, path_key).expect("dictionary lookup succeeds"),
                Some(marker)
            );
        }
        for bits in [
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
        remove_for_test(py, "sys");
        let sys = publish_test_module("sys", None);
        let id = test_registry_id("g4_src");
        let table = module_table(py).unwrap();
        let idx = id as usize;
        let old = module_ensure(py, id);
        assert!(!exception_pending(py));
        assert_eq!(
            table.initialization().states[idx].load(Ordering::Acquire),
            STATE_READY
        );
        let name = crate::attr_name_bits_from_bytes(py, b"g4_src").unwrap();
        let new = crate::builtins::modules::molt_module_new(name);
        crate::builtins::modules::module_cache_publish(
            name,
            new,
            crate::builtins::modules::ModuleCachePublication::Extension,
        );
        assert!(!exception_pending(py));
        assert_eq!(table.initialization().slots[idx].load(Ordering::Acquire), 0);
        assert_eq!(
            table.initialization().states[idx].load(Ordering::Acquire),
            STATE_REPLACED
        );
        let old_ptr = obj_from_bits(old).as_ptr().unwrap();
        assert_eq!(
            unsafe { (*crate::object::header_from_obj_ptr(old_ptr)).ref_count_snapshot() },
            1,
            "dictionary replacement must release the displaced publication owner"
        );
        crate::builtins::modules::module_cache_remove(name, Some(new));
        assert_eq!(table.initialization().slots[idx].load(Ordering::Acquire), 0);
        assert_eq!(
            table.initialization().states[idx].load(Ordering::Acquire),
            STATE_REPLACED,
            "the next ordinary ensure observation owns missing-publication admission"
        );

        inc_ref_bits(py, old);
        table.initialization().slots[idx].store(old, Ordering::Release);
        table.initialization().states[idx].store(STATE_INITIALIZING, Ordering::Release);
        let _owner_fixture_3 = TestExecutionOwner::set(
            table,
            idx,
            crate::concurrency::current_thread_id().wrapping_add(1),
        );
        assert_eq!(publish_extension_result(py, "g4_src", new), 0);
        assert_eq!(detach_cache_publication(py, "g4_src", Some(old)), 0);
        assert_eq!(
            table.initialization().slots[idx].load(Ordering::Acquire),
            old
        );
        assert_eq!(
            table.initialization().states[idx].load(Ordering::Acquire),
            STATE_INITIALIZING
        );
        let _owner_fixture_4 =
            TestExecutionOwner::set(table, idx, crate::concurrency::current_thread_id());
        let retired = publish_extension_result(py, "g4_src", new);
        assert_eq!(retired, old);
        assert_eq!(
            table.initialization().slots[idx].load(Ordering::Acquire),
            new
        );
        assert_eq!(
            table.initialization().states[idx].load(Ordering::Acquire),
            STATE_INITIALIZING
        );
        dec_ref_bits(py, retired);
        assert_eq!(detach_cache_publication(py, "g4_src", Some(old)), 0);
        assert_eq!(
            table.initialization().slots[idx].load(Ordering::Acquire),
            new
        );
        let retired = detach_cache_publication(py, "g4_src", Some(new));
        dec_ref_bits(py, retired);
        let _owner_fixture_5 = TestExecutionOwner::set(table, idx, 0);
        table.initialization().states[idx].store(STATE_UNINIT, Ordering::Release);
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
        let previous_sys = publication_for_test(py, "sys");
        if let Some(bits) = previous_sys {
            inc_ref_bits(py, bits);
        }
        remove_for_test(py, "sys");
        let sys = publish_test_module("sys", None);
        let modules_bits = crate::builtins::modules::sys_modules_dict_bits(py, sys).unwrap();
        let modules = obj_from_bits(modules_bits).as_ptr().unwrap();
        let _modules_owner = crate::PtrDropGuard::new(modules);
        let name = crate::attr_name_bits_from_bytes(py, b"g4_src").unwrap();
        let id = test_registry_id("g4_src");
        unsafe {
            crate::object::ops::dict_del_in_place(py, modules, name).expect("operation succeeds")
        };
        module_table_view_tombstone(py, id);
        let table = module_table(py).unwrap();
        table.initialization().states[id as usize].store(STATE_UNINIT, Ordering::Release);
        let previous = publish_test_module("g4_src", None);
        assert!(unsafe {
            crate::object::ops::dict_del_in_place(py, modules, name).expect("operation succeeds")
        });
        let runs = INIT_RUNS.load(Ordering::SeqCst);
        let current = module_ensure(py, id);
        assert!(!exception_pending(py));
        assert_ne!(
            current, previous,
            "a public miss must reinitialize rather than revive a completed slot"
        );
        assert_eq!(INIT_RUNS.load(Ordering::SeqCst), runs + 1);

        // Same-thread self-import during initialization sees replacement
        // without taking the active initializer's transaction slot/state.
        inc_ref_bits(py, current);
        table.initialization().slots[id as usize].store(current, Ordering::Release);
        table.initialization().states[id as usize].store(STATE_INITIALIZING, Ordering::Release);
        let _owner_fixture_6 =
            TestExecutionOwner::set(table, id as usize, crate::concurrency::current_thread_id());
        let replacement = MoltObject::from_int(42).bits();
        unsafe { assert!(crate::dict_set_in_place(py, modules, name, replacement)) };
        assert!(
            matches!(crate::builtins::modules::module_import_attempt(name),
                Ok(crate::builtins::modules::ModuleImportOutcome::Imported(bits)) if bits == replacement)
        );
        assert_eq!(
            table.initialization().slots[id as usize].load(Ordering::Acquire),
            current
        );
        assert_eq!(
            table.initialization().states[id as usize].load(Ordering::Acquire),
            STATE_INITIALIZING
        );
        let _owner_fixture_7 = TestExecutionOwner::set(table, id as usize, 0);
        table.initialization().states[id as usize].store(STATE_READY, Ordering::Release);
        let completed = table.initialization().slots[id as usize].swap(0, Ordering::AcqRel);
        dec_ref_bits(py, completed);
        unsafe { assert!(crate::dict_set_in_place(py, modules, name, current)) };

        // Parent initialization may publish its child's public replacement
        // without invoking child ensure. The subsequent loop observes it.
        for text in ["g4_pkg.sub", "g4_pkg"] {
            let key = crate::attr_name_bits_from_bytes(py, text.as_bytes()).unwrap();
            unsafe {
                crate::object::ops::dict_del_in_place(py, modules, key).expect("operation succeeds")
            };
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
        remove_for_test(py, "sys");
        if let Some(bits) = previous_sys {
            publish_for_test(py, "sys", bits);
            dec_ref_bits(py, bits);
        }
        dec_ref_bits(py, sys);
        assert!(!exception_pending(py));
    });
}

fn pending_exception_text(_py: &PyToken<'_>, expected_kind: crate::BuiltinExc) -> String {
    assert!(exception_pending(_py), "expected a pending exception");
    let exc_bits = crate::builtins::exceptions::molt_exception_last_pending();
    assert!(
        crate::builtins::exceptions::exception_matches_kind(_py, exc_bits, expected_kind),
        "expected {expected_kind:?} before clearing its owned exception"
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
    let builtins_bits = crate::builtins::frames::frame_effective_builtins_bits(_py, globals_bits)
        .expect("builtins namespace lookup succeeds");
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
        remove_for_test(py, "builtins");
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
            unsafe {
                crate::dict_get_in_place(py, dict, property_name)
                    .expect("dictionary lookup succeeds")
            }
            .is_none(),
            "a same-named standalone module must not gain bootstrap privilege"
        );
        assert!(!module_initialization_awaits_publication(py, "builtins"));
        table.initialization().states[idx].store(STATE_INITIALIZING, Ordering::Release);
        let _owner_fixture_8 =
            TestExecutionOwner::set(table, idx, crate::concurrency::current_thread_id());
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
                unsafe {
                    crate::dict_get_in_place(py, dict, key).expect("dictionary lookup succeeds")
                },
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
        let provider_open = unsafe {
            crate::dict_get_in_place(py, io_dict, open_name).expect("dictionary lookup succeeds")
        };
        assert!(provider_open.is_some(), "admitted provider publishes open");
        assert_eq!(
            unsafe {
                crate::dict_get_in_place(py, dict, open_name).expect("dictionary lookup succeeds")
            },
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
                unsafe {
                    crate::dict_get_in_place(py, dict, key).expect("dictionary lookup succeeds")
                }
                .is_none(),
                "{hidden}"
            );
            dec_ref_bits(py, key);
        }
        table.initialization().states[idx].store(STATE_READY, Ordering::Release);
        let _owner_fixture_9 = TestExecutionOwner::set(table, idx, 0);
        for builtin in ["property", "len", "ValueError"] {
            let key = MoltObject::from_ptr(alloc_string(py, builtin.as_bytes())).bits();
            assert!(unsafe {
                crate::dict_del_in_place(py, dict, key).expect("operation succeeds")
            });
            // Republication is not permission to repair a user mutation.
            let result = crate::molt_module_cache_set(name, module);
            if !is_none_bits(result) {
                dec_ref_bits(py, result);
            }
            let value = crate::molt_module_get_global(unrelated, key);
            assert!(is_none_bits(value));
            assert!(pending_exception_text(py, crate::BuiltinExc::NameError).contains("NameError"));
            assert!(
                unsafe {
                    crate::dict_get_in_place(py, dict, key).expect("dictionary lookup succeeds")
                }
                .is_none()
            );
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
        remove_for_test(_py, "builtins");
        assert!(!exception_pending(_py));
        let builtins_bits = publish_test_module("builtins", None);
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

        // Public membership does not establish the interpreter's builtins
        // role. Only the actual initializer publication captures that
        // dictionary, independently of later transaction metadata.
        assert_eq!(interpreter_builtins_dictionary(_py), None);
        let initial_state = table.initialization().states[idx].load(Ordering::Acquire);
        let cached_bits = module_ensure(_py, builtins_id);
        assert_eq!(cached_bits, builtins_bits);
        dec_ref_bits(_py, cached_bits);
        assert_eq!(
            table.initialization().states[idx].load(Ordering::Acquire),
            initial_state,
            "a public dictionary hit does not invent initialization metadata"
        );
        let snapshot = ModuleTestRestore::execution(_py, detach_initializer(table, builtins_id));
        let owner = crate::concurrency::current_thread_id();
        let _owner_fixture_10 = TestExecutionOwner::set(table, idx, owner);
        table.initialization().states[idx].store(STATE_INITIALIZING, Ordering::Release);
        publish_initializing_namespace(_py, "builtins", builtins_bits);
        assert_eq!(interpreter_builtins_dictionary(_py), Some(dict_bits));
        let initializer_namespace = table.initialization().slots[idx].swap(0, Ordering::AcqRel);
        assert_eq!(initializer_namespace, builtins_bits);
        dec_ref_bits(_py, initializer_namespace);
        // Reservation alone owns no namespace; publication while
        // initializing owns exactly the current slot object. Default
        // lookup still uses the dictionary captured by the real producer.
        for state in [STATE_UNINIT, STATE_INITIALIZING] {
            table.initialization().states[idx].store(state, Ordering::Release);
            let namespace_published = state == STATE_INITIALIZING;
            if namespace_published {
                publish_initializing_namespace(_py, "builtins", builtins_bits);
            } else {
                assert_eq!(
                    detach_cache_publication(_py, "builtins", Some(builtins_bits)),
                    0
                );
                assert_eq!(table.initialization().slots[idx].load(Ordering::Acquire), 0);
                assert_eq!(
                    table.initialization().states[idx].load(Ordering::Acquire),
                    state
                );
                assert_eq!(initializer_owner(table, idx), owner);
            }
            assert_eq!(
                module_execution_owns_initializing_namespace(_py, "builtins", builtins_bits),
                namespace_published
            );
            assert_eq!(
                module_execution_owns_initializing_globals(_py, "builtins", dict_bits),
                namespace_published
            );
            if namespace_published {
                // The source-backed facade is the native CPython builtins
                // namespace, not an ordinary Python module to self-seed.
                for bits in [dict_bits, dict_bits] {
                    inc_ref_bits(_py, bits);
                }
                crate::builtins::frames::frame_stack_push_owned(_py, 0, dict_bits, dict_bits, 0);
                crate::builtins::frames::molt_frame_locals_set(dict_bits);
                assert!(!exception_pending(_py));
                let key = MoltObject::from_ptr(alloc_string(_py, b"__builtins__")).bits();
                assert!(
                    unsafe {
                        crate::dict_get_in_place(_py, dict_ptr, key)
                            .expect("dictionary lookup succeeds")
                    }
                    .is_none()
                );
                crate::builtins::frames::frame_stack_pop(_py);
                dec_ref_bits(_py, key);
            }
            for through_frame in [false, true] {
                let value = lookup_test_global(_py, builtins_bits, name_bits, through_frame);
                assert!(is_none_bits(value));
                assert!(
                    pending_exception_text(_py, crate::BuiltinExc::NameError).contains("NameError")
                );

                let unrelated = lookup_test_global(_py, unrelated_bits, name_bits, through_frame);
                assert!(is_none_bits(unrelated));
                assert!(
                    pending_exception_text(_py, crate::BuiltinExc::NameError).contains("NameError")
                );

                // Actual captured builtins values supply both the module
                // namespace and ordinary fallback lookup during bootstrap.
                let override_bits = MoltObject::from_int(42).bits();
                unsafe {
                    assert!(crate::object::ops::dict_set_in_place(
                        _py,
                        dict_ptr,
                        name_bits,
                        override_bits,
                    ))
                };
                let value = lookup_test_global(_py, builtins_bits, name_bits, through_frame);
                assert_eq!(value, override_bits);
                assert!(!exception_pending(_py));
                let cached = lookup_test_global(_py, unrelated_bits, name_bits, through_frame);
                assert_eq!(cached, override_bits);
                assert!(!exception_pending(_py));
                unsafe {
                    assert!(
                        crate::object::ops::dict_del_in_place(_py, dict_ptr, name_bits)
                            .expect("operation succeeds")
                    )
                };

                for module in [builtins_bits, unrelated_bits] {
                    let deleted = lookup_test_global(_py, module, name_bits, through_frame);
                    assert!(is_none_bits(deleted));
                    assert!(
                        pending_exception_text(_py, crate::BuiltinExc::NameError)
                            .contains("NameError")
                    );
                }

                let _owner_fixture_11 = TestExecutionOwner::set(
                    table,
                    idx,
                    owner.checked_add(1).expect("foreign thread id"),
                );
                assert!(!module_execution_owns_initializing_namespace(
                    _py,
                    "builtins",
                    builtins_bits
                ));
                assert!(!module_execution_owns_initializing_globals(
                    _py, "builtins", dict_bits,
                ));
                let foreign = lookup_test_global(_py, builtins_bits, name_bits, through_frame);
                let _owner_fixture_12 = TestExecutionOwner::set(table, idx, owner);
                assert!(is_none_bits(foreign));
                assert!(
                    pending_exception_text(_py, crate::BuiltinExc::NameError).contains("NameError")
                );
            }
        }
        // A replaced visible cache entry cannot borrow the old table
        // initializer's ownership, even on its thread and under its name.
        {
            inc_ref_bits(_py, unrelated_bits);
            let displaced = replace_module_owned_for_test(_py, "builtins", Some(unrelated_bits))
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
        assert!(!module_execution_owns_initializing_globals(
            _py,
            "builtins",
            unsafe { crate::module_dict_bits(unrelated_ptr) },
        ));
        for through_frame in [false, true] {
            let replacement = lookup_test_global(_py, unrelated_bits, name_bits, through_frame);
            assert!(is_none_bits(replacement));
            assert!(
                pending_exception_text(_py, crate::BuiltinExc::NameError).contains("NameError")
            );
        }
        {
            inc_ref_bits(_py, builtins_bits);
            let displaced = replace_module_owned_for_test(_py, "builtins", Some(builtins_bits))
                .expect("replacement builtins");
            dec_ref_bits(_py, displaced);
        }
        snapshot.restore();
        assert_eq!(
            table.initialization().states[idx].load(Ordering::Acquire),
            initial_state,
            "fresh execution restores the exact prior metadata state"
        );
        for state in [STATE_READY, STATE_TOMBSTONE] {
            if state == STATE_TOMBSTONE {
                module_table_view_tombstone(_py, builtins_id);
                // Keep a held namespace visible to exercise revocation,
                // without redefining absent-cache lazy startup policy.
                publish_for_test(_py, "builtins", builtins_bits);
            } else {
                table.initialization().states[idx].store(state, Ordering::Release);
            }
            // Even a stale owner field must not reopen a closed state.
            let _owner_fixture_13 = TestExecutionOwner::set(table, idx, owner);
            assert!(!module_execution_owns_initializing_namespace(
                _py,
                "builtins",
                builtins_bits
            ));
            assert!(!module_execution_owns_initializing_globals(
                _py, "builtins", dict_bits,
            ));
            for through_frame in [false, true] {
                let missing = lookup_test_global(_py, builtins_bits, name_bits, through_frame);
                assert!(is_none_bits(missing));
                assert!(
                    pending_exception_text(_py, crate::BuiltinExc::NameError).contains("NameError")
                );
            }
            let _owner_fixture_14 = TestExecutionOwner::set(table, idx, 0);
        }
        dec_ref_bits(_py, name_bits);
        dec_ref_bits(_py, builtins_bits);
        dec_ref_bits(_py, unrelated_bits);
    });
}

fn assert_g4_ensure_state_machine_transitions(public_cache: bool) {
    crate::with_gil_entry_nopanic!(_py, {
        let _ = crate::molt_exception_clear();
        let sys = public_cache.then(|| publish_test_module("sys", None));
        let modules_bits = module_dictionary(_py).unwrap();
        inc_ref_bits(_py, modules_bits);
        let modules = Some(crate::ptr_from_bits(modules_bits));
        let _modules_owner = crate::PtrDropGuard::new(crate::ptr_from_bits(modules_bits));
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
        let text = pending_exception_text(_py, crate::BuiltinExc::ValueError);
        assert!(text.contains("g4 init failure"), "{text}");
        // The row must be back to Uninit: a retry re-enters init and
        // fails identically instead of returning a phantom module.
        let retry = module_ensure(_py, fail_id);
        assert!(is_none_bits(retry));
        let text = pending_exception_text(_py, crate::BuiltinExc::ValueError);
        assert!(text.contains("g4 init failure"), "{text}");

        // ── Static-extension failure unwind: Initializing → Uninit ──
        let ext_fail_id = test_registry_id("g4_z_static_ext_fail");
        let ext_runs_before = EXT_FAIL_RUNS.load(Ordering::SeqCst);
        let ext_failed = module_ensure(_py, ext_fail_id);
        assert!(is_none_bits(ext_failed));
        let text = pending_exception_text(_py, crate::BuiltinExc::ImportError);
        assert!(text.contains("ImportError"), "{text}");
        assert!(
            text.contains("static-link PyModuleDef Py_mod_exec slot returned non-zero"),
            "{text}"
        );
        let table = module_table(_py).expect("table");
        assert_eq!(
            table.initialization().states[ext_fail_id as usize].load(Ordering::Acquire),
            STATE_UNINIT,
            "failed static extension init must unwind out of Initializing"
        );
        assert_eq!(
            initializer_owner(table, ext_fail_id as usize),
            0,
            "failed static extension init must release its ensure owner"
        );
        let ext_retry = module_ensure(_py, ext_fail_id);
        assert!(is_none_bits(ext_retry));
        let text = pending_exception_text(_py, crate::BuiltinExc::ImportError);
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
            publication_for_test(_py, "g4_alias").is_some(),
            "alias co-publication must reach the store under the alias name"
        );
        let table = module_table(_py).expect("table");
        assert_eq!(
            table.initialization().slots[target_id as usize].load(Ordering::Acquire),
            0,
            "completed alias target retains no initializer owner"
        );
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
        let text = pending_exception_text(_py, crate::BuiltinExc::ModuleNotFoundError);
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
        remove_for_test(_py, "g4_tomb");
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
        remove_for_test(_py, "g4_tomb_ext");
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
            unsafe {
                assert!(crate::dict_set_in_place(
                    _py,
                    modules,
                    src_name,
                    replacement
                ))
            };
        }
        module_table_view_replace(_py, src_id, replacement);
        let via_replace = module_ensure(_py, src_id);
        assert_eq!(
            via_replace, replacement,
            "Replaced(obj) must return the user object as-is (row 5.1)"
        );
        dec_ref_bits(_py, via_replace);
        if let Some(modules) = modules {
            unsafe {
                assert!(crate::dict_set_in_place(
                    _py,
                    modules,
                    src_name,
                    none_bits()
                ))
            };
        }
        module_table_view_replace(_py, src_id, none_bits());
        let halted = module_ensure(_py, src_id);
        assert!(is_none_bits(halted));
        let text = pending_exception_text(_py, crate::BuiltinExc::ModuleNotFoundError);
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
            table.initialization().states[src_id as usize].load(Ordering::Acquire),
            STATE_REPLACED,
            "view transition must not be overwritten by ensure"
        );

        // ── Dynamic dispatch: registry miss observes the canonical dictionary ──
        let _resolver = crate::test_support::EmptyImportResolver::new(_py);
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
    // Both bootstrap phases have the same state-machine contract. Fresh
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
        let text = pending_exception_text(_py, crate::BuiltinExc::ImportError);
        assert!(
            text.contains("static-link PyModuleDef Py_mod_exec slot returned non-zero"),
            "{text}"
        );

        let table = module_table(_py).expect("table");
        assert_eq!(
            table.initialization().states[ext_fail_id as usize].load(Ordering::Acquire),
            STATE_UNINIT,
            "failed static extension init must unwind the module row"
        );
        assert_eq!(
            initializer_owner(table, ext_fail_id as usize),
            0,
            "failed static extension init must release the ensure owner"
        );

        let retry = module_ensure(_py, ext_fail_id);
        assert!(is_none_bits(retry));
        let text = pending_exception_text(_py, crate::BuiltinExc::ImportError);
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
            // A plain module isolates dictionary custody from
            // molt_module_new's legitimate first intrinsic-registry role.
            let name = crate::attr_name_bits_from_bytes(py, b"g4_target").unwrap();
            let module = MoltObject::from_ptr(crate::alloc_module_obj(py, name)).bits();
            dec_ref_bits(py, name);
            let pointer = obj_from_bits(module).as_ptr().unwrap();
            let owners = || unsafe { (*crate::header_from_obj_ptr(pointer)).ref_count_snapshot() };
            assert_eq!(owners(), 1, "isolated creator before public publication");
            let death = crate::object::weakref::WeakBorrow::new(py, module).unwrap();
            publish_for_test(py, "g4_target", module);
            assert_eq!(owners(), 2, "creator and one public dictionary entry");
            // Alias membership is an actual dictionary publication, not
            // initialization metadata or a second retaining slot.
            publish_for_test(py, "g4_alias", module);
            assert_eq!(owners(), 3, "the alias adds exactly one dictionary edge");
            let target = test_registry_id("g4_target");
            let alias = test_registry_id("g4_alias");
            module_table_view_replace(py, target, module);
            module_table_view_replace(py, alias, module);
            assert_eq!(owners(), 3, "ready metadata retains no module owners");
            let roots =
                take_module_roots_for_shutdown(py, runtime_state(py), |name, _| name == "builtins");
            assert_eq!(roots.iter().filter(|&&bits| bits == module).count(), 2);
            assert_eq!(owners(), 3, "both detached dictionary edges remain owned");
            for bits in roots {
                dec_ref_bits(py, bits);
            }
            assert_eq!(owners(), 1, "retirement releases both public alias owners");
            for name in ["g4_target", "g4_alias"] {
                assert!(module_lookup(py, name).unwrap().is_none());
            }
            let table = module_table(py).unwrap();
            assert_eq!(
                table.initialization().slots[target as usize].load(Ordering::Acquire),
                0
            );
            assert_eq!(
                table.initialization().slots[alias as usize].load(Ordering::Acquire),
                0
            );
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
            assert!(execute_private_body(py, "g4_target").is_err());
            crate::clear_exception(py);
            module_table_view_replace(py, target, module);
            module_table_view_tombstone(py, target);
            assert_eq!(
                table.initialization().states[target as usize].load(Ordering::Acquire),
                STATE_RETIRED
            );
            assert_eq!(
                owners(),
                1,
                "retired admissions cannot add retaining owners"
            );
            dec_ref_bits(py, module);
            assert!(
                death.upgrade_owned().is_none(),
                "the actual module is destroyed after its last creator owner is released"
            );
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
    blob[8..12].copy_from_slice(&2u32.to_le_bytes());
    let err = ModuleRegistry::parse(blob.as_ptr()).expect_err("old schema must fail");
    assert!(err.contains("schema"), "{err}");
    blob[0..8].copy_from_slice(b"MOLTMOD2");
    let err = ModuleRegistry::parse(blob.as_ptr()).expect_err("old magic must fail");
    assert!(err.contains("magic"), "{err}");

    // The byte is a closed protocol: every valid combination is admitted,
    // but no unknown bit can silently alter provider/body interpretation.
    let mut flags = BlobBuilder::new();
    flags.row("zz_flags", 0, None, None, MODULE_KIND_SOURCE, 0);
    let mut blob = flags.build();
    let flags_offset = MODULE_REGISTRY_HEADER_BYTES + 25;
    for value in 0u8..=u8::MAX {
        blob[flags_offset] = value;
        let parsed = ModuleRegistry::parse(blob.as_ptr());
        assert_eq!(
            parsed.is_ok(),
            value & !MODULE_FLAG_KNOWN_MASK == 0,
            "flags 0x{value:02x}: {parsed:?}"
        );
    }

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
        const RESOLVE_ITERS: usize = 2_000_000;
        const ENSURE_ITERS: usize = 250_000;

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
        eprintln!(
            "IMPORT_CATALOG_ATTESTATION {{\"resolve_ns_per_op\":{resolve_ns:.3},\
                 \"ready_ensure_ns_per_op\":{ensure_ns:.3},\
                 \"parallel_8x_resolve_ns_per_op\":{parallel_resolve_ns:.3},\
                 \"resolver_allocations\":{},\"ready_ensure_allocations\":{}}}",
            resolve_alloc.allocations, ensure_alloc.allocations,
        );
    });
}
