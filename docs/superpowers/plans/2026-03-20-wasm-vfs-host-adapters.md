# WASM VFS Host Adapters Implementation Plan (Plan B)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Connect the VFS core (Plan A) to host runtimes — wasmtime injects mounts at instantiation, the module import system resolves from `/bundle`, and `open()` routes through VFS on WASM targets.

**Architecture:** The wasmtime host (`molt-wasm-host`) configures VFS mounts via new CLI flags (`--bundle`, `--vfs-tmp-quota`). The runtime's `open()` and module resolver are patched to dispatch through VFS when active. A `--vfs` flag on the host enables VFS mode.

**Tech Stack:** Rust (molt-wasm-host, molt-runtime)

**Depends on:** Plan A (VFS Core) — completed

---

## File Structure

### Modified Files
- `runtime/molt-wasm-host/src/main.rs` — Add VFS mount setup, `--bundle` flag, inject VFS into runtime state
- `runtime/molt-wasm-host/Cargo.toml` — Add `tar` dependency
- `runtime/molt-runtime/src/builtins/io.rs` — Route `open()` through VFS on WASM
- `runtime/molt-runtime/src/builtins/modules.rs` — Module resolution via VFS `/bundle`
- `runtime/molt-runtime/src/state.rs` — Add VFS state to runtime state struct

---

### Task 1: Add VFS State to Runtime

**Files:**
- Modify: `runtime/molt-runtime/src/state.rs`
- Modify: `runtime/molt-runtime/src/vfs/mod.rs`

- [ ] **Step 1: Add VfsState to runtime state**

Search for the runtime state struct in `state.rs`. Add an `Option<VfsState>` field. This is how the host injects the VFS into the runtime — if `Some`, all filesystem ops route through VFS.

Also add a global accessor function `get_vfs()` that returns `Option<&VfsState>` from the runtime state, callable from io.rs and modules.rs.

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p molt-runtime 2>&1 | tail -5`

- [ ] **Step 3: Commit**

```bash
git commit -m "feat(vfs): add VfsState to runtime state struct"
```

---

### Task 2: Route open() Through VFS

**Files:**
- Modify: `runtime/molt-runtime/src/builtins/io.rs:3678`

- [ ] **Step 1: Add VFS dispatch before std::fs open**

At the `open_impl` function (~line 3678), before `mode_info.options.open(&path)`, add VFS dispatch:

```rust
// Check VFS first (active on WASM targets with VFS configured)
if let Some(vfs_state) = get_vfs() {
    let path_str = path.to_string_lossy();
    if let Some((mount_prefix, backend, rel_path)) = vfs_state.resolve(&path_str) {
        // Capability check
        let is_write = mode_info.writable;
        if let Err(e) = crate::vfs::caps::check_mount_capability(
            &mount_prefix, is_write, &|cap| has_capability(_py, cap),
        ) {
            let exc_type = match e {
                VfsError::CapabilityDenied(_) => "PermissionError",
                VfsError::ReadOnly => "PermissionError",
                _ => "OSError",
            };
            return raise_exception::<_>(_py, exc_type, &e.to_string());
        }
        // Open through VFS file handle
        let vfs_file = if mode_info.writable && mode_info.readable {
            // r+ mode: read existing, allow writes
            MoltVfsFile::open_append(backend, &rel_path)
        } else if mode_info.writable {
            if mode_info.append {
                MoltVfsFile::open_append(backend, &rel_path)
            } else {
                MoltVfsFile::open_write(backend, &rel_path)
            }
        } else {
            MoltVfsFile::open_read(backend, &rel_path)
        };
        match vfs_file {
            Ok(f) => {
                // Wrap MoltVfsFile in the runtime's file object
                // Store in a thread-local or runtime-managed file table
                // Return the file handle bits
                todo!("wrap MoltVfsFile in runtime file object");
            }
            Err(e) => {
                let exc = match e {
                    VfsError::NotFound => "FileNotFoundError",
                    VfsError::IsDirectory => "IsADirectoryError",
                    VfsError::ReadOnly | VfsError::PermissionDenied => "PermissionError",
                    VfsError::QuotaExceeded => "OSError",
                    _ => "OSError",
                };
                return raise_exception::<_>(_py, exc, &format!("{e}: '{}'", path.display()));
            }
        }
    }
}
// Fallback to std::fs
```

Note: The `todo!()` for wrapping MoltVfsFile in a runtime file object is the integration point. The runtime represents files as NaN-boxed handles. The host needs to provide a file-handle registration mechanism. For v0.1, the simplest approach is to store VFS file data as a bytes object and return a BytesIO/StringIO wrapper.

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p molt-runtime 2>&1 | tail -5`

- [ ] **Step 3: Commit**

```bash
git commit -m "feat(vfs): route open() through VFS with capability checks"
```

---

### Task 3: Module Resolution via VFS

**Files:**
- Modify: `runtime/molt-runtime/src/builtins/modules.rs:1082-1135`

- [ ] **Step 1: Add VFS-aware file existence helpers**

Add helper functions that check VFS before std::fs:

```rust
fn vfs_is_file(path: &std::path::Path) -> bool {
    if let Some(vfs) = get_vfs() {
        let p = path.to_string_lossy();
        if let Some((_, backend, rel)) = vfs.resolve(&p) {
            return backend.stat(&rel).map(|s| s.is_file).unwrap_or(false);
        }
    }
    path.is_file()
}

fn vfs_is_dir(path: &std::path::Path) -> bool {
    if let Some(vfs) = get_vfs() {
        let p = path.to_string_lossy();
        if let Some((_, backend, rel)) = vfs.resolve(&p) {
            return backend.stat(&rel).map(|s| s.is_dir).unwrap_or(false);
        }
    }
    path.is_dir()
}

fn vfs_read_to_string(path: &std::path::Path) -> Option<String> {
    if let Some(vfs) = get_vfs() {
        let p = path.to_string_lossy();
        if let Some((_, backend, rel)) = vfs.resolve(&p) {
            return backend.open_read(&rel).ok()
                .and_then(|bytes| String::from_utf8(bytes).ok());
        }
    }
    std::fs::read_to_string(path).ok()
}
```

- [ ] **Step 2: Replace all is_file/is_dir calls in runpy_resolve_module_source**

Replace every `path.is_file()` with `vfs_is_file(&path)` and every `path.is_dir()` with `vfs_is_dir(&path)` in the module resolution function.

- [ ] **Step 3: Prepend /bundle to sys.path on VFS-enabled builds**

In the module initialization, when VFS is active, prepend `/bundle` to `sys.path`:

```rust
if get_vfs().is_some() {
    // Prepend /bundle to sys.path for VFS module resolution
    sys_path.insert(0, "/bundle".to_string());
}
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p molt-runtime 2>&1 | tail -5`

- [ ] **Step 5: Commit**

```bash
git commit -m "feat(vfs): VFS-aware module resolution with /bundle sys.path"
```

---

### Task 4: Wasmtime Host VFS Setup

**Files:**
- Modify: `runtime/molt-wasm-host/src/main.rs`
- Modify: `runtime/molt-wasm-host/Cargo.toml`

- [ ] **Step 1: Add --bundle and --vfs flags to host CLI**

In the argument parsing section of main.rs, add:
- `--bundle <path>` — path to bundle.tar for `/bundle` mount
- `--vfs-tmp-quota <MB>` — TmpFs quota in MB (default 64)
- `--vfs` — enable VFS mode (auto-enabled when --bundle is set)

- [ ] **Step 2: Add VFS mount initialization**

After WASI context setup, before module instantiation:

```rust
if let Some(bundle_path) = &args.bundle {
    let mut mount_table = MountTable::new();

    // /bundle from tar archive
    let tar_bytes = std::fs::read(bundle_path)?;
    let bundle_fs = BundleFs::from_tar(&tar_bytes)
        .map_err(|e| anyhow::anyhow!("Failed to load bundle: {e}"))?;
    mount_table.add_mount("/bundle", Arc::new(bundle_fs));

    // /tmp in-memory
    mount_table.add_mount("/tmp", Arc::new(TmpFs::new(args.vfs_tmp_quota)));

    // /dev pseudo-devices
    mount_table.add_mount("/dev", Arc::new(DevFs::new()));

    // Inject VFS into runtime state via host import
    store.data_mut().vfs = Some(VfsState::from_table(mount_table));
}
```

- [ ] **Step 3: Add tar dependency to Cargo.toml**

```toml
[dependencies]
tar = "0.4"
```

Also add `vfs_bundle_tar` feature to molt-runtime's Cargo.toml and enable it in the host.

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p molt-wasm-host 2>&1 | tail -5`

- [ ] **Step 5: Commit**

```bash
git commit -m "feat(vfs): wasmtime host VFS mount setup with --bundle flag"
```

---

### Task 5: Integration Test

**Files:**
- Create: `tests/test_wasm_vfs_integration.py`

- [ ] **Step 1: Write VFS integration test**

```python
@pytest.mark.slow
def test_vfs_bundle_import(tmp_path):
    """A bundled Python module should be importable from /bundle."""
    # Create bundle with a module
    bundle_dir = tmp_path / "bundle"
    bundle_dir.mkdir()
    (bundle_dir / "mymod.py").write_text("VALUE = 42\n")
    (bundle_dir / "main.py").write_text("import mymod\nprint(mymod.VALUE)\n")

    # Create tar bundle
    import tarfile
    bundle_tar = tmp_path / "bundle.tar"
    with tarfile.open(bundle_tar, "w") as tar:
        tar.add(bundle_dir / "mymod.py", arcname="mymod.py")
        tar.add(bundle_dir / "main.py", arcname="main.py")

    # Build and run with VFS
    # (requires wasmtime host --bundle support)
    ...
```

- [ ] **Step 2: Commit**

```bash
git commit -m "test(vfs): add VFS integration test for bundled module import"
```

---

## Execution Notes

- Task 1 must complete before Tasks 2-4 (they depend on VFS state access)
- Tasks 2 and 3 can run in parallel (different files in molt-runtime)
- Task 4 depends on Task 1 (host needs VFS types)
- Task 5 depends on all previous tasks
