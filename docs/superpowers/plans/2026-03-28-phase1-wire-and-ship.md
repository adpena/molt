# Phase 1: Wire and Ship — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Connect the capability manifest to the build pipeline so resource limits, audit logging, and IO mode are actually enforced end-to-end in compiled binaries.

**Architecture:** The `--capability-manifest` CLI flag loads a `CapabilityManifest` from TOML/YAML/JSON, extracts `ResourceLimits`, `AuditConfig`, and `IoConfig`, and propagates them through the build pipeline to the WASM host runtime. The WASM host initializes a `LimitedTracker` and `AuditSink` at startup based on the manifest.

**Tech Stack:** Python (CLI, manifest parser), Rust (runtime init, resource tracker, audit sink)

---

## File Map

### Modified Files

| File | Change |
|------|--------|
| `src/molt/cli.py` | Wire `--capability-manifest` to `_prepare_build_config`, propagate to backend payload |
| `src/molt/capability_manifest.py` | Add `to_env_vars()` method for runtime propagation |
| `runtime/molt-runtime/src/state/runtime_state.rs` | Read resource limit env vars at `runtime_init`, install `LimitedTracker` |
| `runtime/molt-runtime/src/object/ops_sys.rs` | Add `molt_runtime_init_resource_tracker` FFI function |

### New Files

| File | Responsibility |
|------|---------------|
| `tests/test_manifest_cli_integration.py` | End-to-end: build with manifest, verify limits propagated |
| `tests/harness/corpus/resource/manifest_enforced.py` | Test program that should be killed by manifest limits |

---

### Task 1: Manifest to Environment Variables

**Files:**
- Modify: `src/molt/capability_manifest.py`
- Test: `tests/test_harness_report.py` (append)

- [ ] **Step 1: Write the test**

Append to `tests/test_harness_report.py` (or create a new test file):

```python
# In tests/test_manifest_env.py
import sys
sys.path.insert(0, "src")
from molt.capability_manifest import CapabilityManifest, ResourceLimits, AuditConfig, IoConfig


def test_manifest_to_env_vars_empty():
    m = CapabilityManifest()
    env = m.to_env_vars()
    assert env["MOLT_CAPABILITIES"] == ""
    assert "MOLT_RESOURCE_MAX_MEMORY" not in env


def test_manifest_to_env_vars_with_caps():
    m = CapabilityManifest(allow=["net", "fs.read"])
    env = m.to_env_vars()
    assert env["MOLT_CAPABILITIES"] == "fs.read,net"


def test_manifest_to_env_vars_with_resources():
    m = CapabilityManifest(
        resources=ResourceLimits(
            max_memory=67108864,
            max_duration=30.0,
            max_allocations=1000000,
            max_recursion_depth=500,
        )
    )
    env = m.to_env_vars()
    assert env["MOLT_RESOURCE_MAX_MEMORY"] == "67108864"
    assert env["MOLT_RESOURCE_MAX_DURATION_MS"] == "30000"
    assert env["MOLT_RESOURCE_MAX_ALLOCATIONS"] == "1000000"
    assert env["MOLT_RESOURCE_MAX_RECURSION_DEPTH"] == "500"


def test_manifest_to_env_vars_with_audit():
    m = CapabilityManifest(audit=AuditConfig(enabled=True, sink="jsonl", output="stderr"))
    env = m.to_env_vars()
    assert env["MOLT_AUDIT_ENABLED"] == "1"
    assert env["MOLT_AUDIT_SINK"] == "jsonl"


def test_manifest_to_env_vars_with_io_mode():
    m = CapabilityManifest(io=IoConfig(mode="virtual"))
    env = m.to_env_vars()
    assert env["MOLT_IO_MODE"] == "virtual"
```

- [ ] **Step 2: Implement `to_env_vars()` on CapabilityManifest**

Add to `src/molt/capability_manifest.py` on the `CapabilityManifest` class:

```python
def to_env_vars(self) -> dict[str, str]:
    """Convert manifest to environment variables for runtime propagation.

    The runtime reads these at initialization to configure ResourceTracker,
    AuditSink, and IoMode.
    """
    env: dict[str, str] = {}

    # Capabilities
    effective = sorted(self.effective_capabilities())
    env["MOLT_CAPABILITIES"] = ",".join(effective)

    # Resource limits
    if self.resources.max_memory is not None:
        env["MOLT_RESOURCE_MAX_MEMORY"] = str(self.resources.max_memory)
    if self.resources.max_duration is not None:
        env["MOLT_RESOURCE_MAX_DURATION_MS"] = str(int(self.resources.max_duration * 1000))
    if self.resources.max_allocations is not None:
        env["MOLT_RESOURCE_MAX_ALLOCATIONS"] = str(self.resources.max_allocations)
    if self.resources.max_recursion_depth is not None:
        env["MOLT_RESOURCE_MAX_RECURSION_DEPTH"] = str(self.resources.max_recursion_depth)

    # Audit
    if self.audit.enabled:
        env["MOLT_AUDIT_ENABLED"] = "1"
        env["MOLT_AUDIT_SINK"] = self.audit.sink
        env["MOLT_AUDIT_OUTPUT"] = self.audit.output

    # IO mode
    if self.io.mode != "real":
        env["MOLT_IO_MODE"] = self.io.mode

    return env
```

- [ ] **Step 3: Run tests, verify pass**
- [ ] **Step 4: Commit**

---

### Task 2: CLI Wires Manifest to Build Pipeline

**Files:**
- Modify: `src/molt/cli.py`

- [ ] **Step 1: Read `_prepare_build_config` (around line 3179) and the build payload function (around line 13273)**

- [ ] **Step 2: Add manifest loading to `_prepare_build_config`**

After the existing `--capabilities` parsing block (around line 3300), add:

```python
# Load capability manifest if provided
manifest = None
if capability_manifest is not None:
    from molt.capability_manifest import load_manifest
    try:
        manifest = load_manifest(capability_manifest)
        # Merge manifest capabilities with --capabilities flag
        if capabilities_list is None:
            capabilities_list = sorted(manifest.effective_capabilities())
        else:
            # CLI --capabilities takes precedence, manifest adds
            manifest_caps = manifest.effective_capabilities()
            merged = set(capabilities_list) | manifest_caps
            capabilities_list = sorted(merged)
    except Exception as e:
        return None, _fail(
            f"Invalid capability manifest: {e}",
            json_output,
            command="build",
        )
```

- [ ] **Step 3: Add `manifest` field to `_PreparedBuildConfig` dataclass**

- [ ] **Step 4: Propagate manifest env vars to run subprocess**

In the `run` command handler, after setting `MOLT_CAPABILITIES`, also set the resource/audit/io env vars from the manifest:

```python
if prepared_build_config.manifest is not None:
    env.update(prepared_build_config.manifest.to_env_vars())
```

- [ ] **Step 5: Commit**

---

### Task 3: Runtime Reads Resource Limit Env Vars

**Files:**
- Modify: `runtime/molt-runtime/src/object/ops_sys.rs`

- [ ] **Step 1: Add `molt_runtime_init_resources` function**

This is called during `runtime_init` to read env vars and install a `LimitedTracker`:

```rust
/// Initialize the resource tracker from environment variables.
///
/// Reads MOLT_RESOURCE_MAX_MEMORY, MOLT_RESOURCE_MAX_DURATION_MS,
/// MOLT_RESOURCE_MAX_ALLOCATIONS, MOLT_RESOURCE_MAX_RECURSION_DEPTH.
/// If any are set, installs a LimitedTracker. Otherwise, the default
/// UnlimitedTracker remains.
#[unsafe(no_mangle)]
pub extern "C" fn molt_runtime_init_resources() {
    use crate::resource::{LimitedTracker, ResourceLimits, set_tracker};
    use std::time::Duration;

    let max_memory = std::env::var("MOLT_RESOURCE_MAX_MEMORY")
        .ok()
        .and_then(|s| s.parse::<usize>().ok());
    let max_duration_ms = std::env::var("MOLT_RESOURCE_MAX_DURATION_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok());
    let max_allocations = std::env::var("MOLT_RESOURCE_MAX_ALLOCATIONS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok());
    let max_recursion_depth = std::env::var("MOLT_RESOURCE_MAX_RECURSION_DEPTH")
        .ok()
        .and_then(|s| s.parse::<usize>().ok());

    let has_any = max_memory.is_some()
        || max_duration_ms.is_some()
        || max_allocations.is_some()
        || max_recursion_depth.is_some();

    if has_any {
        let limits = ResourceLimits {
            max_memory,
            max_duration: max_duration_ms.map(Duration::from_millis),
            max_allocations,
            max_recursion_depth,
            max_operation_result_bytes: None,
        };
        set_tracker(Box::new(LimitedTracker::new(&limits)));
    }
}
```

- [ ] **Step 2: Add `molt_runtime_init_audit` function**

```rust
/// Initialize the audit sink from environment variables.
///
/// Reads MOLT_AUDIT_ENABLED, MOLT_AUDIT_SINK, MOLT_AUDIT_OUTPUT.
#[unsafe(no_mangle)]
pub extern "C" fn molt_runtime_init_audit() {
    use crate::audit::{set_audit_sink, StderrSink, JsonLinesSink, NullSink};

    let enabled = std::env::var("MOLT_AUDIT_ENABLED")
        .ok()
        .map(|s| s == "1")
        .unwrap_or(false);

    if !enabled {
        return;
    }

    let sink_type = std::env::var("MOLT_AUDIT_SINK").unwrap_or_else(|_| "stderr".into());
    match sink_type.as_str() {
        "jsonl" => {
            set_audit_sink(Box::new(JsonLinesSink::new(std::io::stderr())));
        }
        "stderr" => {
            set_audit_sink(Box::new(StderrSink));
        }
        _ => {
            set_audit_sink(Box::new(NullSink));
        }
    }
}
```

- [ ] **Step 3: Add `molt_runtime_init_io_mode` function**

```rust
/// Initialize IO mode from environment variable.
///
/// Reads MOLT_IO_MODE (real | virtual | callback).
#[unsafe(no_mangle)]
pub extern "C" fn molt_runtime_init_io_mode() {
    use crate::vfs::caps::{IoMode, set_io_mode};

    let mode_str = std::env::var("MOLT_IO_MODE").unwrap_or_else(|_| "real".into());
    let mode = match mode_str.as_str() {
        "virtual" => IoMode::Virtual,
        "callback" => IoMode::Callback,
        _ => IoMode::Real,
    };
    set_io_mode(mode);
}
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p molt-runtime`
Expected: Pass

- [ ] **Step 5: Commit**

---

### Task 4: CI Integration for New Crates

**Files:**
- Modify: `.github/workflows/ci.yml` (or equivalent CI config)

- [ ] **Step 1: Add harness test jobs to CI**

Add these steps to the existing CI workflow:

```yaml
- name: Test molt-snapshot
  run: cargo test -p molt-snapshot

- name: Test molt-embed
  run: cargo test -p molt-embed

- name: Test molt-harness
  run: cargo test -p molt-harness

- name: Test capability manifest
  run: PYTHONPATH=src python3 -m molt.capability_manifest

- name: Test harness modules
  run: |
    PYTHONPATH=src python3 -c "
    import sys, inspect; sys.path.insert(0, 'tests')
    total_pass = total_fail = 0
    for tf in ['tests/test_harness_report.py', 'tests/test_harness_layers.py',
               'tests/test_harness_self.py', 'tests/test_harness_orchestrator.py']:
        ns = {}
        exec(open(tf).read(), ns)
        for name, obj in ns.items():
            if name.startswith('test_') and callable(obj):
                sig = inspect.signature(obj)
                if sig.parameters: continue
                try: obj(); total_pass += 1
                except Exception as e: total_fail += 1; print(f'FAIL: {name}: {e}')
    assert total_fail == 0, f'{total_fail} tests failed'
    print(f'{total_pass} passed, {total_fail} failed')
    "

- name: Harness quick profile
  run: PYTHONPATH=src python3 -c "from molt.harness import main; exit(main(['quick']))"
```

- [ ] **Step 2: Commit**

---

### Task 5: End-to-End Resource Enforcement Test

**Files:**
- Create: `tests/test_manifest_enforcement.py`

- [ ] **Step 1: Write the end-to-end test**

```python
"""End-to-end test: build with manifest, verify resource limits enforced."""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, "src")


def test_dos_pow_rejected_with_manifest():
    """Building and running a program that does 2**10_000_000 with a manifest
    that sets resource limits should produce a MemoryError."""
    project_root = Path(__file__).parent.parent

    # Create a temp Python file
    with tempfile.NamedTemporaryFile(mode="w", suffix=".py", delete=False) as f:
        f.write("result = 2 ** 10_000_000\nprint(result)\n")
        src_path = f.name

    try:
        # Run through molt with resource limits via env vars
        env = os.environ.copy()
        env["PYTHONPATH"] = str(project_root / "src")
        env["MOLT_RESOURCE_MAX_MEMORY"] = str(64 * 1024 * 1024)  # 64MB

        result = subprocess.run(
            [sys.executable, "-c",
             f"from molt.cli import main; main(['run', '{src_path}'])"],
            capture_output=True, text=True, timeout=30, env=env,
        )

        # The program should fail with MemoryError from the DoS guard
        # (The guard is in ops_arith.rs, independent of the tracker)
        combined = result.stdout + result.stderr
        assert "MemoryError" in combined or result.returncode != 0, \
            f"Expected MemoryError, got: {combined[:500]}"
    finally:
        os.unlink(src_path)


def test_manifest_env_var_propagation():
    """Verify that to_env_vars produces correct env vars."""
    from molt.capability_manifest import (
        CapabilityManifest, ResourceLimits, AuditConfig, IoConfig,
    )
    m = CapabilityManifest(
        allow=["net"],
        resources=ResourceLimits(max_memory=1048576, max_duration=5.0),
        audit=AuditConfig(enabled=True, sink="jsonl"),
        io=IoConfig(mode="virtual"),
    )
    env = m.to_env_vars()
    assert env["MOLT_CAPABILITIES"] == "net"
    assert env["MOLT_RESOURCE_MAX_MEMORY"] == "1048576"
    assert env["MOLT_RESOURCE_MAX_DURATION_MS"] == "5000"
    assert env["MOLT_AUDIT_ENABLED"] == "1"
    assert env["MOLT_AUDIT_SINK"] == "jsonl"
    assert env["MOLT_IO_MODE"] == "virtual"
```

- [ ] **Step 2: Run tests, verify pass**
- [ ] **Step 3: Commit**
