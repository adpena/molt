use super::*;

// Independent SHA256 vectors for the literal bytes, not this loader's hash.
const SYMBOLS: &[u8] = b"molt_init_sys\nmolt_main\n";
const DIGEST: &str = "cd85e5b7627ffa5a6821fde44e31407d23a4d3da80a6e07a5b675af8d0925784";

#[test]
fn admits_exact_bytes_before_parsing() {
    let expected = BTreeSet::from(["molt_init_sys".to_string(), "molt_main".to_string()]);
    assert_eq!(
        parse_admitted_callable_symbols(SYMBOLS, DIGEST).unwrap(),
        expected
    );
    // Same parsed symbols are insufficient: even the newline is bound.
    assert!(
        parse_admitted_callable_symbols(&SYMBOLS[..SYMBOLS.len() - 1], DIGEST)
            .unwrap_err()
            .contains("SHA256 mismatch")
    );
    assert!(
        parse_admitted_callable_symbols(b"molt_changed\nmolt_main\n", DIGEST)
            .unwrap_err()
            .contains("SHA256 mismatch")
    );
}

#[test]
fn rejects_incomplete_or_malformed_admission() {
    assert!(load_runtime_callable_symbols(None, None).unwrap().is_none());
    assert!(load_runtime_callable_symbols(Some(OsStr::new("runtime.txt")), None).is_err());
    assert!(load_runtime_callable_symbols(None, Some(OsStr::new(DIGEST))).is_err());
    for digest in [
        String::new(),
        "abc".to_string(),
        "g".repeat(64),
        DIGEST.to_uppercase(),
    ] {
        assert!(
            parse_admitted_callable_symbols(SYMBOLS, &digest)
                .unwrap_err()
                .contains("lowercase SHA256")
        );
    }
}

#[test]
fn rejects_verified_empty_or_invalid_utf8_input() {
    assert!(
        parse_admitted_callable_symbols(
            b"",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        )
        .unwrap_err()
        .contains("empty")
    );
    assert!(
        parse_admitted_callable_symbols(
            b"\xff",
            "a8100ae6aa1940d0b663bb31cd466142ebbdbd5187131b92d93818987832eb89"
        )
        .unwrap_err()
        .contains("UTF-8")
    );
}

#[test]
fn selected_path_never_falls_back_after_replacement_or_removal() {
    let path = std::env::temp_dir().join(format!(
        "molt-ir-callable-admission-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&path, SYMBOLS).unwrap();
    assert!(
        load_runtime_callable_symbols(Some(path.as_os_str()), Some(OsStr::new(DIGEST)))
            .unwrap()
            .unwrap()
            .contains("molt_main")
    );
    std::fs::write(&path, b"molt_changed\n").unwrap();
    assert!(
        load_runtime_callable_symbols(Some(path.as_os_str()), Some(OsStr::new(DIGEST)))
            .unwrap_err()
            .contains("SHA256 mismatch")
    );
    std::fs::remove_file(&path).unwrap();
    assert!(
        load_runtime_callable_symbols(Some(path.as_os_str()), Some(OsStr::new(DIGEST)))
            .unwrap_err()
            .contains("cannot read")
    );
}

struct AdmittedFile(std::path::PathBuf);
impl AdmittedFile {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        loop {
            let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "molt-callable-snapshot-{}-{sequence}",
                std::process::id()
            ));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    std::io::Write::write_all(&mut file, SYMBOLS).unwrap();
                    return Self(path);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create independent callable fixture: {error}"),
            }
        }
    }
    fn environment(&self) -> Arc<backend_env::BackendEnvironment> {
        let payload = serde_json::json!({
            "MOLT_RUNTIME_CALLABLE_SYMBOLS": self.0.to_str().unwrap(),
            "MOLT_RUNTIME_CALLABLE_SYMBOLS_SHA256": DIGEST,
        });
        Arc::new(
            backend_env::BackendEnvironment::from_request(payload.as_object().unwrap()).unwrap(),
        )
    }
}
impl Drop for AdmittedFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn request_workers_share_admission_and_next_request_readmits_bytes() {
    let file = AdmittedFile::new();
    let environment = file.environment();
    let scope = backend_env::enter(Arc::clone(&environment));
    let first = RuntimeCallableSymbols::capture();
    let expected = BTreeSet::from(["molt_init_sys".to_owned(), "molt_main".to_owned()]);
    assert_eq!(first.symbols(), Some(&expected));
    assert!(matches!(first.required_symbols(), Cow::Borrowed(_)));
    let carried = backend_env::carry();
    let worker = std::thread::spawn(move || carried.enter(RuntimeCallableSymbols::capture))
        .join()
        .unwrap();
    assert!(Arc::ptr_eq(&first, &worker));
    assert!(Arc::ptr_eq(
        &first.shared_symbols().unwrap(),
        &worker.shared_symbols().unwrap()
    ));
    // Already-admitted bytes remain usable without a path. This distinguishes
    // sharing the immutable buffer from repeated reads or a path-keyed cache.
    std::fs::remove_file(&file.0).unwrap();
    for _ in 0..64 {
        let again = RuntimeCallableSymbols::capture();
        assert!(Arc::ptr_eq(&first, &again));
        assert_eq!(again.symbols(), Some(&expected));
    }
    drop(scope);
    let _next = backend_env::enter(file.environment());
    assert!(std::panic::catch_unwind(RuntimeCallableSymbols::capture).is_err());
}

#[test]
fn nested_request_restores_snapshot_without_global_retention() {
    let file = AdmittedFile::new();
    let weak;
    {
        let environment = file.environment();
        let _scope = backend_env::enter(Arc::clone(&environment));
        let outer = RuntimeCallableSymbols::capture();
        weak = Arc::downgrade(&outer);
        {
            let _inner = backend_env::enter(file.environment());
            let inner = RuntimeCallableSymbols::capture();
            assert!(!Arc::ptr_eq(&inner, &outer));
            assert_eq!(inner.symbols(), outer.symbols());
        }
        assert!(Arc::ptr_eq(&outer, &RuntimeCallableSymbols::capture()));
    }
    assert!(
        weak.upgrade().is_none(),
        "request snapshot leaked beyond its owners"
    );
}

#[test]
fn optional_absence_is_distinct_from_invalid_selected_input() {
    let absent = RuntimeCallableSymbols::load(|_| None).unwrap();
    assert!(absent.symbols().is_none());
    assert!(absent.shared_symbols().is_none());
    // Preserve the existing molt-ir-only cfg(test) empty resolver contract.
    assert!(absent.required_symbols().is_empty());
    let invalid = RuntimeCallableSymbols::load(|key| {
        (key == BackendEnv::MOLT_RUNTIME_CALLABLE_SYMBOLS).then(|| "missing.txt".into())
    });
    assert!(invalid.is_err());
}

#[test]
fn racing_workers_receive_the_same_request_snapshot() {
    let file = AdmittedFile::new();
    let environment = file.environment();
    let _scope = backend_env::enter(environment);
    let carried = backend_env::carry();
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let carried = carried.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                carried.enter(|| {
                    barrier.wait();
                    RuntimeCallableSymbols::capture()
                })
            })
        })
        .collect();
    barrier.wait();
    let snapshots: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert!(Arc::ptr_eq(&snapshots[0], &snapshots[1]));
    assert!(snapshots[0].symbols().unwrap().contains("molt_main"));
}
