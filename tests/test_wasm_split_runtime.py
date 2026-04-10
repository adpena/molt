"""End-to-end tests for split-runtime WASM deployment.

Exercises the full --split-runtime pipeline:
  1. Build a Python program with --split-runtime
  2. Verify output directory contains expected artifacts
  3. Verify file sizes are reasonable
  4. Verify worker.js contains key runtime patterns
  5. Verify manifest.json structure
  6. Verify two different programs produce identical molt_runtime.wasm (CDN cacheability)
"""
from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest
from tests.wasm_linked_runner import _read_timeout_seconds

ROOT = Path(__file__).resolve().parents[1]

# Two deliberately different programs to verify runtime identity.
PROGRAM_A = """\
class Point:
    x: int
    y: int

p = Point()
p.x = 10
p.y = 32
print(p.x + p.y)
"""

PROGRAM_B = """\
def fib(n: int) -> int:
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)

for i in range(10):
    print(fib(i))
"""


def test_generate_split_worker_js_lifecycle_contract() -> None:
    from molt.cli import _generate_split_worker_js

    worker_js = _generate_split_worker_js(
        shared_memory_initial_pages=8,
        shared_table_initial=16,
        shared_table_base=4096,
    )

    assert "\x00" not in worker_js
    assert 'encoder.encode(a + "\\0")' in worker_js
    assert 'encoder.encode(e + "\\0")' in worker_js
    assert "const stdoutDecoder = new TextDecoder();" in worker_js
    assert "const stderrDecoder = new TextDecoder();" in worker_js
    assert "rtInstance.exports.molt_runtime_shutdown" in worker_js
    assert "molt_set_wasm_table_base(BigInt(4096))" in worker_js


def test_build_isolate_import_ops_initializes_code_slots() -> None:
    from molt.cli import _build_isolate_import_ops

    ops = _build_isolate_import_ops(
        code_slot_count=17,
        module_order=["sys"],
        register_global_code_id=lambda _symbol: 123,
        per_module_code_ops={
            "sys": [
                {"kind": "const_none", "out": "v0"},
                {"kind": "code_slot_set", "value": 9, "args": ["v0"]},
            ]
        },
    )

    assert ops[0] == {"kind": "code_slots_init", "value": 17}
    assert any(op.get("kind") == "code_slot_set" for op in ops)
    assert any(op.get("s_value") == "molt_init_sys" for op in ops)


def _build_split(source_file: Path, output_dir: Path) -> subprocess.CompletedProcess:
    """Run ``molt build --target wasm --split-runtime`` and return the result."""
    env = os.environ.copy()
    repo_src = str(ROOT / "src")
    current_pythonpath = env.get("PYTHONPATH", "")
    env["PYTHONPATH"] = (
        repo_src + os.pathsep + current_pythonpath
        if current_pythonpath
        else repo_src
    )
    env["MOLT_BACKEND_DAEMON"] = "0"
    target_dir = ROOT / "target" / "pytest" / "test_wasm_split_runtime"
    target_dir.mkdir(parents=True, exist_ok=True)
    env["CARGO_TARGET_DIR"] = str(target_dir)
    env["MOLT_DIFF_CARGO_TARGET_DIR"] = str(target_dir)
    env.setdefault("MOLT_SESSION_ID", "test-wasm-split-runtime")

    cmd = [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        str(source_file),
        "--target", "wasm",
        "--split-runtime",
        "--no-cache",
        "--out-dir", str(output_dir),
    ]
    build_timeout = _read_timeout_seconds("MOLT_WASM_TEST_BUILD_TIMEOUT_SEC", 900.0)
    return subprocess.run(
        cmd,
        capture_output=True,
        text=True,
        env=env,
        cwd=str(ROOT),
        timeout=build_timeout,
    )


def _run_split_direct(
    output_dir: Path,
    *argv: str,
    timeout: int = 60,
) -> subprocess.CompletedProcess[str]:
    env = os.environ.copy()
    env["MOLT_WASM_DIRECT_LINK"] = "1"
    env["MOLT_WASM_PREFER_LINKED"] = "0"
    env["MOLT_RUNTIME_WASM"] = str(output_dir / "molt_runtime.wasm")
    return subprocess.run(
        ["node", "wasm/run_wasm.js", str(output_dir / "app.wasm"), *argv],
        capture_output=True,
        text=True,
        env=env,
        cwd=str(ROOT),
        timeout=timeout,
    )


def _sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _collect_module_imports(path: Path, module_name: str) -> list[str]:
    text = subprocess.check_output(
        ["wasm-tools", "print", str(path)],
        cwd=str(ROOT),
        text=True,
    )
    imports: list[str] = []
    for line in text.splitlines():
        stripped = line.strip()
        prefix = f'(import "{module_name}" "'
        if not stripped.startswith(prefix):
            continue
        remainder = stripped[len(prefix) :]
        name, _, _ = remainder.partition('"')
        imports.append(name)
    return imports


def _collect_export_names(path: Path) -> list[str]:
    text = subprocess.check_output(
        ["wasm-tools", "print", str(path)],
        cwd=str(ROOT),
        text=True,
    )
    exports: list[str] = []
    prefix = '(export "'
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped.startswith(prefix):
            continue
        remainder = stripped[len(prefix) :]
        name, _, _ = remainder.partition('"')
        exports.append(name)
    return exports


def _reserved_runtime_callable_indices() -> list[int]:
    include_path = ROOT / "runtime" / "wasm_runtime_callables.inc"
    indices: list[int] = []
    pattern = re.compile(r"^\s*\((\d+),")
    for line in include_path.read_text().splitlines():
        match = pattern.match(line)
        if match:
            indices.append(int(match.group(1)))
    return indices


def _infer_wasm_table_base_from_reserved_refs(path: Path) -> int | None:
    export_names = _collect_export_names(path)
    ref_indices = sorted(
        int(name.removeprefix("__molt_table_ref_"))
        for name in export_names
        if name.startswith("__molt_table_ref_")
    )
    if not ref_indices:
        return None

    reserved_indices = _reserved_runtime_callable_indices()
    reserved_count = len(reserved_indices)
    ref_set = set(ref_indices)
    shared_abi_prefix_len = 33 + reserved_count * 2

    for ref_index in ref_indices:
        expected = {
            ref_index + offset for offset in range(shared_abi_prefix_len)
        }
        if expected.issubset(ref_set):
            return ref_index

    return None


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

@pytest.fixture(scope="module")
def split_build_a(tmp_path_factory):
    """Build PROGRAM_A with --split-runtime and return the output directory."""
    base = tmp_path_factory.mktemp("split_a")
    src = base / "prog_a.py"
    src.write_text(PROGRAM_A)
    out_dir = base / "out"
    out_dir.mkdir()
    result = _build_split(src, out_dir)
    return out_dir, result


@pytest.fixture(scope="module")
def split_build_b(tmp_path_factory):
    """Build PROGRAM_B with --split-runtime and return the output directory."""
    base = tmp_path_factory.mktemp("split_b")
    src = base / "prog_b.py"
    src.write_text(PROGRAM_B)
    out_dir = base / "out"
    out_dir.mkdir()
    result = _build_split(src, out_dir)
    return out_dir, result


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------

@pytest.mark.slow
class TestSplitRuntimeArtifacts:
    """Verify the split-runtime build produces all expected artifacts."""

    def test_build_succeeds(self, split_build_a):
        out_dir, result = split_build_a
        assert result.returncode == 0, (
            f"Build failed (rc={result.returncode}).\n"
            f"stdout:\n{result.stdout[-2000:]}\n"
            f"stderr:\n{result.stderr[-2000:]}"
        )

    def test_expected_files_exist(self, split_build_a):
        out_dir, result = split_build_a
        if result.returncode != 0:
            pytest.skip("build failed")
        expected = [
            "app.wasm",
            "molt_runtime.wasm",
            "molt_vfs_browser.js",
            "worker.js",
            "manifest.json",
            "wrangler.jsonc",
        ]
        for name in expected:
            assert (out_dir / name).exists(), f"Missing artifact: {name}"
        assert not (out_dir / "wrangler.toml").exists()

    def test_app_wasm_size(self, split_build_a):
        out_dir, result = split_build_a
        if result.returncode != 0:
            pytest.skip("build failed")
        app_wasm = out_dir / "app.wasm"
        if not app_wasm.exists():
            pytest.skip("app.wasm not produced")
        size_mb = app_wasm.stat().st_size / (1024 * 1024)
        assert size_mb < 1, f"app.wasm is {size_mb:.2f} MB, expected < 1 MB"

    def test_app_wasm_smaller_than_raw_output_module(self, split_build_a):
        out_dir, result = split_build_a
        if result.returncode != 0:
            pytest.skip("build failed")
        app_wasm = out_dir / "app.wasm"
        raw_output = out_dir / "output.wasm"
        if not app_wasm.exists() or not raw_output.exists():
            pytest.skip("split-runtime app/raw output not produced")
        assert app_wasm.stat().st_size < raw_output.stat().st_size, (
            "split-runtime app.wasm should be deforested below the raw rewritten "
            "output.wasm artifact"
        )

    def test_runtime_wasm_size(self, split_build_a):
        out_dir, result = split_build_a
        if result.returncode != 0:
            pytest.skip("build failed")
        rt_wasm = out_dir / "molt_runtime.wasm"
        if not rt_wasm.exists():
            pytest.skip("molt_runtime.wasm not produced")
        size_mb = rt_wasm.stat().st_size / (1024 * 1024)
        assert size_mb < 5, f"molt_runtime.wasm is {size_mb:.2f} MB, expected < 5 MB"

    def test_app_wasm_retains_runtime_abi_imports(self, split_build_a):
        out_dir, result = split_build_a
        if result.returncode != 0:
            pytest.skip("build failed")
        app_wasm = out_dir / "app.wasm"
        if not app_wasm.exists():
            pytest.skip("app.wasm not produced")
        runtime_imports = _collect_module_imports(app_wasm, "molt_runtime")
        assert runtime_imports, "app.wasm must retain molt_runtime imports in split mode"
        assert "molt_string_from_bytes" in runtime_imports
        assert "molt_module_import" in runtime_imports


    def test_worker_uses_backend_wasm_table_base(self, split_build_a):
        out_dir, result = split_build_a
        if result.returncode != 0:
            pytest.skip("build failed")
        app_wasm = out_dir / "app.wasm"
        worker_js = out_dir / "worker.js"
        if not app_wasm.exists() or not worker_js.exists():
            pytest.skip("split-runtime artifacts not produced")

        wasm_table_base = _infer_wasm_table_base_from_reserved_refs(app_wasm)
        assert wasm_table_base is not None, (
            "app.wasm must export a canonical reserved runtime callable/trampoline "
            "ref block so the split-runtime worker can recover wasm_table_base"
        )

        worker_content = worker_js.read_text()
        assert (
            f"molt_set_wasm_table_base(BigInt({wasm_table_base}))" in worker_content
        ), (
            "worker.js must propagate the backend's wasm_table_base into the runtime; "
            f"expected {wasm_table_base}"
        )


@pytest.mark.slow
def test_cloudflare_demo_root_route_completes_under_split_runtime(
    tmp_path: Path,
) -> None:
    source = ROOT / "examples" / "cloudflare-demo" / "src" / "app.py"
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    build = _build_split(source, out_dir)
    assert build.returncode == 0, (
        f"split build failed (rc={build.returncode}).\n"
        f"stdout:\n{build.stdout[-2000:]}\n"
        f"stderr:\n{build.stderr[-2000:]}"
    )

    run = _run_split_direct(out_dir, "/", timeout=45)
    assert run.returncode == 0, (
        f"direct-link run failed (rc={run.returncode}).\n"
        f"stdout:\n{run.stdout[-2000:]}\n"
        f"stderr:\n{run.stderr[-2000:]}"
    )
    assert "Python compiled to WebAssembly." in run.stdout


@pytest.mark.slow
class TestWorkerJsContent:
    """Verify worker.js contains key runtime patterns."""

    def _read_worker(self, split_build_a):
        out_dir, result = split_build_a
        if result.returncode != 0:
            pytest.skip("build failed")
        worker = out_dir / "worker.js"
        if not worker.exists():
            pytest.skip("worker.js not produced")
        return worker.read_text()

    def test_shared_table(self, split_build_a):
        content = self._read_worker(split_build_a)
        assert "sharedTable" in content, "worker.js must reference sharedTable"

    def test_molt_runtime_import(self, split_build_a):
        content = self._read_worker(split_build_a)
        assert "molt_runtime" in content, "worker.js must reference molt_runtime"

    def test_worker_bridges_call_indirect(self, split_build_a):
        content = self._read_worker(split_build_a)
        assert "molt_call_indirect" in content, "worker.js must bridge runtime call_indirect imports"

    def test_worker_bridges_isolate_import(self, split_build_a):
        content = self._read_worker(split_build_a)
        assert "molt_isolate_import" in content, "worker.js must bridge runtime isolate imports"

    def test_worker_remaps_runtime_abi_exports(self, split_build_a):
        content = self._read_worker(split_build_a)
        assert "runtimeAbiExports" in content, (
            "worker.js must remap prefixed runtime exports onto the molt_runtime ABI"
        )
        assert 'name.startsWith("molt_")' in content

    def test_worker_sets_runtime_table_base(self, split_build_a):
        content = self._read_worker(split_build_a)
        assert "molt_set_wasm_table_base" in content, (
            "worker.js must propagate the computed wasm table base into the runtime"
        )

    def test_worker_runs_runtime_shutdown(self, split_build_a):
        content = self._read_worker(split_build_a)
        assert "molt_runtime_shutdown" in content, (
            "worker.js must shut the runtime down so stdio buffers flush"
        )

    def test_worker_provisions_shared_memory(self, split_build_a):
        content = self._read_worker(split_build_a)
        assert "new WebAssembly.Memory" in content, "worker.js must provision shared memory"

    def test_worker_imports_split_vfs_adapter(self, split_build_a):
        content = self._read_worker(split_build_a)
        assert 'import "./molt_vfs_browser.js";' in content
        assert "new globalThis.MoltVfs()" in content

    def test_worker_exposes_vfs_host_imports(self, split_build_a):
        content = self._read_worker(split_build_a)
        assert "molt_vfs_read" in content
        assert "molt_vfs_write" in content
        assert "molt_vfs_exists" in content
        assert "molt_vfs_unlink" in content

    def test_runtime_wasm_import(self, split_build_a):
        content = self._read_worker(split_build_a)
        assert "molt_runtime.wasm" in content, "worker.js must import molt_runtime.wasm"

    def test_app_wasm_import(self, split_build_a):
        content = self._read_worker(split_build_a)
        assert "app.wasm" in content, "worker.js must import app.wasm"

    def test_worker_uses_escaped_nul_terminators(self, split_build_a):
        content = self._read_worker(split_build_a)
        assert "\x00" not in content, "worker.js must not embed literal NUL bytes"
        assert 'encoder.encode(a + "\\0")' in content
        assert 'encoder.encode(e + "\\0")' in content


@pytest.mark.slow
class TestManifestJson:
    """Verify manifest.json has the correct structure."""

    def _read_manifest(self, split_build_a):
        out_dir, result = split_build_a
        if result.returncode != 0:
            pytest.skip("build failed")
        manifest = out_dir / "manifest.json"
        if not manifest.exists():
            pytest.skip("manifest.json not produced")
        return json.loads(manifest.read_text())

    def test_version(self, split_build_a):
        data = self._read_manifest(split_build_a)
        assert data["version"] == 2

    def test_mode(self, split_build_a):
        data = self._read_manifest(split_build_a)
        assert data["mode"] == "split-runtime"

    def test_tree_shaken(self, split_build_a):
        data = self._read_manifest(split_build_a)
        assert data["tree_shaken"] is True

    def test_shared_table_initial(self, split_build_a):
        data = self._read_manifest(split_build_a)
        assert data["shared_table_initial"] == 8192

    def test_shared_memory_initial_pages(self, split_build_a):
        data = self._read_manifest(split_build_a)
        assert isinstance(data["shared_memory_initial_pages"], int)
        assert data["shared_memory_initial_pages"] >= 1

    def test_modules_structure(self, split_build_a):
        data = self._read_manifest(split_build_a)
        modules = data["modules"]
        assert "runtime" in modules
        assert "app" in modules
        assert modules["runtime"]["path"] == "molt_runtime.wasm"
        assert modules["app"]["path"] == "app.wasm"
        assert isinstance(modules["runtime"]["size"], int)
        assert isinstance(modules["app"]["size"], int)
        assert modules["runtime"]["size"] > 0
        assert modules["app"]["size"] > 0

    def test_instantiation_order(self, split_build_a):
        data = self._read_manifest(split_build_a)
        assert data["instantiation_order"] == ["runtime", "app"]

    def test_entry_point(self, split_build_a):
        data = self._read_manifest(split_build_a)
        entry = data["entry"]
        assert entry["module"] == "app"
        assert entry["function"] == "molt_main"

    def test_total_size_consistent(self, split_build_a):
        data = self._read_manifest(split_build_a)
        expected = data["modules"]["runtime"]["size"] + data["modules"]["app"]["size"]
        assert data["total_size"] == expected


@pytest.mark.slow
class TestRuntimeCacheability:
    """Two different programs must produce identical molt_runtime.wasm for CDN caching."""

    def test_runtime_hash_identical(self, split_build_a, split_build_b):
        out_a, result_a = split_build_a
        out_b, result_b = split_build_b
        if result_a.returncode != 0 or result_b.returncode != 0:
            pytest.skip("one or both builds failed")
        rt_a = out_a / "molt_runtime.wasm"
        rt_b = out_b / "molt_runtime.wasm"
        if not rt_a.exists() or not rt_b.exists():
            pytest.skip("molt_runtime.wasm not produced in both builds")
        hash_a = _sha256(rt_a)
        hash_b = _sha256(rt_b)
        assert hash_a == hash_b, (
            f"molt_runtime.wasm differs between two builds — CDN caching will break.\n"
            f"  Program A runtime hash: {hash_a}\n"
            f"  Program B runtime hash: {hash_b}\n"
            f"  Program A runtime size: {rt_a.stat().st_size}\n"
            f"  Program B runtime size: {rt_b.stat().st_size}"
        )

    def test_app_wasm_differs(self, split_build_a, split_build_b):
        """Sanity check: the app modules should be different."""
        out_a, result_a = split_build_a
        out_b, result_b = split_build_b
        if result_a.returncode != 0 or result_b.returncode != 0:
            pytest.skip("one or both builds failed")
        app_a = out_a / "app.wasm"
        app_b = out_b / "app.wasm"
        if not app_a.exists() or not app_b.exists():
            pytest.skip("app.wasm not produced in both builds")
        hash_a = _sha256(app_a)
        hash_b = _sha256(app_b)
        assert hash_a != hash_b, (
            "app.wasm is identical for two different programs — "
            "split may not be working correctly"
        )
