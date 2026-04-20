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
import shutil
import signal
import socket
import subprocess
import sys
import time
from pathlib import Path

import pytest
import urllib.error
import urllib.request
import molt.cli as cli
import tools.bench_wasm as bench_wasm
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


def _split_runtime_target_dirs(env: dict[str, str]) -> tuple[Path, Path]:
    default_target_dir = ROOT / "target" / "pytest" / "test_wasm_split_runtime"
    raw_target = env.get("CARGO_TARGET_DIR", "").strip()
    target_dir = (
        Path(raw_target).expanduser() if raw_target else default_target_dir
    )
    raw_diff_target = env.get("MOLT_DIFF_CARGO_TARGET_DIR", "").strip()
    diff_target_dir = (
        Path(raw_diff_target).expanduser() if raw_diff_target else target_dir
    )
    return target_dir, diff_target_dir


def test_split_runtime_target_dir_respects_explicit_env_override() -> None:
    env = {
        "CARGO_TARGET_DIR": "/tmp/molt-explicit-target",
        "MOLT_DIFF_CARGO_TARGET_DIR": "/tmp/molt-explicit-diff-target",
    }

    target_dir, diff_target_dir = _split_runtime_target_dirs(env)

    assert target_dir == Path("/tmp/molt-explicit-target")
    assert diff_target_dir == Path("/tmp/molt-explicit-diff-target")


def test_split_runtime_target_dir_defaults_to_repo_pytest_target() -> None:
    target_dir, diff_target_dir = _split_runtime_target_dirs({})

    assert target_dir == ROOT / "target" / "pytest" / "test_wasm_split_runtime"
    assert diff_target_dir == target_dir


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
    assert "molt_gpu_webgpu_dispatch_host() { return -38; }" in worker_js


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
    target_dir, diff_target_dir = _split_runtime_target_dirs(env)
    target_dir.mkdir(parents=True, exist_ok=True)
    diff_target_dir.mkdir(parents=True, exist_ok=True)
    env["CARGO_TARGET_DIR"] = str(target_dir)
    env["MOLT_DIFF_CARGO_TARGET_DIR"] = str(diff_target_dir)
    env.setdefault("MOLT_SESSION_ID", "test-wasm-split-runtime")
    env.setdefault("CARGO_BUILD_JOBS", "1")
    env.setdefault("MOLT_BUILD_LOCK_TIMEOUT", "45")
    env.setdefault("MOLT_CARGO_TIMEOUT", "900")
    env.setdefault("MOLT_WASM_DISABLE_SCCACHE", "1")

    cmd = [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        str(source_file),
        "--target", "wasm",
        "--profile", "cloudflare",
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
    extra_env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    env = os.environ.copy()
    env["MOLT_WASM_DIRECT_LINK"] = "1"
    env["MOLT_WASM_PREFER_LINKED"] = "0"
    env["MOLT_RUNTIME_WASM"] = str(output_dir / "molt_runtime.wasm")
    if extra_env:
        env.update(extra_env)
    return subprocess.run(
        ["node", "wasm/run_wasm.js", str(output_dir / "app.wasm"), *argv],
        capture_output=True,
        text=True,
        env=env,
        cwd=str(ROOT),
        timeout=timeout,
    )


def _run_split_direct_host_exports(
    output_dir: Path,
    calls_path: Path,
    *,
    timeout: int = 60,
    extra_env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    env = os.environ.copy()
    env["MOLT_WASM_DIRECT_LINK"] = "1"
    env["MOLT_WASM_PREFER_LINKED"] = "0"
    env["MOLT_RUNTIME_WASM"] = str(output_dir / "molt_runtime.wasm")
    env["MOLT_WASM_EXPORT_CALLS_JSON"] = str(calls_path)
    if extra_env:
        env.update(extra_env)
    return subprocess.run(
        ["node", "wasm/run_wasm.js", str(output_dir / "app.wasm")],
        capture_output=True,
        text=True,
        env=env,
        cwd=str(ROOT),
        timeout=timeout,
    )


def _run_split_worker_live(
    output_dir: Path,
    path: str = "/",
    timeout: float = 120.0,
) -> tuple[int, str, str]:
    wrangler = shutil.which("wrangler")
    if wrangler is None:
        pytest.skip("wrangler is required for live split-runtime worker verification")

    port_socket = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    try:
        port_socket.bind(("127.0.0.1", 0))
        port = port_socket.getsockname()[1]
    finally:
        port_socket.close()

    env = os.environ.copy()
    env.setdefault("MOLT_SESSION_ID", "test-wasm-split-runtime-worker")
    proc = subprocess.Popen(
        [
            wrangler,
            "dev",
            "--local",
            "--ip",
            "127.0.0.1",
            "--port",
            str(port),
            "--config",
            str(output_dir / "wrangler.jsonc"),
        ],
        cwd=str(output_dir),
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        start_new_session=True,
    )

    def _terminate_worker_tree(sig: int) -> None:
        if proc.poll() is not None:
            return
        try:
            os.killpg(proc.pid, sig)
        except ProcessLookupError:
            return

    def _collect_logs() -> str:
        if proc.stdout is None:
            return ""
        _terminate_worker_tree(signal.SIGTERM)
        try:
            out, _ = proc.communicate(timeout=10)
        except subprocess.TimeoutExpired:
            _terminate_worker_tree(signal.SIGKILL)
            out, _ = proc.communicate()
        return out

    result: tuple[int, str] | None = None
    try:
        deadline = time.monotonic() + timeout
        last_error: Exception | None = None
        url = f"http://127.0.0.1:{port}{path}"
        while time.monotonic() < deadline:
            if proc.poll() is not None:
                break
            try:
                with urllib.request.urlopen(url, timeout=5) as resp:
                    body = resp.read().decode("utf-8", errors="replace")
                    result = (resp.status, body)
                    break
            except urllib.error.HTTPError as exc:
                body = exc.read().decode("utf-8", errors="replace")
                result = (exc.code, body)
                break
            except (urllib.error.URLError, OSError) as exc:
                last_error = exc
                time.sleep(0.5)
        if result is None:
            logs = _collect_logs()
            raise AssertionError(
                f"wrangler dev did not produce a response for {url} within {timeout:.0f}s\n"
                f"last error: {last_error!r}\n"
                f"logs:\n{logs}"
            )
        logs = _collect_logs()
        return result[0], result[1], logs
    finally:
        if proc.poll() is None:
            _terminate_worker_tree(signal.SIGTERM)
            try:
                proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                _terminate_worker_tree(signal.SIGKILL)
                proc.wait(timeout=10)


def test_split_runtime_compiled_gpu_kernel_vector_add_matches_expected_output(
    tmp_path: Path,
) -> None:
    src = tmp_path / "gpu_kernel_smoke.py"
    src.write_text(
        "import molt.gpu as gpu\n"
        "\n"
        "@gpu.kernel\n"
        "def vector_add(a, b, c, n):\n"
        "    tid = gpu.thread_id()\n"
        "    if tid < n:\n"
        "        c[tid] = a[tid] + b[tid]\n"
        "\n"
        "a = gpu.to_device([1.0, 2.0, 3.0, 4.0])\n"
        "b = gpu.to_device([10.0, 20.0, 30.0, 40.0])\n"
        "c = gpu.alloc(4, float)\n"
        "vector_add[1, 4](a, b, c, 4)\n"
        "print(gpu.from_device(c))\n",
        encoding="utf-8",
    )
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    build = _build_split(src, out_dir)
    assert build.returncode == 0, build.stdout + build.stderr

    run = _run_split_direct(out_dir, timeout=120)
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "[11.0, 22.0, 33.0, 44.0]"


def test_hostfed_call_bundle_parses_profile_and_classifies_timeout(
    monkeypatch,
    tmp_path: Path,
) -> None:
    app_wasm = tmp_path / "app.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    app_wasm.write_bytes(b"\0asm\x01\x00\x00\x00")
    runtime_wasm.write_bytes(b"\0asm\x01\x00\x00\x00")

    calls_path = tmp_path / "calls.json"
    calls_path.write_text(
        json.dumps({"calls": [{"export": "main_molt__init", "args": []}]}, indent=2) + "\n",
        encoding="utf-8",
    )

    def _fake_run_cmd(cmd, env, capture, tty, log, timeout_s=None):
        assert timeout_s == 12.5
        return bench_wasm._RunResult(
            returncode=124,
            stderr=(
                "# timeout after 12.5s (command aborted)\n"
                'molt_profile_json {"alloc_count": 9, "handle_resolve": 3}\n'
            ),
            timed_out=True,
        )

    monkeypatch.setattr(bench_wasm, "_run_cmd", _fake_run_cmd)

    payload = bench_wasm._run_hostfed_call_bundle(
        label="init_only",
        app_wasm=app_wasm,
        runtime_wasm=runtime_wasm,
        calls_path=calls_path,
        runner_cmd=["node", "wasm/run_wasm.js"],
        runner_name="node",
        log=None,
        timeout_s=12.5,
    )

    assert payload["ok"] is False
    assert payload["timed_out"] is True
    assert payload["timeout_s"] == 12.5
    assert payload["error_class"] == "runner_timeout"
    assert payload["profile"] == {"alloc_count": 9, "handle_resolve": 3}


def test_run_cmd_timeout_gracefully_collects_sigterm_output(tmp_path: Path) -> None:
    if os.name != "posix":
        pytest.skip("graceful SIGTERM timeout handling is POSIX-specific")

    script = tmp_path / "term_cleanup.py"
    script.write_text(
        "import signal\n"
        "import sys\n"
        "import time\n"
        "def _on_term(signum, frame):\n"
        "    sys.stderr.write('TERM_CLEANUP\\n')\n"
        "    sys.stderr.flush()\n"
        "    raise SystemExit(0)\n"
        "signal.signal(signal.SIGTERM, _on_term)\n"
        "while True:\n"
        "    time.sleep(1)\n",
        encoding="utf-8",
    )

    res = bench_wasm._run_cmd(
        [sys.executable, str(script)],
        env=os.environ.copy(),
        capture=True,
        tty=False,
        log=None,
        timeout_s=0.1,
    )

    assert res.timed_out is True
    assert res.returncode == 124
    assert "TERM_CLEANUP" in res.stderr


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
def test_split_runtime_app_exports_host_init(
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

    app_wasm = out_dir / "app.wasm"
    exports = cli._collect_wasm_export_names(app_wasm)
    assert "molt_host_init" in exports
    assert "molt_main" in exports


@pytest.mark.slow
def test_split_runtime_host_export_calls_decode_result_repr(
    tmp_path: Path,
) -> None:
    source = tmp_path / "host_call_smoke.py"
    source.write_text(
        "_state = None\n"
        "\n"
        "def init(weights: bytes, config_json: str) -> None:\n"
        "    global _state\n"
        "    _state = (len(weights), config_json)\n"
        "\n"
        "def infer(width: int, height: int, rgb: bytes, prompt_ids: list[int], max_new_tokens: int) -> list[int]:\n"
        "    if _state is None:\n"
        "        raise RuntimeError('not initialized')\n"
        "    return [width, height, len(rgb), max_new_tokens, _state[0], len(_state[1])] + prompt_ids\n",
        encoding="utf-8",
    )
    calls_path = tmp_path / "calls.json"
    calls_path.write_text(
        json.dumps(
            {
                "calls": [
                    {
                        "export": "host_call_smoke__init",
                        "args": [
                            {"kind": "bytes_utf8", "value": "weights-bytes"},
                            {"kind": "string", "value": "{\"k\":1}"},
                        ],
                    },
                    {
                        "export": "host_call_smoke__infer",
                        "args": [
                            {"kind": "int", "value": 8},
                            {"kind": "int", "value": 4},
                            {"kind": "bytes_utf8", "value": "abcdefghijklmnopqrstuvwx"},
                            {"kind": "list_int", "value": [7, 8, 9]},
                            {"kind": "int", "value": 3},
                        ],
                    },
                ]
            }
        ),
        encoding="utf-8",
    )
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    build = _build_split(source, out_dir)
    assert build.returncode == 0, (
        f"split build failed (rc={build.returncode}).\n"
        f"stdout:\n{build.stdout[-2000:]}\n"
        f"stderr:\n{build.stderr[-2000:]}"
    )

    run = _run_split_direct_host_exports(out_dir, calls_path, timeout=60)
    assert run.returncode == 0, (
        f"split direct host-call run failed (rc={run.returncode}).\n"
        f"stdout:\n{run.stdout[-2000:]}\n"
        f"stderr:\n{run.stderr[-2000:]}"
    )
    results = json.loads(run.stdout)
    assert results[0]["result_repr"] == "None"
    assert results[1]["result_repr"] == "[8, 4, 24, 3, 13, 7, 7, 8, 9]"


@pytest.mark.slow
def test_split_runtime_host_export_struct_unpack_from_reads_u64(
    tmp_path: Path,
) -> None:
    source = tmp_path / "struct_unpack_smoke.py"
    source.write_text(
        "import struct\n\n"
        "def read_qword(data: bytes) -> list[int]:\n"
        "    return [struct.unpack_from(\"<Q\", data, 0)[0]]\n",
        encoding="utf-8",
    )
    data_path = tmp_path / "data.bin"
    data_path.write_bytes(bytes([8, 7, 6, 5, 4, 3, 2, 1]))
    calls_path = tmp_path / "calls.json"
    calls_path.write_text(
        json.dumps(
            {
                "calls": [
                    {
                        "export": "struct_unpack_smoke__read_qword",
                        "args": [
                            {
                                "kind": "bytes_path",
                                "path": str(data_path),
                            }
                        ],
                    }
                ]
            }
        ),
        encoding="utf-8",
    )
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    build = _build_split(source, out_dir)
    assert build.returncode == 0, (
        f"split build failed (rc={build.returncode}).\n"
        f"stdout:\n{build.stdout[-2000:]}\n"
        f"stderr:\n{build.stderr[-2000:]}"
    )

    run = _run_split_direct_host_exports(out_dir, calls_path, timeout=60)
    assert run.returncode == 0, (
        f"split direct host-call run failed (rc={run.returncode}).\n"
        f"stdout:\n{run.stdout[-2000:]}\n"
        f"stderr:\n{run.stderr[-2000:]}"
    )
    results = json.loads(run.stdout)
    assert results[0]["result_repr"] == "[72623859790382856]"


@pytest.mark.slow
def test_split_runtime_profile_json_emits_on_wasm(
    tmp_path: Path,
) -> None:
    source = tmp_path / "profile_smoke.py"
    source.write_text('print("ok")\n', encoding="utf-8")
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    build = _build_split(source, out_dir)
    assert build.returncode == 0, (
        f"split build failed (rc={build.returncode}).\n"
        f"stdout:\n{build.stdout[-2000:]}\n"
        f"stderr:\n{build.stderr[-2000:]}"
    )

    run = _run_split_direct(
        out_dir,
        timeout=60,
        extra_env={"MOLT_PROFILE": "1", "MOLT_PROFILE_JSON": "1"},
    )
    assert run.returncode == 0, (
        f"split direct run failed (rc={run.returncode}).\n"
        f"stdout:\n{run.stdout[-2000:]}\n"
        f"stderr:\n{run.stderr[-2000:]}"
    )
    assert "ok" in run.stdout
    assert "molt_profile_json " in run.stderr


@pytest.mark.slow
def test_split_runtime_host_export_bytes_survive_raw_temp_cleanup(
    tmp_path: Path,
) -> None:
    source = tmp_path / "main.py"
    pkg_dir = tmp_path / "pkg"
    pkg_dir.mkdir()
    (pkg_dir / "__init__.py").write_text("# package marker\n", encoding="utf-8")
    (pkg_dir / "helper_mod.py").write_text(
        "import _intrinsics as _molt_intrinsics\n"
        "from molt.gpu import Buffer, alloc, to_device, from_device\n"
        "\n"
        "def _load_optional_intrinsic(name: str):\n"
        "    loader = getattr(_molt_intrinsics, 'load_intrinsic', None)\n"
        "    if callable(loader):\n"
        "        return loader(name)\n"
        "    require = getattr(_molt_intrinsics, 'require_intrinsic', None)\n"
        "    if callable(require):\n"
        "        try:\n"
        "            return require(name)\n"
        "        except RuntimeError:\n"
        "            return None\n"
        "    return None\n"
        "\n"
        "_MOLT_GPU_BUFFER_TO_LIST = _load_optional_intrinsic('molt_gpu_buffer_to_list')\n"
        "_MOLT_GPU_TENSOR_FROM_BUFFER = _load_optional_intrinsic('molt_gpu_tensor_from_buffer')\n"
        "_MOLT_GPU_TENSOR_FROM_PARTS = _load_optional_intrinsic('molt_gpu_tensor_from_parts')\n"
        "_MOLT_GPU_TENSOR_ZEROS = _load_optional_intrinsic('molt_gpu_tensor__zeros')\n"
        "_MOLT_GPU_REPEAT_AXIS_CONTIGUOUS = _load_optional_intrinsic('molt_gpu_repeat_axis_contiguous')\n"
        "_MOLT_GPU_LINEAR_CONTIGUOUS = _load_optional_intrinsic('molt_gpu_linear_contiguous')\n"
        "_MOLT_GPU_LINEAR_SPLIT_LAST_DIM_CONTIGUOUS = _load_optional_intrinsic('molt_gpu_linear_split_last_dim_contiguous')\n"
        "_MOLT_GPU_TENSOR_LINEAR_SPLIT_LAST_DIM = _load_optional_intrinsic('molt_gpu_tensor__tensor_linear_split_last_dim')\n"
        "_MOLT_GPU_TENSOR_SCALED_DOT_PRODUCT_ATTENTION = _load_optional_intrinsic('molt_gpu_tensor__tensor_scaled_dot_product_attention')\n"
        "_MOLT_GPU_LINEAR_SQUARED_RELU_GATE_INTERLEAVED_CONTIGUOUS = _load_optional_intrinsic('molt_gpu_linear_squared_relu_gate_interleaved_contiguous')\n"
        "_MOLT_GPU_BROADCAST_BINARY_CONTIGUOUS = _load_optional_intrinsic('molt_gpu_broadcast_binary_contiguous')\n"
        "_MOLT_GPU_MATMUL_CONTIGUOUS = _load_optional_intrinsic('molt_gpu_matmul_contiguous')\n"
        "_MOLT_GPU_PERMUTE_CONTIGUOUS = _load_optional_intrinsic('molt_gpu_permute_contiguous')\n"
        "_MOLT_GPU_RMS_NORM_LAST_AXIS_CONTIGUOUS = _load_optional_intrinsic('molt_gpu_rms_norm_last_axis_contiguous')\n"
        "_MOLT_GPU_SOFTMAX_LAST_AXIS_CONTIGUOUS = _load_optional_intrinsic('molt_gpu_softmax_last_axis_contiguous')\n"
        "_MOLT_GPU_SQUARED_RELU_GATE_INTERLEAVED_CONTIGUOUS = _load_optional_intrinsic('molt_gpu_squared_relu_gate_interleaved_contiguous')\n"
        "\n"
        "class Huge:\n"
        "    def m0(self):\n"
        "        return 0\n"
        "    def m1(self):\n"
        "        return 1\n"
        "    def m2(self):\n"
        "        return 2\n"
        "    def m3(self):\n"
        "        return 3\n"
        "    def m4(self):\n"
        "        return 4\n"
        "    def m5(self):\n"
        "        return 5\n"
        "    def m6(self):\n"
        "        return 6\n"
        "    def m7(self):\n"
        "        return 7\n"
        "    def m8(self):\n"
        "        return 8\n"
        "    def m9(self):\n"
        "        return 9\n"
        "    def m10(self):\n"
        "        return 10\n"
        "    def m11(self):\n"
        "        return 11\n"
        "    def m12(self):\n"
        "        return 12\n"
        "    def m13(self):\n"
        "        return 13\n"
        "    def m14(self):\n"
        "        return 14\n"
        "    def m15(self):\n"
        "        return 15\n"
        "    def m16(self):\n"
        "        return 16\n"
        "    def m17(self):\n"
        "        return 17\n"
        "    def m18(self):\n"
        "        return 18\n"
        "    def m19(self):\n"
        "        return 19\n"
        "    def m20(self):\n"
        "        return 20\n"
        "    def m21(self):\n"
        "        return 21\n"
        "    def m22(self):\n"
        "        return 22\n"
        "    def m23(self):\n"
        "        return 23\n"
        "    def m24(self):\n"
        "        return 24\n"
        "    def m25(self):\n"
        "        return 25\n"
        "    def m26(self):\n"
        "        return 26\n"
        "    def m27(self):\n"
        "        return 27\n"
        "    def m28(self):\n"
        "        return 28\n"
        "    def m29(self):\n"
        "        return 29\n"
        "    def m30(self):\n"
        "        return 30\n"
        "    def m31(self):\n"
        "        return 31\n"
        "    def m32(self):\n"
        "        return 32\n"
        "    def m33(self):\n"
        "        return 33\n"
        "    def m34(self):\n"
        "        return 34\n"
        "    def m35(self):\n"
        "        return 35\n"
        "    def m36(self):\n"
        "        return 36\n"
        "    def m37(self):\n"
        "        return 37\n"
        "    def m38(self):\n"
        "        return 38\n"
        "    def m39(self):\n"
        "        return 39\n"
        "    @property\n"
        "    def p0(self):\n"
        "        return 0\n"
        "    @property\n"
        "    def p1(self):\n"
        "        return 1\n",
        encoding="utf-8",
    )
    source.write_text(
        "from pkg.helper_mod import Huge\n\n"
        "def main__probe(data: bytes):\n"
        "    return [data[0], len(data)]\n",
        encoding="utf-8",
    )
    data_path = tmp_path / "data.bin"
    payload = (
        b'O\\x00\\x00\\x00\\x00\\x00\\x00\\x00'
        b'{\"x\":{\"dtype\":\"F32\",\"shape\":[1],\"data_offsets\":[0,4]},\"__metadata__\":{\"a\":\"b\"}}'
        b'\\x00\\x00\\x60\\x40'
    )
    data_path.write_bytes(payload)
    calls_path = tmp_path / "calls.json"
    calls_path.write_text(
        json.dumps(
            {
                "calls": [
                    {
                        "export": "main__main__probe",
                        "args": [{"kind": "bytes_path", "path": str(data_path)}],
                    }
                ]
            }
        ),
        encoding="utf-8",
    )
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    build = _build_split(source, out_dir)
    assert build.returncode == 0, (
        f"split build failed (rc={build.returncode}).\n"
        f"stdout:\n{build.stdout[-2000:]}\n"
        f"stderr:\n{build.stderr[-2000:]}"
    )

    run = _run_split_direct_host_exports(out_dir, calls_path, timeout=120)
    assert run.returncode == 0, (
        f"split direct host-call run failed (rc={run.returncode}).\n"
        f"stdout:\n{run.stdout[-2000:]}\n"
        f"stderr:\n{run.stderr[-2000:]}"
    )
    results = json.loads(run.stdout)
    assert results[0]["result_repr"] == "[79, 91]"


@pytest.mark.slow
def test_cloudflare_demo_root_route_completes_under_split_runtime_worker(
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

    status, body, logs = _run_split_worker_live(out_dir, "/")
    assert status == 200, (
        f"split-runtime worker returned HTTP {status}.\n"
        f"body:\n{body[-2000:]}\n"
        f"logs:\n{logs[-4000:]}"
    )
    assert "Python compiled to WebAssembly." in body


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

    def test_worker_builds_signature_aware_runtime_imports(self, split_build_a):
        content = self._read_worker(split_build_a)
        assert "const buildRuntimeImports = (module, runtimeInstance) => {" in content, (
            "worker.js must synthesize runtime imports from the app import surface"
        )
        assert "const runtimeImportSignatures =" in content
        assert "const runtimeImportResultKinds =" in content
        assert "molt_string_from_bytes" in content

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

    def test_worker_exposes_wasi_fallback_imports(self, split_build_a):
        content = self._read_worker(split_build_a)
        assert "path_filestat_set_times" in content
        assert "path_link" in content
        assert "path_symlink" in content

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
