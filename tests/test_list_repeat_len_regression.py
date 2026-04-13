from __future__ import annotations

import os
import shutil
import subprocess
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest


SCRIPT = (
    "def f(token_ids):\n"
    "    in_block = [False] * len(token_ids)\n"
    "    block_idx = [-1] * len(token_ids)\n"
    "    depth = 0\n"
    "    current_block = -1\n"
    "    for i, tid in enumerate(token_ids):\n"
    "        is_soi = tid == 244\n"
    "        is_eoi = tid == 230\n"
    "        if is_soi:\n"
    "            depth += 1\n"
    "            current_block += 1\n"
    "        if depth > 0:\n"
    "            in_block[i] = True\n"
    "            block_idx[i] = current_block\n"
    "        if is_eoi and depth > 0:\n"
    "            depth -= 1\n"
    "    return in_block, block_idx\n"
    "\n"
    "x, y = f([229, 244, 245, 246, 247, 248, 227, 230])\n"
    "print(x)\n"
    "print(y)\n"
    "print(len(x))\n"
    "print(len(y))\n"
)


def _native_env(root: Path) -> dict[str, str]:
    env = os.environ.copy()
    env["PYTHONPATH"] = str(root / "src")
    return env


def _browser_wasm_build_env(root: Path) -> dict[str, str]:
    env = os.environ.copy()
    env["PYTHONPATH"] = str(root / "src")
    env.setdefault("CARGO_BUILD_JOBS", "1")
    env.setdefault("MOLT_WASM_DISABLE_SCCACHE", "1")
    env.setdefault("MOLT_BUILD_LOCK_TIMEOUT", "45")
    env.setdefault("MOLT_CARGO_TIMEOUT", "900")
    env.setdefault("MOLT_BACKEND_DAEMON", "0")
    return env


def _expected_lines() -> list[str]:
    return [
        "[False, True, True, True, True, True, True, True]",
        "[-1, 0, 0, 0, 0, 0, 0, 0]",
        "8",
        "8",
    ]


def test_list_repeat_len_regression_native(tmp_path: Path) -> None:
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for native regression test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "list_repeat_len_native.py"
    src.write_text(SCRIPT, encoding="utf-8")

    run = subprocess.run(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "run",
            "--profile",
            "dev",
            str(src),
        ],
        cwd=root,
        env=_native_env(root),
        capture_output=True,
        text=True,
        timeout=900,
    )
    assert run.returncode == 0, run.stderr
    lines = [line.strip() for line in run.stdout.splitlines() if line.strip()]
    assert lines == _expected_lines()


def test_list_repeat_len_regression_split_browser_host(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser host regression test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser host regression test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "list_repeat_len_browser.py"
    src.write_text(SCRIPT, encoding="utf-8")

    build = subprocess.run(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            str(src),
            "--build-profile",
            "dev",
            "--profile",
            "browser",
            "--target",
            "wasm",
            "--split-runtime",
            "--out-dir",
            str(tmp_path),
        ],
        cwd=root,
        env=_browser_wasm_build_env(root),
        capture_output=True,
        text=True,
        timeout=900,
    )
    assert build.returncode == 0, build.stderr

    app_wasm = tmp_path / "app.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    assert app_wasm.exists()
    assert runtime_wasm.exists()

    class _WasmHandler(BaseHTTPRequestHandler):
        def log_message(self, fmt: str, *args: object) -> None:
            return None

        def do_GET(self) -> None:  # noqa: N802
            if self.path == "/app.wasm":
                payload = app_wasm.read_bytes()
            elif self.path == "/molt_runtime.wasm":
                payload = runtime_wasm.read_bytes()
            else:
                self.send_response(404)
                self.end_headers()
                return
            self.send_response(200)
            self.send_header("content-type", "application/wasm")
            self.send_header("content-length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

    server = ThreadingHTTPServer(("127.0.0.1", 0), _WasmHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        base_url = f"http://127.0.0.1:{server.server_address[1]}"
        browser_host_uri = (root / "wasm" / "browser_host.js").as_uri()
        script = tmp_path / "run_list_repeat_len_browser.mjs"
        script.write_text(
            f"""
import {{ loadMoltWasm }} from {browser_host_uri!r};

const baseUrl = {base_url!r};
const host = await loadMoltWasm({{
  wasmUrl: `${{baseUrl}}/app.wasm`,
  runtimeUrl: `${{baseUrl}}/molt_runtime.wasm`,
  preferLinked: false,
}});
host.run();
""".lstrip(),
            encoding="utf-8",
        )
        run = subprocess.run(
            ["node", str(script)],
            cwd=root,
            capture_output=True,
            text=True,
            timeout=120,
        )
        assert run.returncode == 0, run.stderr
        lines = [line.strip() for line in run.stdout.splitlines() if line.strip()]
        assert lines == _expected_lines()
    finally:
        server.shutdown()
