# Wave C: WASM First-Class Target Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Promote WASM to full semantic parity with native — fix `importlib.machinery`, finish stdlib partition, harden the WASM artifact/link/deploy pipeline, and optimize size/startup.

**Architecture:** Five tracks: C1 fixes WASM importlib (depends on Wave A gate), C2 finishes stdlib partition cache/link fingerprinting (independent, starts immediately), C3 hardens the WASM artifact pipeline (depends on C1), C4 hardens Cloudflare deploy (depends on C3), C5 optimizes WASM size/startup (depends on C1+C2).

**Tech Stack:** Python CLI, Rust wasm.rs backend, wasm-encoder/wasmparser, pytest, wasm_link.py, Cloudflare Workers/wrangler

**Spec:** `docs/superpowers/specs/2026-03-27-operation-greenfield-design.md`

---

### Task 1: Finish Stdlib Partition — Cache-Mode Versioning (Track C2, independent)

**Files:**
- Modify: `src/molt/cli.py` (cache identity computation)
- Modify: `tests/cli/test_cli_import_collection.py`

- [ ] **Step 1: Write the failing cache-mode versioning test**

Add to `tests/cli/test_cli_import_collection.py`:
```python
def test_stdlib_partition_mode_changes_cache_identity(tmp_path, monkeypatch):
    """Cache identity must differ when stdlib partition mode changes."""
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path))
    # Import the cache identity helpers from molt.cli
    from molt.cli import _backend_cache_variant

    variant_mono = _backend_cache_variant(
        profile="dev", target="native", partition_mode=False
    )
    variant_part = _backend_cache_variant(
        profile="dev", target="native", partition_mode=True
    )
    assert variant_mono != variant_part, (
        "Cache variant must change when partition mode changes"
    )
```

- [ ] **Step 2: Run the test to verify it fails**

Run:
```bash
UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py -k stdlib_partition_mode_changes_cache
```

Expected: FAIL (no `partition_mode` parameter in `_backend_cache_variant` yet).

- [ ] **Step 3: Implement cache-mode versioning**

In `src/molt/cli.py`, modify the cache variant computation (near `_backend_cache_variant` or equivalent) to include a `partition_mode` boolean that changes the hash. When partition mode is enabled, append `":partitioned:v1"` to the cache key before hashing. This ensures monolithic cache entries cannot be reused under the split pipeline.

- [ ] **Step 4: Run the test to verify it passes**

Run:
```bash
UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py -k stdlib_partition_mode_changes_cache
```

Expected: PASS.

- [ ] **Step 5: Commit**

Run:
```bash
git add src/molt/cli.py tests/cli/test_cli_import_collection.py
git commit -m "feat: cache identity encodes stdlib partition mode"
```

### Task 2: Finish Stdlib Partition — Link Fingerprinting (Track C2)

**Files:**
- Modify: `src/molt/cli.py:18783-18814` (`_link_fingerprint()`)
- Modify: `tests/cli/test_cli_import_collection.py`

- [ ] **Step 1: Write the failing link fingerprint test**

Add to `tests/cli/test_cli_import_collection.py`:
```python
def test_stdlib_link_fingerprint_changes_on_partition_artifact(tmp_path, monkeypatch):
    """Link fingerprint must change when any stdlib partition artifact changes."""
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path))
    from molt.cli import _link_fingerprint

    # Create two fake stdlib partition artifacts
    stdlib_a = tmp_path / "stdlib_a.o"
    stdlib_b = tmp_path / "stdlib_a.o"
    stdlib_a.write_bytes(b"artifact_v1")

    fp1 = _link_fingerprint(
        link_cmd=["cc", "-o", "out"],
        input_artifacts=[str(stdlib_a)],
    )

    # Change the artifact content
    stdlib_a.write_bytes(b"artifact_v2")
    fp2 = _link_fingerprint(
        link_cmd=["cc", "-o", "out"],
        input_artifacts=[str(stdlib_a)],
    )

    assert fp1["hash"] != fp2["hash"], (
        "Link fingerprint must change when stdlib partition artifact content changes"
    )
```

- [ ] **Step 2: Run the test to verify it fails**

Run:
```bash
UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py -k stdlib_link_fingerprint_changes
```

Expected: FAIL (current `_link_fingerprint` may not hash individual artifact contents, or may not accept `input_artifacts` as a parameter).

- [ ] **Step 3: Implement explicit artifact-list link fingerprinting**

In `src/molt/cli.py`, modify `_link_fingerprint()` (near line 18783) to:
1. Accept an explicit `input_artifacts: list[str]` parameter listing all stdlib partition artifacts
2. Hash each artifact's content (not just its path) into the fingerprint
3. Include the artifact count in the hash so adding/removing artifacts changes the fingerprint

The existing `_hash_runtime_file()` helper can be reused for content hashing.

- [ ] **Step 4: Run the test to verify it passes**

Run:
```bash
UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py -k stdlib_link_fingerprint_changes
```

Expected: PASS.

- [ ] **Step 5: Run the broader CLI tests for regression**

Run:
```bash
UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py
```

Expected: all 237+ tests pass.

- [ ] **Step 6: Commit**

Run:
```bash
git add src/molt/cli.py tests/cli/test_cli_import_collection.py
git commit -m "feat: link fingerprint hashes explicit stdlib partition artifact contents"
```

### Task 3: Fix `importlib.machinery` in WASM (Track C1, depends on Wave A)

**Files:**
- Modify: `src/molt/stdlib/importlib/__init__.py:72-123`
- Modify: `src/molt/stdlib/importlib/machinery.py:789-885`
- Modify: `runtime/molt-backend/src/wasm.rs` (if import resolution gap)
- Modify: `tests/test_wasm_importlib_machinery.py`

- [ ] **Step 1: Write the failing WASM importlib.machinery test**

Add to `tests/test_wasm_importlib_machinery.py`:
```python
def test_wasm_linked_import_importlib_machinery(wasm_linked_runner):
    """importlib.machinery must resolve in the WASM linked runner."""
    result = wasm_linked_runner("import importlib.machinery; print(type(importlib.machinery).__name__)")
    assert result.returncode == 0, f"WASM importlib.machinery failed: {result.stderr}"
    assert "module" in result.stdout.strip().lower()
```

- [ ] **Step 2: Run the test to verify it fails**

Run:
```bash
PYTHONPATH=src uv run --python 3.12 python3 -m pytest -q tests/test_wasm_importlib_machinery.py -k wasm_linked_import_importlib_machinery
```

Expected: FAIL with `ImportError: No module named 'importlib.machinery'`.

- [ ] **Step 3: Diagnose the WASM import resolution boundary**

The issue: `importlib.machinery` is a submodule. The importlib `__init__.py` at line 72-123 uses `_MOLT_IMPORTLIB_RESOLVE_NAME` intrinsic for top-level resolution, but submodule resolution (dotted imports) may not properly route through the WASM import boundary.

Check:
1. Does the `import_module()` function in `importlib/__init__.py` handle dotted names like `importlib.machinery`?
2. Does the WASM runtime expose `importlib.machinery` in its module table?
3. Is `machinery.py` included in the WASM compilation graph?

- [ ] **Step 4: Fix the import resolution boundary**

The fix must be at the importlib boundary (`src/molt/stdlib/importlib/__init__.py`), not a caller-specific shim. Ensure:
1. `import_module("importlib.machinery")` resolves to the machinery submodule
2. The version-gated absence behavior stays centralized
3. The fix works for both native and WASM lanes

- [ ] **Step 5: Run the test to verify it passes**

Run:
```bash
PYTHONPATH=src uv run --python 3.12 python3 -m pytest -q tests/test_wasm_importlib_machinery.py -k wasm_linked_import_importlib_machinery
```

Expected: PASS.

- [ ] **Step 6: Run WASM benchmark probes**

Run:
```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/bench_wasm.py --bench tests/benchmarks/bench_sum.py --linked --output bench/results/bench_wasm_importlib_fixed.json
```

Expected: completes without ImportError.

- [ ] **Step 7: Commit**

Run:
```bash
git add src/molt/stdlib/importlib/__init__.py tests/test_wasm_importlib_machinery.py
git commit -m "fix: importlib.machinery resolves in WASM via centralized import boundary"
```

### Task 4: Land and Harden WASM Artifact Pipeline (Track C3, depends on C1)

**Files:**
- Modify: `tests/cli/test_cli_wasm_artifact_validation.py`
- Modify: `tests/test_wasm_link_validation.py`
- Modify: `tests/wasm_linked_runner.py`
- Modify: `tools/wasm_link.py`
- Modify: `tools/bench_wasm.py`

- [ ] **Step 1: Commit the uncommitted WASM test and tooling changes**

The working tree has ~1,562 lines of uncommitted changes across these files. Review, stage, and commit them:

Run:
```bash
git diff --stat tests/cli/test_cli_wasm_artifact_validation.py tests/test_wasm_link_validation.py tests/wasm_linked_runner.py tools/wasm_link.py tools/bench_wasm.py
```

Review each file's changes, then:
```bash
git add tests/cli/test_cli_wasm_artifact_validation.py tests/test_wasm_link_validation.py tests/wasm_linked_runner.py tools/wasm_link.py tools/bench_wasm.py
git commit -m "test: land WASM artifact validation and link validation tests"
```

- [ ] **Step 2: Run all WASM tests**

Run:
```bash
PYTHONPATH=src uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_wasm_artifact_validation.py tests/test_wasm_link_validation.py tests/test_wasm_importlib_machinery.py
```

Expected: all tests pass.

- [ ] **Step 3: Add negative-path coverage for malformed WASM artifacts**

Add tests to `tests/test_wasm_link_validation.py` for:
1. Truncated WASM binary (valid magic but truncated sections)
2. WASM binary with invalid section order
3. WASM binary with missing code section
4. WASM link with mismatched import/export signatures

Each test should verify that the tooling produces a deterministic, human-readable error message — not a panic or silent corruption.

- [ ] **Step 4: Run all WASM tests including new negative-path tests**

Run:
```bash
PYTHONPATH=src uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_wasm_artifact_validation.py tests/test_wasm_link_validation.py
```

Expected: all pass.

- [ ] **Step 5: Commit**

Run:
```bash
git add tests/test_wasm_link_validation.py
git commit -m "test: add negative-path WASM artifact validation coverage"
```

### Task 5: Harden Cloudflare Split-Runtime Deploy (Track C4, depends on C3)

**Files:**
- Modify: `src/molt/cli.py:21599-21769` (`_deploy()`)
- Modify: `tests/cli/test_cli_import_collection.py`

- [ ] **Step 1: Write failing Cloudflare negative-path tests**

Add to `tests/cli/test_cli_import_collection.py`:
```python
def test_deploy_cloudflare_rejects_missing_bundle_root(monkeypatch, tmp_path):
    """Cloudflare deploy must fail deterministically when bundle_root is missing."""
    from molt.cli import _deploy
    # Simulate build result with no bundle_root
    # Assert deterministic error, not silent failure or KeyError

def test_deploy_cloudflare_rejects_missing_wrangler_config(monkeypatch, tmp_path):
    """Cloudflare deploy must fail when wrangler_config is absent from artifacts."""
    # Simulate build result with bundle_root but no wrangler_config artifact

def test_deploy_cloudflare_rejects_wrangler_config_outside_bundle_root(monkeypatch, tmp_path):
    """Cloudflare deploy must reject wrangler_config that escapes bundle_root."""
    # Simulate build result where wrangler_config path traverses outside bundle_root

def test_deploy_cloudflare_split_runtime_explicit_output(monkeypatch, tmp_path):
    """Split-runtime deploy with explicit --output/--out-dir must work."""
    # Simulate build result with explicit output directory
```

- [ ] **Step 2: Run the tests to verify they fail**

Run:
```bash
UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py -k 'deploy_cloudflare_rejects or deploy_cloudflare_split'
```

Expected: FAIL (missing validation in `_deploy()`).

- [ ] **Step 3: Implement the Cloudflare negative-path validation**

In `src/molt/cli.py`'s `_deploy()` function (near line 21688):
1. After retrieving `bundle_root` from build contract (line 21698), raise a clear `MoltDeployError` if it's None or doesn't exist
2. After retrieving `wrangler_config` from artifacts (line 21705), raise if absent
3. Validate that `wrangler_config` is under `bundle_root` (path traversal check)
4. Support explicit `--output` / `--out-dir` for split-runtime deploy

- [ ] **Step 4: Run the tests to verify they pass**

Run:
```bash
UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py -k 'deploy_cloudflare_rejects or deploy_cloudflare_split'
```

Expected: PASS.

- [ ] **Step 5: Run the full CLI test suite**

Run:
```bash
UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py
```

Expected: all 237+ tests pass.

- [ ] **Step 6: Commit**

Run:
```bash
git add src/molt/cli.py tests/cli/test_cli_import_collection.py
git commit -m "fix: Cloudflare deploy validates bundle_root, wrangler_config, path containment"
```

### Task 6: WASM Size/Startup Optimization (Track C5, depends on C1+C2)

**Files:**
- Modify: `runtime/molt-backend/src/wasm.rs:3695-3791` (trampolines)
- Modify: `runtime/molt-backend/src/wasm.rs:1134-1159` (data segments)

- [ ] **Step 1: Measure baseline WASM binary size and cold start**

Run:
```bash
PYTHONPATH=src MOLT_BACKEND_DAEMON=0 uv run --python 3.12 python3 -m molt.cli build --target wasm --profile dev -o tmp/wasm_baseline.wasm examples/hello.py
ls -la tmp/wasm_baseline.wasm
wc -c tmp/wasm_baseline.wasm > bench/results/wasm_size_baseline.txt
```

Record the baseline binary size.

- [ ] **Step 2: Identify trampoline deduplication opportunities**

The WASM backend generates trampolines for each indirectly-called function (lines 3695-3791). If multiple functions share the same arity and closure flag, their trampolines are identical except for the target function index. These can share a common trampoline body with a table-dispatch prefix.

Analyze:
```bash
grep -c "compile_trampoline" runtime/molt-backend/src/wasm.rs
```

Count how many unique trampoline shapes exist vs. total trampolines generated.

- [ ] **Step 3: Implement shared trampoline helpers**

Refactor `compile_trampoline()` to:
1. Group functions by `(arity, has_closure)` — each group gets one shared trampoline body
2. The shared body reads the actual function index from a dispatch table
3. Each function's indirect table entry points to the shared trampoline with its index encoded

This reduces code section size proportionally to the number of unique arity/closure combinations vs. total function count.

- [ ] **Step 4: Measure size improvement**

Run:
```bash
PYTHONPATH=src MOLT_BACKEND_DAEMON=0 uv run --python 3.12 python3 -m molt.cli build --target wasm --profile dev -o tmp/wasm_deduped.wasm examples/hello.py
wc -c tmp/wasm_deduped.wasm > bench/results/wasm_size_deduped.txt
diff bench/results/wasm_size_baseline.txt bench/results/wasm_size_deduped.txt
```

Expected: measurable size reduction.

- [ ] **Step 5: Run all WASM tests to verify no regressions**

Run:
```bash
PYTHONPATH=src uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_wasm_artifact_validation.py tests/test_wasm_link_validation.py tests/test_wasm_importlib_machinery.py
PYTHONPATH=src uv run --python 3.12 python3 tools/bench_wasm.py --bench tests/benchmarks/bench_sum.py --linked --output bench/results/bench_wasm_deduped.json
```

Expected: all tests pass, benchmark completes.

- [ ] **Step 6: Commit**

Run:
```bash
git add runtime/molt-backend/src/wasm.rs bench/results/wasm_size_baseline.txt bench/results/wasm_size_deduped.txt
git commit -m "perf: deduplicate WASM trampolines by arity — reduces code section size"
```

### Task 7: Wave C Exit Gate

- [ ] **Step 1: Run the full WASM exit gate validation**

Run all commands — every one must pass:
```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/bench_wasm.py --bench tests/benchmarks/bench_sum.py --linked
PYTHONPATH=src uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_wasm_artifact_validation.py tests/test_wasm_link_validation.py tests/test_wasm_importlib_machinery.py
UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py -k 'stdlib_partition or stdlib_link_fingerprint or deploy_cloudflare'
UV_CACHE_DIR=$PWD/.uv-cache uv run --python 3.12 python3 -m pytest -q tests/cli/test_cli_import_collection.py
```

Expected: all green.

- [ ] **Step 2: Record WASM benchmark artifacts**

Run:
```bash
PYTHONPATH=src uv run --python 3.12 python3 tools/bench_wasm.py --bench tests/benchmarks/bench_sum.py --linked --output bench/results/bench_wasm_wave_c_exit.json
```

- [ ] **Step 3: Update canonical status docs**

Update `docs/spec/STATUS.md` and `ROADMAP.md` with:
- importlib.machinery WASM: FIXED
- stdlib partition cache versioning: DONE
- stdlib partition link fingerprinting: DONE
- Cloudflare deploy negative-path validation: DONE
- WASM trampoline deduplication: DONE (with size delta)

- [ ] **Step 4: Refresh Linear workspace artifacts**

Run:
```bash
python3 tools/linear_hygiene.py refresh-local-artifacts --repo-root .
```

- [ ] **Step 5: Commit status updates**

Run:
```bash
git add docs/spec/STATUS.md ROADMAP.md
git commit -m "docs: update status — Wave C WASM first-class target complete"
```
