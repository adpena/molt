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
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest

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
    return subprocess.run(
        cmd,
        capture_output=True,
        text=True,
        env=env,
        cwd=str(ROOT),
        timeout=300,
    )


def _sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


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
        expected = ["app.wasm", "molt_runtime.wasm", "worker.js", "manifest.json", "wrangler.toml"]
        for name in expected:
            assert (out_dir / name).exists(), f"Missing artifact: {name}"

    def test_app_wasm_size(self, split_build_a):
        out_dir, result = split_build_a
        if result.returncode != 0:
            pytest.skip("build failed")
        app_wasm = out_dir / "app.wasm"
        if not app_wasm.exists():
            pytest.skip("app.wasm not produced")
        size_mb = app_wasm.stat().st_size / (1024 * 1024)
        assert size_mb < 5, f"app.wasm is {size_mb:.2f} MB, expected < 5 MB"

    def test_runtime_wasm_size(self, split_build_a):
        out_dir, result = split_build_a
        if result.returncode != 0:
            pytest.skip("build failed")
        rt_wasm = out_dir / "molt_runtime.wasm"
        if not rt_wasm.exists():
            pytest.skip("molt_runtime.wasm not produced")
        size_mb = rt_wasm.stat().st_size / (1024 * 1024)
        assert size_mb > 1, f"molt_runtime.wasm is {size_mb:.2f} MB, expected > 1 MB"


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

    def test_runtime_wasm_import(self, split_build_a):
        content = self._read_worker(split_build_a)
        assert "molt_runtime.wasm" in content, "worker.js must import molt_runtime.wasm"

    def test_app_wasm_import(self, split_build_a):
        content = self._read_worker(split_build_a)
        assert "app.wasm" in content, "worker.js must import app.wasm"


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
