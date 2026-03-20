# Molt WASM Virtual Filesystem & Edge Deployment

## Overview

A mount-oriented virtual filesystem that enables Molt programs to run on Cloudflare Workers, Fastly Compute, Deno Deploy, browsers, and WASI runtimes with full `open()`, `pathlib`, and `importlib` support — without emulating a real OS. Packaged Python modules and resources live in `/bundle`, scratch space in `/tmp`, and stdio maps to host logging. Capabilities gate every operation.

**Goal:** `molt build --target wasm --bundle src/ --profile cloudflare` produces a single deployable artifact that cold-starts in <50ms on Cloudflare Workers with full module import, file I/O for bundled resources, and temp file support.

**Non-goals:** Full POSIX filesystem, pip install, arbitrary host filesystem access, persistent storage in v0.1, D1/R2/KV/Cache as generic path trees (these are schema-first host services per spec 0968 Section 5.2).

---

## Architecture

```
Python code
    |
    v
open("/bundle/data.csv", "r")
    |
    v
+-----------------------------+
|  VFS Layer (molt-runtime)   |
|  +------------------------+ |
|  |  MountTable            | |
|  |  /bundle -> BundleFs   | |
|  |  /tmp    -> TmpFs      | |
|  |  /dev/*  -> DevFs      | |
|  |  /state  -> StateFs    | |
|  +------------------------+ |
|  Capability enforcement     |
|  Path normalization         |
|  Traversal protection       |
+-----------------------------+
    |
    v
+-----------------------------+
|  VfsBackend trait            |
|  open / read / write / stat |
|  readdir / mkdir / unlink   |
|  rename / exists            |
+-----------------------------+
    |
    v
+-----------+-----------+------------+-----------+
| BundleFs  |  TmpFs    |  DevFs     | WasiPassFs|
| (ROM)     | (RAM)     | (streams)  | (preopens)|
| HashMap   | RwLock    | buffers    | fd_read   |
+-----------+-----------+------------+-----------+
```

---

## Layer 1: VFS Core (`runtime/molt-runtime/src/vfs/`)

### 1.1 Module Structure

```
runtime/molt-runtime/src/vfs/
+-- mod.rs          # VfsBackend trait, MountTable, VfsError, path resolution
+-- bundle.rs       # BundleFs -- read-only in-memory filesystem from packaged data
+-- tmp.rs          # TmpFs -- ephemeral read-write in-memory filesystem
+-- dev.rs          # DevFs -- /dev/stdin, /dev/stdout, /dev/stderr pseudo-devices
+-- state.rs        # StateFs -- host-delegated persistent storage (stub in v0.1)
+-- wasi_pass.rs    # WasiPassthroughFs -- delegates to real WASI preopens
+-- caps.rs         # Capability constants and mount-to-capability mapping
+-- file.rs         # MoltVfsFile -- file handle wrapper bridging VFS to runtime
```

### 1.2 VfsBackend Trait

```rust
pub enum VfsError {
    NotFound,
    PermissionDenied,
    ReadOnly,
    IsDirectory,
    NotDirectory,
    AlreadyExists,
    QuotaExceeded,
    SeekNotSupported,
    IoError(String),
    CapabilityDenied(String),
}

pub struct VfsStat {
    pub is_file: bool,
    pub is_dir: bool,
    pub size: u64,
    pub readonly: bool,
    pub mtime: u64,  // seconds since epoch; 0 for BundleFs (deterministic)
}

pub trait VfsBackend: Send + Sync {
    fn open_read(&self, path: &str) -> Result<Vec<u8>, VfsError>;
    fn open_write(&self, path: &str, data: &[u8]) -> Result<(), VfsError>;
    fn open_append(&self, path: &str, data: &[u8]) -> Result<(), VfsError>;
    fn stat(&self, path: &str) -> Result<VfsStat, VfsError>;
    fn readdir(&self, path: &str) -> Result<Vec<String>, VfsError>;
    fn mkdir(&self, path: &str) -> Result<(), VfsError>;
    fn unlink(&self, path: &str) -> Result<(), VfsError>;
    fn rename(&self, from: &str, to: &str) -> Result<(), VfsError>;
    fn exists(&self, path: &str) -> bool;
    fn is_readonly(&self) -> bool;
}
```

**Limitations (v0.1):** Streaming/seek file I/O is not supported. `open_read` returns the entire file content as `Vec<u8>`. `open_write` is atomic full-file replacement. Attempts to seek raise `VfsError::SeekNotSupported`. This is sufficient for `importlib.resources`, `json.load`, `csv.reader`, and other common patterns. Full streaming support is a v0.2 goal.

### 1.3 MountTable

```rust
pub struct MountTable {
    mounts: Vec<(String, Box<dyn VfsBackend>)>,  // sorted longest-prefix-first
}

impl MountTable {
    pub fn resolve(&self, path: &str) -> Option<(&str, &dyn VfsBackend, &str)> {
        // Returns (mount_prefix, backend, relative_path)
        // Path traversal protection: reject ".." that escapes mount root
    }
    pub fn add_mount(&mut self, prefix: &str, backend: Box<dyn VfsBackend>) { ... }
}
```

Path resolution:
1. Normalize path (collapse `//`, resolve `.`)
2. Reject `..` that would escape mount root
3. Reject empty path and bare `/` (no root-level operations)
4. Match longest prefix in mount table
5. Check capability for the operation (read/write) against the mount
6. Delegate to backend with relative path

### 1.4 BundleFs (Read-Only In-Memory)

```rust
pub struct BundleFs {
    files: HashMap<String, Vec<u8>>,      // path -> content
    dirs: HashSet<String>,                 // known directory paths
}

impl BundleFs {
    pub fn from_entries(entries: Vec<(String, Vec<u8>)>) -> Self { ... }
    pub fn from_tar(tar_bytes: &[u8]) -> Self {
        // Materialize entire tar into HashMap at init time.
        // Tar bytes can be dropped after materialization to save memory.
        // SECURITY: Reject symlinks in tar entries (traversal escape vector).
        // SECURITY: Reject paths containing ".." components.
    }
}
```

- Populated at module instantiation; tar bytes dropped after materialization
- All writes return `VfsError::ReadOnly`
- `stat()` returns `readonly: true`, `mtime: 0` (deterministic)
- Deterministic iteration order (sorted keys)

### 1.5 TmpFs (Ephemeral Read-Write In-Memory)

```rust
pub struct TmpFs {
    files: RwLock<HashMap<String, Vec<u8>>>,
    dirs: RwLock<HashSet<String>>,
    quota_bytes: usize,                    // max total bytes (default 64 MB)
    used_bytes: AtomicUsize,
}
```

- Uses `RwLock` (not `RefCell`) — satisfies `Send + Sync` trait bound
- Created empty at startup
- Enforces quota; exceeding it returns `VfsError::QuotaExceeded`
- Configurable via `MOLT_TMP_QUOTA_MB`
- Not durable across requests/restarts
- Concurrent writes to the same file are last-writer-wins (no advisory locking per spec Section 4)
- `stat()` returns `mtime` from `std::time::SystemTime` (or 0 on freestanding)

### 1.6 DevFs (Pseudo-Devices)

```rust
pub struct DevFs {
    stdout_buffer: Mutex<Vec<u8>>,
    stderr_buffer: Mutex<Vec<u8>>,
    stdin_data: Vec<u8>,  // set at init from host, empty by default
}
```

- `/dev/stdout` -> write-only, content forwarded to host logging
- `/dev/stderr` -> write-only, content forwarded to host logging
- `/dev/stdin` -> read-only, content from host request body (if provided)
- Flush triggered at request completion or explicit `flush()`
- Always accessible (no capability gate per spec Section 2.1)

### 1.7 WasiPassthroughFs (for `wasm_wasi` profile)

For generic WASI runtimes (not Cloudflare/browser), the VFS can delegate to real WASI preopens:

```rust
pub struct WasiPassthroughFs {
    base_dir: String,  // the preopened directory path
}
```

This backend implements `VfsBackend` by calling `std::fs` operations scoped to `base_dir`. When a WASI host preopens `/data` as a read-only directory, the VFS maps it as:
- `mount_table.add_mount("/bundle", WasiPassthroughFs::new("/data"))`

Capabilities for WASI hosts are communicated via `MOLT_CAPABILITIES` environment variable (the existing mechanism), which WASI runtimes can set via `--env` flags.

### 1.8 Capability Mapping

```rust
const MOUNT_CAPABILITIES: &[(&str, &str, &str)] = &[
    // (mount_prefix, read_cap, write_cap)
    ("/bundle", "fs.bundle.read", ""),           // no write cap = never writable
    ("/tmp",    "fs.tmp.read",    "fs.tmp.write"),
    ("/state",  "fs.state.read",  "fs.state.write"),
    ("/dev",    "",               ""),            // always accessible
];
```

Capability denials produce structured diagnostics visible in stderr/logs:
```
[molt-vfs] PermissionError: operation requires 'fs.tmp.write' capability
  path: /tmp/scratch.txt
  mount: /tmp
  hint: set MOLT_CAPABILITIES=fs.tmp.write or add to host profile
```

### 1.9 MoltVfsFile — File Handle Bridge

```rust
pub struct MoltVfsFile {
    content: Vec<u8>,
    cursor: usize,
    path: String,
    mode: FileMode,
    mount_prefix: String,
}
```

Bridges VFS `Vec<u8>` reads/writes with the runtime's file object expectations:
- `read(n)` reads from cursor position
- `write(data)` appends to content buffer (flushed to TmpFs on close)
- `seek()` returns `VfsError::SeekNotSupported` in v0.1
- `close()` flushes pending writes back to the VFS backend
- `fileno()` returns -1 (no real fd)

Integrates with `open_impl` in `io.rs`: when VFS is active, `open()` returns a `MoltVfsFile` instead of a `std::fs::File`. The runtime file object wrapper detects the type and dispatches accordingly.

Also integrates with `molt_os_open` (line 6423 of io.rs) on WASM targets — this path also routes through VFS.

### 1.10 Module Import Integration

Update `runpy_resolve_module_source` to intercept ALL `is_file()` / `is_dir()` calls:

```rust
fn vfs_is_file(path: &Path) -> bool {
    if let Some(vfs) = get_vfs() {
        let p = path.to_str().unwrap_or("");
        if let Ok(stat) = vfs.stat(p) {
            return stat.is_file;
        }
        return false;
    }
    path.is_file()  // fallback for native builds
}

fn vfs_is_dir(path: &Path) -> bool {
    if let Some(vfs) = get_vfs() {
        let p = path.to_str().unwrap_or("");
        if let Ok(stat) = vfs.stat(p) {
            return stat.is_dir;
        }
        return false;
    }
    path.is_dir()
}
```

Replace every `path.is_file()` and `path.is_dir()` in `runpy_resolve_module_source` (lines 1101, 1111, 1113, 1124 of modules.rs) with these VFS-aware helpers.

When VFS is active, `sys.path` is prepended with `/bundle`:
```python
sys.path = ["/bundle", "/bundle/lib"]
```

---

## Layer 2: Host Adapters

### 2.1 Wasmtime Host (`molt-wasm-host`)

**Mount setup at instantiation:**

```rust
fn setup_vfs_mounts(store: &mut Store<HostState>, config: &VfsConfig) -> Result<()> {
    let mut mount_table = MountTable::new();

    // /bundle: populated from sidecar .tar or embedded data segment
    if let Some(bundle_path) = &config.bundle_path {
        let tar_bytes = std::fs::read(bundle_path)?;
        mount_table.add_mount("/bundle", Box::new(BundleFs::from_tar(&tar_bytes)));
    }

    // /tmp: in-memory with quota
    mount_table.add_mount("/tmp", Box::new(TmpFs::new(config.tmp_quota_mb)));

    // /dev: pseudo-devices
    mount_table.add_mount("/dev", Box::new(DevFs::new()));

    store.data_mut().vfs = Some(mount_table);
    Ok(())
}
```

For the `wasm_wasi` profile, WASI preopens are translated into mount entries:
```rust
if config.profile == "wasm_wasi" {
    // Map WASI preopens to VFS mounts via WasiPassthroughFs
    for (guest_path, host_path) in &config.preopens {
        mount_table.add_mount(guest_path, Box::new(WasiPassthroughFs::new(host_path)));
    }
}
```

### 2.2 Browser Host (JS)

```javascript
class MoltVfs {
    constructor() {
        this.mounts = new Map();
    }

    addBundleMount(files) {
        // files: Map<string, Uint8Array> from fetch() of bundle archive
        this.mounts.set('/bundle', new BundleFs(files));
    }

    addTmpMount(quotaBytes = 64 * 1024 * 1024) {
        this.mounts.set('/tmp', new TmpFs(quotaBytes));
    }

    addDevMount() {
        this.mounts.set('/dev', new DevFs());
    }

    // Called by WASM imports
    vfs_open_read(pathPtr, pathLen) { ... }
    vfs_open_write(pathPtr, pathLen, dataPtr, dataLen) { ... }
    vfs_stat(pathPtr, pathLen) { ... }
    vfs_readdir(pathPtr, pathLen) { ... }
}
```

### 2.3 Cloudflare Worker Host

```javascript
// worker.js -- Cloudflare Worker entry point
import { MoltRuntime } from './molt-wasm-host.js';

export default {
    async fetch(request, env, ctx) {
        const runtime = new MoltRuntime();

        // /bundle from Worker bundle (static assets)
        runtime.vfs.addBundleMount(BUNDLE_FILES);

        // /tmp from Worker memory (32 MB quota for Workers)
        runtime.vfs.addTmpMount(32 * 1024 * 1024);

        // /dev pseudo-devices
        runtime.vfs.addDevMount();

        // Set capabilities for Cloudflare profile
        runtime.setCapabilities([
            'fs.bundle.read',
            'fs.tmp.read', 'fs.tmp.write',
            'http.fetch',
        ]);

        // Set stdin from request body
        const body = await request.arrayBuffer();
        runtime.vfs.setStdin(new Uint8Array(body));

        // Run
        const result = await runtime.call('handle_request');

        // Collect stdout as response
        return new Response(runtime.vfs.getStdout());
    }
};
```

**D1/R2/KV/Cache:** Per spec Section 5.2, these are NOT exposed as filesystem paths. They remain explicit schema-first host services accessed through dedicated `molt_kv_*`, `molt_object_*`, `molt_cache_*` host imports (reserved capability families from spec Section 6.4).

---

## Layer 3: Packaging (CLI)

### 3.1 Bundle Format

The bundle is a **tar archive** (uncompressed) fully materialized into a `HashMap` at init time. The tar bytes are dropped after materialization to avoid doubling memory usage.

```
bundle.tar
+-- __manifest__.json    # file list, hashes, metadata
+-- main.py              # entry point
+-- mymodule/
|   +-- __init__.py
|   +-- utils.py
+-- data/
|   +-- config.csv
+-- requirements.txt     # metadata only, no pip install
```

**Security:** `from_tar` rejects symlinks and paths containing `..` to prevent traversal attacks.

### 3.2 CLI Commands

```bash
# Build for Cloudflare Workers with bundled source
molt build app.py --target wasm --bundle src/ --profile cloudflare \
    --output dist/worker.wasm --linked-output dist/worker_linked.wasm

# Build bundle separately
molt bundle src/ --output dist/bundle.tar

# Build for browser
molt build app.py --target wasm --bundle src/ --profile browser

# Build for generic WASI
molt build app.py --target wasm --bundle src/ --profile wasi

# Build for Fastly Compute
molt build app.py --target wasm --bundle src/ --profile fastly
```

### 3.3 `--profile` Configurations

| Profile | wasm-opt | wasm-profile | Precompile | Default Capabilities | Tmp Quota |
|---------|----------|-------------|-----------|---------------------|-----------|
| `cloudflare` | Oz | pure | Yes (.cwasm) | fs.bundle.read, fs.tmp.* , http.fetch | 32 MB |
| `browser` | Oz | pure | No | fs.bundle.read, fs.tmp.* | 64 MB |
| `wasi` | O3 | full | Optional | host-defined | 256 MB |
| `fastly` | Oz | pure | Yes | fs.bundle.read, fs.tmp.*, http.fetch | 64 MB |

---

## Layer 4: Snapshot Artifact (`molt.snapshot`)

### 4.1 Purpose

Capture post-init runtime state for sub-millisecond cold starts on edge platforms. After deterministic init (imports resolved, top-level code executed), the runtime state is serialized into a snapshot that can be restored instead of re-executing init.

### 4.2 Snapshot Format

```json
{
    "snapshot_version": 1,
    "abi_version": "0.1.0",
    "target_profile": "wasm_worker_cloudflare",
    "module_hash": "sha256:abc123...",
    "schema_registry_hash": "sha256:def456...",
    "mount_plan": {
        "/bundle": {"type": "bundle", "hash": "sha256:..."},
        "/tmp": {"type": "tmp", "quota_mb": 32},
        "/dev": {"type": "dev"}
    },
    "capability_manifest": ["fs.bundle.read", "fs.tmp.read", "fs.tmp.write", "http.fetch"],
    "determinism_stamp": "2026-03-20T00:00:00Z",
    "init_state_size": 524288
}
```

The `init_state_blob` is a binary appendix containing serialized WASM linear memory + globals after init completes.

### 4.3 Snapshot Lifecycle (Cloudflare)

1. `molt build --profile cloudflare --snapshot` runs init in a sandbox
2. After all imports resolve and top-level code executes, freeze memory state
3. Serialize to `molt.snapshot` (JSON header + binary blob)
4. Deploy: `worker_linked.wasm` + `bundle.tar` + `molt.snapshot` + `worker.js`
5. At cold start: restore snapshot instead of re-running init (skip import resolution)

### 4.4 Snapshot Rules

- Generation MUST occur after deterministic init only (no randomness, no time, no network)
- Secrets MUST NOT be captured (capability `rand.bytes` and `crypto.*` are denied during snapshot)
- Validity tied to `module_hash` + `abi_version` — stale snapshots are rejected
- `mount_plan` records which mounts were active at snapshot time
- Hosts MAY reject snapshots exceeding policy limits

### 4.5 CLI Integration

```bash
# Generate snapshot during build
molt build app.py --target wasm --bundle src/ --profile cloudflare --snapshot

# Generates:
#   dist/worker_linked.wasm
#   dist/bundle.tar
#   dist/molt.snapshot
#   dist/worker.js
```

---

## Deployment Artifact Structure

```
dist/
+-- worker_linked.wasm      # Linked WASM module (or .cwasm precompiled)
+-- bundle.tar              # Packaged Python source + resources
+-- molt.snapshot           # Post-init state snapshot (optional)
+-- manifest.json           # Deployment manifest (capabilities, mounts, profile)
+-- worker.js               # Host entry point (generated per profile)
```

---

## Testing Strategy

### Unit Tests (Rust)
- VFS path resolution with mount table (longest prefix, normalization)
- Traversal protection (`..` escape, symlink rejection in tar)
- BundleFs from tar archive (materialization, sorted iteration)
- TmpFs quota enforcement (`QuotaExceeded` error)
- TmpFs concurrent writes (last-writer-wins)
- Capability denial behavior with diagnostic messages
- DevFs read/write/flush semantics
- MoltVfsFile cursor-based read, write, close flush
- Empty path and root path handling
- WasiPassthroughFs delegation

### Integration Tests (Python)
- `open("/bundle/file.txt")` reads bundled content
- `open("/tmp/scratch.txt", "w")` writes temp file
- `open("/tmp/scratch.txt", "r")` reads back written content
- `import mymodule` resolves from `/bundle/`
- `pathlib.Path("/bundle").iterdir()` lists bundle contents
- `os.rename("/tmp/a", "/tmp/b")` works within mount
- Capability denial raises `PermissionError` with diagnostic hint
- Quota exceeded raises appropriate error

### Snapshot Tests (spec Section 9.2)
- Snapshot determinism: same input produces same snapshot bytes
- Snapshot restore correctness: restored state matches post-init state
- Cold-start delta: measure time with and without snapshot
- Stale snapshot rejection: modified module_hash causes rejection

### Platform Integration Tests (spec Section 9.3)
- Cloudflare Worker: fetch, /bundle, /tmp, cancellation, error surfacing
- Browser: bundle loading via fetch, /tmp in-memory, console logging
- WASI: preopen mapping, capability passthrough

### E2E Tests
- Full `molt build --bundle src/ --target wasm --profile cloudflare` produces working artifact
- Artifact structure matches expected layout (wasm + bundle + manifest + worker.js)
- Bundle contains expected files per manifest
- Generated worker.js is syntactically valid

---

## Performance Targets

| Metric | Target |
|--------|--------|
| VFS path resolution | < 1us per call |
| Bundle file read (1 KB) | < 10us |
| TmpFs write (1 KB) | < 5us |
| Cold start with 1 MB bundle (no snapshot) | < 50ms on Cloudflare Workers |
| Cold start with snapshot | < 5ms on Cloudflare Workers |
| Bundle overhead in deployment | < 5% of bundle data size |
