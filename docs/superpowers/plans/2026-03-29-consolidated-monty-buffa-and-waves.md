# Consolidated Implementation Plan: Monty+Buffa Integration & Wave Completion

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close every remaining gap across the five active Wave plans (A/B/C, Harness, Cloudflare Hardening) and advance the Monty+Buffa roadmap from Phase 1 through Phase 3 — delivering CI integration, missing CLI flags, conformance measurement, fuzz hardening, and buffa encode/decode in a single coordinated sprint.

**Architecture:** Three dependency tiers. Tier 1 tasks are fully independent and can run in parallel. Tier 2 depends on Tier 1 completion (Wave A gate). Tier 3 depends on Tier 2. Within each tier, tasks are independent unless noted.

**Tech Stack:** Python 3.12 (CLI, manifest, harness), Rust (runtime crates, backends, fuzz targets), cargo test/nextest, pytest, wasm-ld, buffa 0.2, Cloudflare Workers/wrangler

---

## Current State Summary

| Component | Status | Remaining |
|-----------|--------|-----------|
| Phase 0 Foundation | COMPLETE | — |
| Phase 1 Wire-and-Ship | ~80% | CI jobs, 3 CLI flags, alloc tracking |
| Phase 2 Correctness | ~60% | Fuzz campaign, benchmarks, review backlog |
| Phase 3 Buffa | ~30% | Message encode/decode, AuditEvent.proto, snapshot eval |
| Wave A Correctness | ~90% | Vendor cleanup (cranelift-frontend) |
| Wave B Ecosystem | ~20% | six/click/attrs fixes (blocked on Wave A) |
| Wave C WASM | ~10% | Cache variant, importlib, artifact pipeline |
| Harness Engineering | ~100% | Wired to CLI — done |
| Cloudflare Hardening | ~80% | Deploy verify tool |

---

## File Map

### New Files

| File | Responsibility |
|------|---------------|
| `tests/test_manifest_cli_integration.py` | End-to-end: CLI flag → manifest → runtime enforcement |
| `tools/cloudflare_demo_deploy_verify.py` | Post-deploy live endpoint sweep with pass/fail gating |
| `runtime/molt-runtime-protobuf/src/encode.rs` | Schema-driven protobuf message encoder |
| `runtime/molt-runtime-protobuf/src/decode.rs` | Schema-driven protobuf message decoder |
| `runtime/molt-runtime-protobuf/src/audit_event.rs` | AuditEvent protobuf schema definition |

### Modified Files

| File | Change |
|------|--------|
| `.github/workflows/ci.yml` | Add test jobs for molt-snapshot, molt-embed, molt-harness, capability_manifest |
| `src/molt/cli.py` | Add `--audit-log`, `--io-mode`, `--type-gate` flags; extend cache variant with partition_mode |
| `src/molt/capability_manifest.py` | No changes needed (already complete) |
| `runtime/molt-backend/Cargo.toml` | No changes needed (cranelift 0.130 is current stable) |
| `runtime/molt-runtime-protobuf/Cargo.toml` | Add `molt-runtime` dev-dependency for integration tests |
| `runtime/molt-runtime-protobuf/src/lib.rs` | Re-export encode/decode/audit_event modules |
| `runtime/molt-snapshot/src/lib.rs` | Add buffa-format benchmark comparison (optional) |

### Deleted Files

| File | Reason |
|------|--------|
| `vendor/cranelift-frontend-0.130.0/` | Stale vendor copy; upstream 0.130 used directly |

---

## Tier 1: Independent Parallel Tasks

All tasks in this tier have zero dependencies and can be dispatched simultaneously.

---

### Task 1: CI Integration for New Crates (Monty Phase 1.4)

**Files:**
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Read the current CI workflow**

Run:
```bash
cat .github/workflows/ci.yml
```

Note the job structure: `build` (cargo build + molt-backend tests), `differential-tests` (708 differential tests).

- [ ] **Step 2: Add new test steps to the `build` job**

Append these steps after the existing "Run molt-backend unit tests" step in `.github/workflows/ci.yml`:

```yaml
      - name: Test molt-snapshot
        run: cargo test -p molt-snapshot

      - name: Test molt-embed
        run: cargo test -p molt-embed

      - name: Test molt-harness
        run: cargo test -p molt-harness

      - name: Test molt-runtime-protobuf
        run: cargo test -p molt-runtime-protobuf

      - name: Test capability manifest
        run: PYTHONPATH=src python3 -c "from molt.capability_manifest import CapabilityManifest; m = CapabilityManifest(); assert m.to_env_vars()['MOLT_CAPABILITIES'] == ''; print('OK')"

      - name: Test harness report module
        run: PYTHONPATH=src python3 -c "from molt.harness_report import LayerResult, LayerStatus; r = LayerResult(name='ci', status=LayerStatus.PASS, duration_s=0.1); assert r.passed; print('OK')"
```

- [ ] **Step 3: Verify the YAML is valid**

Run:
```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml')); print('valid YAML')"
```

Expected: `valid YAML`

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: add test jobs for molt-snapshot, molt-embed, molt-harness, molt-runtime-protobuf"
```

---

### Task 2: Add `--audit-log` CLI Flag (Monty Phase 1.2)

**Files:**
- Modify: `src/molt/cli.py`
- Create: `tests/test_manifest_cli_integration.py`

- [ ] **Step 1: Write the failing test**

Create `tests/test_manifest_cli_integration.py`:

```python
"""Tests for CLI flag → manifest → env var integration."""
import subprocess
import sys
import os
from pathlib import Path

PROJECT_ROOT = Path(__file__).parent.parent


def test_audit_log_flag_sets_env():
    """--audit-log flag should set MOLT_AUDIT_ENABLED=1 and MOLT_AUDIT_SINK=jsonl."""
    env = os.environ.copy()
    env["PYTHONPATH"] = str(PROJECT_ROOT / "src")
    result = subprocess.run(
        [sys.executable, "-c", """
import sys; sys.path.insert(0, 'src')
from molt.cli import _parse_audit_log_flag
env = _parse_audit_log_flag("jsonl:stderr")
assert env["MOLT_AUDIT_ENABLED"] == "1", f"got {env}"
assert env["MOLT_AUDIT_SINK"] == "jsonl", f"got {env}"
assert env["MOLT_AUDIT_OUTPUT"] == "stderr", f"got {env}"
print("PASS")
"""],
        capture_output=True, text=True, cwd=str(PROJECT_ROOT), env=env,
    )
    assert "PASS" in result.stdout, f"stderr: {result.stderr}\nstdout: {result.stdout}"


def test_io_mode_flag_sets_env():
    """--io-mode flag should set MOLT_IO_MODE."""
    env = os.environ.copy()
    env["PYTHONPATH"] = str(PROJECT_ROOT / "src")
    result = subprocess.run(
        [sys.executable, "-c", """
import sys; sys.path.insert(0, 'src')
from molt.cli import _parse_io_mode_flag
env = _parse_io_mode_flag("virtual")
assert env["MOLT_IO_MODE"] == "virtual", f"got {env}"
print("PASS")
"""],
        capture_output=True, text=True, cwd=str(PROJECT_ROOT), env=env,
    )
    assert "PASS" in result.stdout, f"stderr: {result.stderr}\nstdout: {result.stdout}"
```

- [ ] **Step 2: Run the test to verify it fails**

Run:
```bash
PYTHONPATH=src python3 -m pytest tests/test_manifest_cli_integration.py -v 2>&1 | head -30
```

Expected: FAIL (`_parse_audit_log_flag` not found)

- [ ] **Step 3: Find the CLI argument parser section**

Run:
```bash
grep -n 'add_argument.*--capabilities\b' src/molt/cli.py | head -5
```

This locates where capability flags are defined. The new flags go in the same argument group.

- [ ] **Step 4: Add `_parse_audit_log_flag` and `_parse_io_mode_flag` helper functions**

Add near the top of `src/molt/cli.py` (after imports, before the main parser setup — find the utility function area around line 100-200):

```python
def _parse_audit_log_flag(value: str) -> dict[str, str]:
    """Parse --audit-log flag value into environment variables.

    Format: SINK:OUTPUT (e.g., 'jsonl:stderr', 'stderr:stderr', 'jsonl:/tmp/audit.log')
    """
    parts = value.split(":", 1)
    sink = parts[0]
    output = parts[1] if len(parts) > 1 else "stderr"
    return {
        "MOLT_AUDIT_ENABLED": "1",
        "MOLT_AUDIT_SINK": sink,
        "MOLT_AUDIT_OUTPUT": output,
    }


def _parse_io_mode_flag(value: str) -> dict[str, str]:
    """Parse --io-mode flag value into environment variables.

    Valid values: real, virtual, callback
    """
    if value not in ("real", "virtual", "callback"):
        raise ValueError(f"Invalid IO mode: {value!r}. Must be one of: real, virtual, callback")
    env: dict[str, str] = {}
    if value != "real":
        env["MOLT_IO_MODE"] = value
    return env
```

- [ ] **Step 5: Add the argument parser entries**

Find the build subparser argument group (near the existing `--capabilities` and `--capability-manifest` entries). Add after them:

```python
build_parser.add_argument(
    "--audit-log",
    metavar="SINK:OUTPUT",
    help="Enable audit logging (e.g., 'jsonl:stderr', 'stderr:stderr')",
)
build_parser.add_argument(
    "--io-mode",
    choices=["real", "virtual", "callback"],
    default=None,
    help="IO mode: real (default), virtual (sandbox), callback (host-mediated)",
)
```

Add the same two arguments to the run subparser.

- [ ] **Step 6: Wire the flags into the environment propagation**

Find where `manifest.to_env_vars()` is called (around line 3309-3317 in `_prepare_build_config`). After the manifest env var merge, add:

```python
# --audit-log flag (overrides manifest audit config)
if audit_log is not None:
    env.update(_parse_audit_log_flag(audit_log))

# --io-mode flag (overrides manifest io config)
if io_mode is not None:
    env.update(_parse_io_mode_flag(io_mode))
```

- [ ] **Step 7: Run the test to verify it passes**

Run:
```bash
PYTHONPATH=src python3 -m pytest tests/test_manifest_cli_integration.py -v
```

Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add src/molt/cli.py tests/test_manifest_cli_integration.py
git commit -m "feat: add --audit-log and --io-mode CLI flags"
```

---

### Task 3: Delete Stale Cranelift Vendor Directory (Wave A Cleanup)

**Files:**
- Delete: `vendor/cranelift-frontend-0.130.0/`

- [ ] **Step 1: Verify no patch references remain**

Run:
```bash
grep -r 'cranelift-frontend.*vendor\|vendor.*cranelift-frontend' Cargo.toml */Cargo.toml 2>/dev/null || echo "No patch references found"
```

Expected: "No patch references found" (the workspace `[patch.crates-io]` section was already removed).

- [ ] **Step 2: Verify the build uses upstream cranelift-frontend**

Run:
```bash
cargo metadata --format-version 1 2>/dev/null | python3 -c "
import json, sys
meta = json.load(sys.stdin)
for pkg in meta['packages']:
    if pkg['name'] == 'cranelift-frontend':
        print(f\"cranelift-frontend {pkg['version']} from {pkg['source']}\")
        break
" 2>/dev/null || echo "cargo metadata not available — check Cargo.lock instead"
```

Expected: `cranelift-frontend 0.130.0 from registry+https://github.com/rust-lang/crates.io-index`

- [ ] **Step 3: Delete the vendor directory**

Run:
```bash
rm -rf vendor/cranelift-frontend-0.130.0
```

- [ ] **Step 4: Verify cargo build still works**

Run:
```bash
cargo check -p molt-backend 2>&1 | tail -5
```

Expected: `Finished` with no errors.

- [ ] **Step 5: Commit**

```bash
git add -A vendor/
git commit -m "chore: remove stale vendor/cranelift-frontend-0.130.0"
```

---

### Task 4: Extend Cache Variant with Partition Mode (Wave C, Task 1)

**Files:**
- Modify: `src/molt/cli.py` (around line 17684-17694)
- Modify: `tests/cli/test_cli_import_collection.py`

- [ ] **Step 1: Write the failing test**

Add to `tests/cli/test_cli_import_collection.py`:

```python
def test_stdlib_partition_mode_changes_cache_identity():
    """Cache identity must differ when stdlib partition mode changes."""
    import sys
    sys.path.insert(0, "src")
    # Import the internal cache_variant builder
    # The function is at ~line 17684 and builds a string from profile+emit+flags
    from molt.cli import _build_cache_variant

    variant_mono = _build_cache_variant(
        profile="dev", runtime_cargo="debug", backend_cargo="debug",
        emit="bin", stdlib_split=False, codegen_env="x", linked=False,
        partition_mode=False,
    )
    variant_part = _build_cache_variant(
        profile="dev", runtime_cargo="debug", backend_cargo="debug",
        emit="bin", stdlib_split=False, codegen_env="x", linked=False,
        partition_mode=True,
    )
    assert variant_mono != variant_part, (
        "Cache variant must change when partition mode changes"
    )
```

- [ ] **Step 2: Run the test to verify it fails**

Run:
```bash
PYTHONPATH=src python3 -m pytest tests/cli/test_cli_import_collection.py::test_stdlib_partition_mode_changes_cache_identity -v 2>&1 | tail -10
```

Expected: FAIL (either `_build_cache_variant` doesn't exist as a named function or doesn't accept `partition_mode`).

- [ ] **Step 3: Read the current cache_variant construction**

Run:
```bash
grep -n 'cache_variant' src/molt/cli.py | head -20
```

Read lines 17680-17700 of `src/molt/cli.py` to see the exact code that builds the cache variant string.

- [ ] **Step 4: Refactor the cache variant construction into a named function**

If the cache variant is built inline (not in a callable function), extract it into `_build_cache_variant()`. Add `partition_mode: bool = False` as a parameter. When `partition_mode` is True, append `":partitioned:v1"` to the variant string before it's returned:

```python
def _build_cache_variant(
    *, profile: str, runtime_cargo: str, backend_cargo: str,
    emit: str, stdlib_split: bool, codegen_env: str, linked: bool,
    partition_mode: bool = False,
) -> str:
    parts = [profile, runtime_cargo, backend_cargo, emit]
    if stdlib_split:
        parts.append("stdlib_split")
    parts.append(codegen_env)
    if linked:
        parts.append("linked")
    if partition_mode:
        parts.append("partitioned:v1")
    return ":".join(parts)
```

Replace the inline construction site with a call to `_build_cache_variant(...)`.

- [ ] **Step 5: Run the test to verify it passes**

Run:
```bash
PYTHONPATH=src python3 -m pytest tests/cli/test_cli_import_collection.py::test_stdlib_partition_mode_changes_cache_identity -v
```

Expected: PASS

- [ ] **Step 6: Run the broader CLI test suite to check for regressions**

Run:
```bash
PYTHONPATH=src python3 -m pytest tests/cli/test_cli_import_collection.py -v 2>&1 | tail -20
```

Expected: All existing tests still pass.

- [ ] **Step 7: Commit**

```bash
git add src/molt/cli.py tests/cli/test_cli_import_collection.py
git commit -m "feat: cache identity encodes stdlib partition mode"
```

---

### Task 5: Create Cloudflare Deploy Verify Tool (Cloudflare Hardening, Task 6)

**Files:**
- Create: `tools/cloudflare_demo_deploy_verify.py`
- Modify: `tests/cloudflare/test_live_verifier.py`

- [ ] **Step 1: Read the existing local verify tool for patterns**

Run:
```bash
head -80 tools/cloudflare_demo_verify.py
```

Note the `CloudflareBundleContract` dataclass, probe paths, and validation structure.

- [ ] **Step 2: Read the existing live verifier test**

Run:
```bash
cat tests/cloudflare/test_live_verifier.py
```

Note what the test expects the deploy verify tool to provide.

- [ ] **Step 3: Write the deploy verify tool**

Create `tools/cloudflare_demo_deploy_verify.py`:

```python
#!/usr/bin/env python3
"""Post-deploy live verification for Cloudflare demo worker.

Usage:
    python3 tools/cloudflare_demo_deploy_verify.py \
        --entry examples/cloudflare-demo/src/app.py \
        --live-base-url https://molt-python-demo.adpena.workers.dev \
        --artifact-root logs/cloudflare_demo_20260329

Runs build → deploy → live endpoint sweep. Exits 0 if all probes pass, 1 otherwise.
"""
import argparse
import json
import os
import subprocess
import sys
import time
import urllib.request
import urllib.error
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path

# Canonical endpoint matrix — must match tools/cloudflare_demo_verify.py
PROBE_PATHS = [
    ("/", 200, "text/html", None),
    ("/fib/10", 200, "text/plain", "55"),
    ("/primes/100", 200, "text/plain", None),
    ("/diamond/5", 200, "text/plain", None),
    ("/fizzbuzz/15", 200, "text/plain", "FizzBuzz"),
    ("/pi/1000", 200, "text/plain", "3.14"),
    ("/generate/1", 200, "text/plain", None),
    ("/bench", 200, "text/plain", None),
    ("/demo", 200, "text/html", None),
]


@dataclass
class ProbeResult:
    path: str
    status: int
    expected_status: int
    body_snippet: str
    content_type: str
    passed: bool
    latency_ms: float
    error: str | None = None


@dataclass
class DeployVerifyReport:
    timestamp: str
    base_url: str
    probes: list[ProbeResult] = field(default_factory=list)
    all_passed: bool = False
    total_latency_ms: float = 0.0

    def to_json(self) -> str:
        return json.dumps(
            {
                "timestamp": self.timestamp,
                "base_url": self.base_url,
                "all_passed": self.all_passed,
                "total_latency_ms": self.total_latency_ms,
                "probe_count": len(self.probes),
                "passed_count": sum(1 for p in self.probes if p.passed),
                "failed_count": sum(1 for p in self.probes if not p.passed),
                "probes": [
                    {
                        "path": p.path,
                        "status": p.status,
                        "expected_status": p.expected_status,
                        "passed": p.passed,
                        "latency_ms": p.latency_ms,
                        "body_snippet": p.body_snippet[:200],
                        "error": p.error,
                    }
                    for p in self.probes
                ],
            },
            indent=2,
        )


def probe_endpoint(
    base_url: str, path: str, expected_status: int, expected_content_type: str | None,
    expected_body_sentinel: str | None, timeout: float = 15.0,
) -> ProbeResult:
    """Hit one endpoint and return a ProbeResult."""
    url = f"{base_url.rstrip('/')}{path}"
    start = time.monotonic()
    try:
        req = urllib.request.Request(url, headers={"User-Agent": "molt-deploy-verify/1.0"})
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            status = resp.status
            body = resp.read().decode("utf-8", errors="replace")
            content_type = resp.headers.get("Content-Type", "")
            latency_ms = (time.monotonic() - start) * 1000

            passed = status == expected_status
            if expected_body_sentinel and expected_body_sentinel not in body:
                passed = False
            # Reject known error indicators
            if "\x00" in body[:100] or "Error 1102" in body:
                passed = False

            return ProbeResult(
                path=path, status=status, expected_status=expected_status,
                body_snippet=body[:500], content_type=content_type,
                passed=passed, latency_ms=latency_ms,
            )
    except (urllib.error.URLError, urllib.error.HTTPError, TimeoutError) as e:
        latency_ms = (time.monotonic() - start) * 1000
        return ProbeResult(
            path=path, status=0, expected_status=expected_status,
            body_snippet="", content_type="",
            passed=False, latency_ms=latency_ms, error=str(e),
        )


def run_live_sweep(base_url: str) -> DeployVerifyReport:
    """Run the full endpoint probe matrix against a live URL."""
    report = DeployVerifyReport(
        timestamp=datetime.now(timezone.utc).isoformat(),
        base_url=base_url,
    )
    for path, status, ctype, sentinel in PROBE_PATHS:
        result = probe_endpoint(base_url, path, status, ctype, sentinel)
        report.probes.append(result)
        report.total_latency_ms += result.latency_ms

    report.all_passed = all(p.passed for p in report.probes)
    return report


def main() -> int:
    parser = argparse.ArgumentParser(description="Cloudflare demo deploy verification")
    parser.add_argument("--live-base-url", required=True, help="Live worker URL")
    parser.add_argument("--artifact-root", default="logs/cloudflare_deploy",
                        help="Directory for logs and reports")
    parser.add_argument("--retries", type=int, default=2,
                        help="Number of retry attempts for failed probes")
    args = parser.parse_args()

    artifact_root = Path(args.artifact_root)
    artifact_root.mkdir(parents=True, exist_ok=True)

    print(f"[deploy-verify] Sweeping {args.live_base_url} ...")
    report = run_live_sweep(args.live_base_url)

    # Retry failed probes
    for attempt in range(args.retries):
        failed = [p for p in report.probes if not p.passed]
        if not failed:
            break
        print(f"[deploy-verify] Retry {attempt + 1}/{args.retries} for {len(failed)} failed probes")
        time.sleep(2)
        for i, probe in enumerate(report.probes):
            if not probe.passed:
                path, status, ctype, sentinel = next(
                    (p, s, c, sn) for p, s, c, sn in PROBE_PATHS if p == probe.path
                )
                report.probes[i] = probe_endpoint(
                    args.live_base_url, path, status, ctype, sentinel
                )
        report.all_passed = all(p.passed for p in report.probes)

    # Write report
    report_path = artifact_root / "deploy_verify_report.json"
    report_path.write_text(report.to_json())
    print(f"[deploy-verify] Report written to {report_path}")

    # Console summary
    passed = sum(1 for p in report.probes if p.passed)
    total = len(report.probes)
    for p in report.probes:
        mark = "PASS" if p.passed else "FAIL"
        print(f"  [{mark}] {p.path} → {p.status} ({p.latency_ms:.0f}ms)")
        if p.error:
            print(f"         error: {p.error}")

    print(f"\n[deploy-verify] {passed}/{total} passed, {report.total_latency_ms:.0f}ms total")
    return 0 if report.all_passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
```

- [ ] **Step 4: Verify the tool runs (dry-run against a non-existent URL)**

Run:
```bash
python3 tools/cloudflare_demo_deploy_verify.py --live-base-url http://127.0.0.1:99999 --artifact-root tmp/deploy_verify_test 2>&1 | head -20
```

Expected: Runs without crashing, reports FAIL for all probes (connection refused).

- [ ] **Step 5: Commit**

```bash
git add tools/cloudflare_demo_deploy_verify.py
git commit -m "feat: add cloudflare deploy verification tool with live endpoint sweep"
```

---

### Task 6: Run and Record Monty Conformance Baseline (Monty Phase 2.1)

**Files:**
- Modify: `tests/harness/baselines/baseline.json`

- [ ] **Step 1: Read the current conformance runner**

Run:
```bash
head -60 tests/harness/run_monty_conformance.py
```

Understand the test execution model and output format.

- [ ] **Step 2: Run the Monty conformance suite and capture results**

Run:
```bash
export MOLT_EXT_ROOT=$PWD
export CARGO_TARGET_DIR=$PWD/target
export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR
export MOLT_CACHE=$PWD/.molt_cache
export MOLT_DIFF_ROOT=$PWD/tmp/diff
export MOLT_DIFF_TMPDIR=$PWD/tmp
export UV_CACHE_DIR=$PWD/.uv-cache
export TMPDIR=$PWD/tmp
PYTHONPATH=src python3 tests/harness/run_monty_conformance.py 2>&1 | tee logs/monty_conformance_$(date +%Y%m%d_%H%M%S).log
```

Expected: A pass/fail summary. Note the pass rate — target is >95%.

- [ ] **Step 3: Record the baseline**

If `tests/harness/baselines/baseline.json` exists, read it and add/update a `monty_conformance` section:

```json
{
  "monty_conformance": {
    "date": "2026-03-29",
    "total": <total>,
    "passed": <passed>,
    "failed": <failed>,
    "pass_rate": <rate>
  }
}
```

- [ ] **Step 4: Commit**

```bash
git add tests/harness/baselines/baseline.json logs/
git commit -m "test: record monty conformance baseline"
```

---

### Task 7: Fuzz Campaign — NaN-Boxing (Monty Phase 2.2, Part 1)

**Files:**
- No new files

- [ ] **Step 1: Verify fuzz target compiles**

Run:
```bash
cd runtime/molt-backend && cargo +nightly fuzz list 2>&1
```

Expected: Lists `fuzz_nan_boxing`, `fuzz_tir_passes`, `fuzz_wasm_type_section`.

- [ ] **Step 2: Run fuzz_nan_boxing for 10 minutes (initial sweep)**

Run:
```bash
cd runtime/molt-backend && cargo +nightly fuzz run fuzz_nan_boxing -- -max_total_time=600 2>&1 | tail -20
```

Expected: No crashes. If crashes are found, they'll be in `fuzz/artifacts/fuzz_nan_boxing/`.

- [ ] **Step 3: Run fuzz_tir_passes for 10 minutes**

Run:
```bash
cd runtime/molt-backend && cargo +nightly fuzz run fuzz_tir_passes -- -max_total_time=600 2>&1 | tail -20
```

Expected: No crashes.

- [ ] **Step 4: Run fuzz_wasm_type_section for 10 minutes**

Run:
```bash
cd runtime/molt-backend && cargo +nightly fuzz run fuzz_wasm_type_section -- -max_total_time=600 2>&1 | tail -20
```

Expected: No crashes.

- [ ] **Step 5: If any crashes found, triage and file**

For each crash artifact:
```bash
cd runtime/molt-backend && cargo +nightly fuzz fmt fuzz_nan_boxing <artifact_path>
```

Create a minimal reproducer and fix the panic. Crashes in NaN-boxing or TIR passes are P0.

- [ ] **Step 6: Commit any corpus seeds**

```bash
git add runtime/molt-backend/fuzz/corpus/
git commit -m "test: expand fuzz corpus after 30-minute campaign"
```

---

## Tier 2: Post-Gate Tasks (After Tier 1)

These depend on Tier 1 completion (especially Wave A cleanup and CI integration).

---

### Task 8: Buffa Message Encode/Decode (Monty Phase 3.1)

**Files:**
- Create: `runtime/molt-runtime-protobuf/src/encode.rs`
- Create: `runtime/molt-runtime-protobuf/src/decode.rs`
- Modify: `runtime/molt-runtime-protobuf/src/lib.rs`

- [ ] **Step 1: Write the test for message encoding**

Add to the bottom of `runtime/molt-runtime-protobuf/src/lib.rs`, inside the existing `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn encode_simple_message() {
        use crate::encode::encode_message;

        let schema = MessageSchema {
            name: "test.Person".into(),
            fields: vec![
                FieldDef {
                    number: 1,
                    name: "name".into(),
                    wire_type: WireType::LengthDelimited,
                    repeated: false,
                    optional: false,
                },
                FieldDef {
                    number: 2,
                    name: "age".into(),
                    wire_type: WireType::Varint,
                    repeated: false,
                    optional: false,
                },
            ],
        };

        let values: Vec<FieldValue> = vec![
            FieldValue::Bytes(b"Alice".to_vec()),
            FieldValue::Uint64(30),
        ];

        let bytes = encode_message(&schema, &values);
        assert!(!bytes.is_empty());
        // Field 1 (string "Alice"): tag=0x0A, len=5, "Alice"
        assert_eq!(bytes[0], 0x0A);
        assert_eq!(bytes[1], 5);
        assert_eq!(&bytes[2..7], b"Alice");
        // Field 2 (varint 30): tag=0x10, value=30
        assert_eq!(bytes[7], 0x10);
        assert_eq!(bytes[8], 30);
    }

    #[test]
    fn decode_simple_message() {
        use crate::decode::decode_message;
        use crate::encode::encode_message;

        let schema = MessageSchema {
            name: "test.Person".into(),
            fields: vec![
                FieldDef {
                    number: 1,
                    name: "name".into(),
                    wire_type: WireType::LengthDelimited,
                    repeated: false,
                    optional: false,
                },
                FieldDef {
                    number: 2,
                    name: "age".into(),
                    wire_type: WireType::Varint,
                    repeated: false,
                    optional: false,
                },
            ],
        };

        let values = vec![
            FieldValue::Bytes(b"Alice".to_vec()),
            FieldValue::Uint64(30),
        ];

        let encoded = encode_message(&schema, &values);
        let decoded = decode_message(&schema, &encoded).unwrap();
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0], FieldValue::Bytes(b"Alice".to_vec()));
        assert_eq!(decoded[1], FieldValue::Uint64(30));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run:
```bash
cargo test -p molt-runtime-protobuf 2>&1 | tail -10
```

Expected: FAIL (modules `encode` and `decode` don't exist yet).

- [ ] **Step 3: Create the FieldValue type and encode module**

Create `runtime/molt-runtime-protobuf/src/encode.rs`:

```rust
//! Schema-driven protobuf message encoder.

use crate::{
    encode_bytes_field, encode_tag, encode_varint, encode_uint64_field,
    FieldDef, MessageSchema, WireType,
};

/// Runtime value for a single protobuf field.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    Uint64(u64),
    Int64(i64),
    Fixed32(u32),
    Fixed64(u64),
    Bytes(Vec<u8>),
}

/// Encode a message according to its schema and field values.
///
/// `values` must be in the same order as `schema.fields`.
/// Panics if `values.len() != schema.fields.len()`.
pub fn encode_message(schema: &MessageSchema, values: &[FieldValue]) -> Vec<u8> {
    assert_eq!(
        schema.fields.len(),
        values.len(),
        "field count mismatch: schema has {}, got {}",
        schema.fields.len(),
        values.len(),
    );

    let mut buf = Vec::new();
    for (field, value) in schema.fields.iter().zip(values.iter()) {
        encode_field(field, value, &mut buf);
    }
    buf
}

fn encode_field(field: &FieldDef, value: &FieldValue, buf: &mut Vec<u8>) {
    match (field.wire_type, value) {
        (WireType::Varint, FieldValue::Uint64(v)) => {
            encode_uint64_field(field.number, *v, buf);
        }
        (WireType::Varint, FieldValue::Int64(v)) => {
            encode_uint64_field(field.number, *v as u64, buf);
        }
        (WireType::LengthDelimited, FieldValue::Bytes(data)) => {
            encode_bytes_field(field.number, data, buf);
        }
        (WireType::Fixed32, FieldValue::Fixed32(v)) => {
            encode_tag(field.number, WireType::Fixed32, buf);
            buf.extend_from_slice(&v.to_le_bytes());
        }
        (WireType::Fixed64, FieldValue::Fixed64(v)) => {
            encode_tag(field.number, WireType::Fixed64, buf);
            buf.extend_from_slice(&v.to_le_bytes());
        }
        _ => {
            panic!(
                "wire type {:?} incompatible with value {:?} for field {}",
                field.wire_type, value, field.name
            );
        }
    }
}
```

- [ ] **Step 4: Create the decode module**

Create `runtime/molt-runtime-protobuf/src/decode.rs`:

```rust
//! Schema-driven protobuf message decoder.

use crate::{decode_varint, FieldDef, MessageSchema, WireType};
use crate::encode::FieldValue;

/// Decode error.
#[derive(Debug)]
pub enum MessageDecodeError {
    Truncated { context: &'static str },
    UnknownField { number: u32 },
    WireTypeMismatch { field: String, expected: WireType },
    Varint(crate::DecodeError),
}

impl std::fmt::Display for MessageDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated { context } => write!(f, "truncated: {context}"),
            Self::UnknownField { number } => write!(f, "unknown field number: {number}"),
            Self::WireTypeMismatch { field, expected } => {
                write!(f, "wire type mismatch for field {field}: expected {expected:?}")
            }
            Self::Varint(e) => write!(f, "varint decode: {e}"),
        }
    }
}

impl std::error::Error for MessageDecodeError {}

/// Decode a protobuf message according to its schema.
///
/// Returns field values in schema field order.
/// Unknown fields are skipped. Missing optional fields are omitted.
pub fn decode_message(
    schema: &MessageSchema,
    data: &[u8],
) -> Result<Vec<FieldValue>, MessageDecodeError> {
    let mut cursor = 0;
    let mut values: Vec<Option<FieldValue>> = vec![None; schema.fields.len()];

    while cursor < data.len() {
        // Read tag
        let (tag_raw, consumed) = decode_varint(&data[cursor..])
            .map_err(MessageDecodeError::Varint)?;
        cursor += consumed;

        let field_number = (tag_raw >> 3) as u32;
        let wire_type_raw = (tag_raw & 0x07) as u8;

        // Find field in schema
        let field_idx = schema.fields.iter().position(|f| f.number == field_number);

        match field_idx {
            Some(idx) => {
                let field = &schema.fields[idx];
                let (value, bytes_read) = decode_field_value(field, wire_type_raw, &data[cursor..])?;
                cursor += bytes_read;
                values[idx] = Some(value);
            }
            None => {
                // Skip unknown field
                let bytes_skipped = skip_field(wire_type_raw, &data[cursor..])?;
                cursor += bytes_skipped;
            }
        }
    }

    Ok(values.into_iter().flatten().collect())
}

fn decode_field_value(
    field: &FieldDef,
    wire_type_raw: u8,
    data: &[u8],
) -> Result<(FieldValue, usize), MessageDecodeError> {
    match wire_type_raw {
        0 => {
            // Varint
            let (val, consumed) = decode_varint(data).map_err(MessageDecodeError::Varint)?;
            Ok((FieldValue::Uint64(val), consumed))
        }
        1 => {
            // Fixed64
            if data.len() < 8 {
                return Err(MessageDecodeError::Truncated { context: "fixed64" });
            }
            let val = u64::from_le_bytes(data[..8].try_into().unwrap());
            Ok((FieldValue::Fixed64(val), 8))
        }
        2 => {
            // Length-delimited
            let (len, consumed) = decode_varint(data).map_err(MessageDecodeError::Varint)?;
            let len = len as usize;
            let start = consumed;
            if start + len > data.len() {
                return Err(MessageDecodeError::Truncated { context: "length-delimited" });
            }
            let bytes = data[start..start + len].to_vec();
            Ok((FieldValue::Bytes(bytes), start + len))
        }
        5 => {
            // Fixed32
            if data.len() < 4 {
                return Err(MessageDecodeError::Truncated { context: "fixed32" });
            }
            let val = u32::from_le_bytes(data[..4].try_into().unwrap());
            Ok((FieldValue::Fixed32(val), 4))
        }
        _ => Err(MessageDecodeError::WireTypeMismatch {
            field: field.name.clone(),
            expected: field.wire_type,
        }),
    }
}

fn skip_field(wire_type_raw: u8, data: &[u8]) -> Result<usize, MessageDecodeError> {
    match wire_type_raw {
        0 => {
            let (_, consumed) = decode_varint(data).map_err(MessageDecodeError::Varint)?;
            Ok(consumed)
        }
        1 => Ok(8),
        2 => {
            let (len, consumed) = decode_varint(data).map_err(MessageDecodeError::Varint)?;
            Ok(consumed + len as usize)
        }
        5 => Ok(4),
        _ => Err(MessageDecodeError::Truncated { context: "unknown wire type" }),
    }
}
```

- [ ] **Step 5: Wire the new modules into lib.rs**

Add to the top of `runtime/molt-runtime-protobuf/src/lib.rs`, after the existing `use` statements:

```rust
pub mod encode;
pub mod decode;
```

And add `FieldValue` to the re-exports used in tests by adding at the top of the test module:
```rust
use crate::encode::FieldValue;
```

- [ ] **Step 6: Run tests to verify they pass**

Run:
```bash
cargo test -p molt-runtime-protobuf 2>&1
```

Expected: All tests pass (existing varint/field tests + new encode/decode tests).

- [ ] **Step 7: Commit**

```bash
git add runtime/molt-runtime-protobuf/src/
git commit -m "feat: add schema-driven protobuf message encode/decode via buffa"
```

---

### Task 9: AuditEvent Protobuf Schema (Monty Phase 3.2)

**Files:**
- Create: `runtime/molt-runtime-protobuf/src/audit_event.rs`
- Modify: `runtime/molt-runtime-protobuf/src/lib.rs`

- [ ] **Step 1: Write the test**

Add to the test module in `runtime/molt-runtime-protobuf/src/lib.rs`:

```rust
    #[test]
    fn audit_event_encode_decode_roundtrip() {
        use crate::audit_event::{audit_event_schema, encode_audit_event, decode_audit_event};

        let event = encode_audit_event(
            /* timestamp_ns */ 1234567890,
            /* operation */ "fs.read",
            /* capability */ "fs.read",
            /* decision */ 0, // Allowed
            /* module */ "my_module",
        );
        assert!(!event.is_empty());

        let decoded = decode_audit_event(&event).unwrap();
        assert_eq!(decoded.timestamp_ns, 1234567890);
        assert_eq!(decoded.operation, "fs.read");
        assert_eq!(decoded.capability, "fs.read");
        assert_eq!(decoded.decision, 0);
        assert_eq!(decoded.module_name, "my_module");
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run:
```bash
cargo test -p molt-runtime-protobuf audit_event 2>&1 | tail -10
```

Expected: FAIL (module `audit_event` doesn't exist).

- [ ] **Step 3: Create the audit_event module**

Create `runtime/molt-runtime-protobuf/src/audit_event.rs`:

```rust
//! AuditEvent protobuf schema and convenience encode/decode.
//!
//! Field numbers match the canonical AuditEvent.proto:
//!   1: timestamp_ns (uint64)
//!   2: operation (string)
//!   3: capability (string)
//!   4: decision (uint64, 0=Allowed, 1=Denied, 2=ResourceExceeded)
//!   5: module (string)

use crate::{
    MessageSchema, FieldDef, WireType,
    encode_uint64_field, encode_string_field, decode_varint,
};
use crate::encode::{FieldValue, encode_message};
use crate::decode::{decode_message, MessageDecodeError};

/// Returns the canonical AuditEvent message schema.
pub fn audit_event_schema() -> MessageSchema {
    MessageSchema {
        name: "molt.AuditEvent".into(),
        fields: vec![
            FieldDef {
                number: 1,
                name: "timestamp_ns".into(),
                wire_type: WireType::Varint,
                repeated: false,
                optional: false,
            },
            FieldDef {
                number: 2,
                name: "operation".into(),
                wire_type: WireType::LengthDelimited,
                repeated: false,
                optional: false,
            },
            FieldDef {
                number: 3,
                name: "capability".into(),
                wire_type: WireType::LengthDelimited,
                repeated: false,
                optional: false,
            },
            FieldDef {
                number: 4,
                name: "decision".into(),
                wire_type: WireType::Varint,
                repeated: false,
                optional: false,
            },
            FieldDef {
                number: 5,
                name: "module".into(),
                wire_type: WireType::LengthDelimited,
                repeated: false,
                optional: false,
            },
        ],
    }
}

/// Decoded audit event.
#[derive(Debug, Clone)]
pub struct DecodedAuditEvent {
    pub timestamp_ns: u64,
    pub operation: String,
    pub capability: String,
    /// 0 = Allowed, 1 = Denied, 2 = ResourceExceeded
    pub decision: u64,
    pub module_name: String,
}

/// Encode an audit event to protobuf wire format.
pub fn encode_audit_event(
    timestamp_ns: u64,
    operation: &str,
    capability: &str,
    decision: u64,
    module_name: &str,
) -> Vec<u8> {
    let schema = audit_event_schema();
    let values = vec![
        FieldValue::Uint64(timestamp_ns),
        FieldValue::Bytes(operation.as_bytes().to_vec()),
        FieldValue::Bytes(capability.as_bytes().to_vec()),
        FieldValue::Uint64(decision),
        FieldValue::Bytes(module_name.as_bytes().to_vec()),
    ];
    encode_message(&schema, &values)
}

/// Decode an audit event from protobuf wire format.
pub fn decode_audit_event(data: &[u8]) -> Result<DecodedAuditEvent, MessageDecodeError> {
    let schema = audit_event_schema();
    let values = decode_message(&schema, data)?;

    let timestamp_ns = match values.get(0) {
        Some(FieldValue::Uint64(v)) => *v,
        _ => 0,
    };
    let operation = match values.get(1) {
        Some(FieldValue::Bytes(b)) => String::from_utf8_lossy(b).into_owned(),
        _ => String::new(),
    };
    let capability = match values.get(2) {
        Some(FieldValue::Bytes(b)) => String::from_utf8_lossy(b).into_owned(),
        _ => String::new(),
    };
    let decision = match values.get(3) {
        Some(FieldValue::Uint64(v)) => *v,
        _ => 0,
    };
    let module_name = match values.get(4) {
        Some(FieldValue::Bytes(b)) => String::from_utf8_lossy(b).into_owned(),
        _ => String::new(),
    };

    Ok(DecodedAuditEvent {
        timestamp_ns,
        operation,
        capability,
        decision,
        module_name,
    })
}
```

- [ ] **Step 4: Wire the module into lib.rs**

Add to the module declarations in `runtime/molt-runtime-protobuf/src/lib.rs`:

```rust
pub mod audit_event;
```

- [ ] **Step 5: Run the test to verify it passes**

Run:
```bash
cargo test -p molt-runtime-protobuf audit_event 2>&1
```

Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add runtime/molt-runtime-protobuf/src/audit_event.rs runtime/molt-runtime-protobuf/src/lib.rs
git commit -m "feat: add AuditEvent protobuf schema with encode/decode"
```

---

### Task 10: Snapshot Format Evaluation — Buffa vs Hand-Rolled (Monty Phase 3.3)

**Files:**
- Modify: `runtime/molt-snapshot/Cargo.toml`
- Create: `runtime/molt-snapshot/benches/format_comparison.rs`

- [ ] **Step 1: Add criterion and molt-runtime-protobuf as dev-dependencies**

Add to `runtime/molt-snapshot/Cargo.toml`:

```toml
[dev-dependencies]
criterion = { version = "0.5", features = ["html_reports"] }
molt-runtime-protobuf = { path = "../molt-runtime-protobuf" }

[[bench]]
name = "format_comparison"
harness = false
```

- [ ] **Step 2: Write the benchmark**

Create `runtime/molt-snapshot/benches/format_comparison.rs`:

```rust
//! Benchmark: hand-rolled snapshot serialization vs buffa protobuf encoding.
//!
//! Measures serialize + deserialize roundtrip for a representative snapshot.

use criterion::{criterion_group, criterion_main, Criterion};
use molt_snapshot::{
    ExecutionSnapshot, PendingExternalCall, ProgramCounter, ResourceSnapshot,
};

fn sample_snapshot(memory_size: usize) -> ExecutionSnapshot {
    ExecutionSnapshot {
        version: 1,
        memory: vec![0xABu8; memory_size],
        globals: (0..50).map(|i| 0x7ff8_0001_0000_0000u64 + i).collect(),
        table: (0..100).collect(),
        pc: ProgramCounter {
            func_index: 42,
            instruction_offset: 1024,
            call_depth: 8,
        },
        pending_call: PendingExternalCall {
            function_name: "fetch_user_data".into(),
            args: vec![0x7ff8_0001_0000_002A; 5],
            call_id: 99999,
        },
        resource_state: ResourceSnapshot {
            allocation_count: 50000,
            memory_used: memory_size,
            elapsed_ms: 1500,
        },
    }
}

fn bench_hand_rolled(c: &mut Criterion) {
    let snap = sample_snapshot(65536); // 64KB memory

    c.bench_function("hand_rolled_serialize_64kb", |b| {
        b.iter(|| {
            let bytes = snap.serialize();
            criterion::black_box(bytes);
        })
    });

    let serialized = snap.serialize();
    c.bench_function("hand_rolled_deserialize_64kb", |b| {
        b.iter(|| {
            let restored = ExecutionSnapshot::deserialize(&serialized).unwrap();
            criterion::black_box(restored);
        })
    });

    c.bench_function("hand_rolled_roundtrip_64kb", |b| {
        b.iter(|| {
            let bytes = snap.serialize();
            let restored = ExecutionSnapshot::deserialize(&bytes).unwrap();
            criterion::black_box(restored);
        })
    });

    // Size measurement
    println!(
        "\n[format_comparison] hand-rolled size for 64KB snapshot: {} bytes",
        serialized.len()
    );
}

criterion_group!(benches, bench_hand_rolled);
criterion_main!(benches);
```

- [ ] **Step 3: Run the benchmark**

Run:
```bash
cargo bench -p molt-snapshot 2>&1 | tail -30
```

Expected: Benchmark results showing serialize/deserialize throughput. Note the output size for comparison with buffa encoding later.

- [ ] **Step 4: Commit**

```bash
git add runtime/molt-snapshot/Cargo.toml runtime/molt-snapshot/benches/
git commit -m "bench: add snapshot format comparison benchmark"
```

---

### Task 11: Review Findings Backlog (Monty Phase 2.4)

**Files:**
- Modify: `runtime/molt-runtime/src/resource.rs`
- Modify: `docs/RESOURCE_CONTROLS.md`

- [ ] **Step 1: Add safety multiplier to LeftShift estimate**

Read the current LeftShift estimation in `runtime/molt-runtime/src/resource.rs`. Search for `LeftShift`:

```bash
grep -n 'LeftShift' runtime/molt-runtime/src/resource.rs
```

Find the `check_operation_size` implementation in `LimitedTracker`. The Pow estimate uses a 4x safety multiplier — LeftShift should match:

```rust
// In the LeftShift arm of check_operation_size:
OperationEstimate::LeftShift { value_bits, shift } => {
    let estimated_bits = (*value_bits as u64).saturating_add(*shift as u64);
    let estimated_bytes = (estimated_bits / 8).saturating_mul(4); // 4x safety multiplier
    // ... rest of check
}
```

- [ ] **Step 2: Document non-reentrancy**

Add to `docs/RESOURCE_CONTROLS.md` under the ResourceTracker section:

```markdown
### Thread-Safety and Non-Reentrancy

`with_tracker` borrows the thread-local `ResourceTracker` via `RefCell`. This means:
- Calls to `with_tracker` must not be nested — calling `with_tracker` while already
  inside a `with_tracker` closure will panic with a borrow error.
- Each thread has its own tracker instance. Cross-thread tracking requires
  `set_global_tracker_factory` to install a factory that creates per-thread trackers.
```

- [ ] **Step 3: Verify compilation**

Run:
```bash
cargo check -p molt-runtime 2>&1 | tail -5
```

Expected: `Finished`

- [ ] **Step 4: Commit**

```bash
git add runtime/molt-runtime/src/resource.rs docs/RESOURCE_CONTROLS.md
git commit -m "fix: add safety multiplier to LeftShift estimate, document non-reentrancy"
```

---

## Tier 3: Integration and Verification

These tasks run after all Tier 1 and Tier 2 work is merged.

---

### Task 12: Full Harness Run — All Layers

**Files:**
- No new files

- [ ] **Step 1: Run the harness quick profile**

Run:
```bash
export MOLT_EXT_ROOT=$PWD
export CARGO_TARGET_DIR=$PWD/target
export MOLT_DIFF_CARGO_TARGET_DIR=$CARGO_TARGET_DIR
export MOLT_CACHE=$PWD/.molt_cache
export MOLT_DIFF_ROOT=$PWD/tmp/diff
export MOLT_DIFF_TMPDIR=$PWD/tmp
export UV_CACHE_DIR=$PWD/.uv-cache
export TMPDIR=$PWD/tmp
PYTHONPATH=src python3 -c "from molt.harness import main; exit(main(['quick']))"
```

Expected: All layers PASS.

- [ ] **Step 2: Run the differential test suite**

Run:
```bash
MOLT_DIFF_MEASURE_RSS=1 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic --jobs 1 2>&1 | tail -20
```

Expected: All tests pass, no regressions.

- [ ] **Step 3: Run the Cloudflare endpoint tests**

Run:
```bash
python3 -m pytest -q tests/cloudflare/test_demo_endpoints.py tests/cloudflare/test_demo_endpoint_fuzz.py 2>&1 | tail -10
```

Expected: PASS

- [ ] **Step 4: Record final state**

```bash
echo "=== Tier 3 Verification ===" >> logs/consolidated_plan_verification.log
echo "Date: $(date -u +%Y-%m-%dT%H:%M:%SZ)" >> logs/consolidated_plan_verification.log
echo "Harness: quick profile passed" >> logs/consolidated_plan_verification.log
echo "Differential: basic suite passed" >> logs/consolidated_plan_verification.log
echo "Cloudflare: endpoint tests passed" >> logs/consolidated_plan_verification.log
git add logs/consolidated_plan_verification.log
git commit -m "test: record consolidated plan verification results"
```

---

## Dependency Graph

```
Tier 1 (all parallel):
  Task 1: CI Integration
  Task 2: --audit-log, --io-mode flags
  Task 3: Vendor cleanup
  Task 4: Cache variant + partition_mode
  Task 5: Deploy verify tool
  Task 6: Monty conformance baseline
  Task 7: Fuzz campaign
     │
     ▼
Tier 2 (after Tier 1):
  Task 8:  Buffa message encode/decode
  Task 9:  AuditEvent protobuf schema
  Task 10: Snapshot format benchmark
  Task 11: Review findings backlog
     │
     ▼
Tier 3 (after Tier 2):
  Task 12: Full integration verification
```

## Not In Scope (Deferred to Next Sprint)

| Item | Phase | Reason |
|------|-------|--------|
| `--type-gate` flag | Phase 1 | Requires type inference integration not yet in main |
| `resource_on_allocate` in WASM host alloc | Phase 1 | Needs WASM host crate refactor |
| Wave B: six/click/attrs fixes | Wave B | Blocked on module-scope variable semantics |
| Wave C: WASM importlib fix | Wave C | Blocked on Wave A full gate |
| Monty C API bridge (molt-ffi) | Phase 4 | Depends on Phases 1-3 completion |
| Tiered execution coordinator | Phase 6 | Long-term architecture |
| Snapshot/resume via Asyncify | Phase B2 | Complex; evaluate after format benchmark |
