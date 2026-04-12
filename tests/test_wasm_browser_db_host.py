import base64
import json
import os
import shutil
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import ClassVar

import pytest


ARROW_BYTES = b"ARROW1"


class _DbHostHandler(BaseHTTPRequestHandler):
    output_wasm: ClassVar[bytes] = b""
    runtime_wasm: ClassVar[bytes] = b""

    def log_message(self, fmt: str, *args: object) -> None:
        return None

    def _send_bytes(self, payload: bytes, content_type: str) -> None:
        self.send_response(200)
        self.send_header("content-type", content_type)
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def do_GET(self) -> None:  # noqa: N802
        if self.path == "/output.wasm":
            self._send_bytes(self.output_wasm, "application/wasm")
            return
        if self.path == "/molt_runtime.wasm":
            self._send_bytes(self.runtime_wasm, "application/wasm")
            return
        self.send_response(404)
        self.end_headers()

    def do_POST(self) -> None:  # noqa: N802
        if self.path != "/db":
            self.send_response(404)
            self.end_headers()
            return
        length = int(self.headers.get("content-length", "0"))
        body = self.rfile.read(length)
        try:
            payload = json.loads(body)
        except json.JSONDecodeError:
            self.send_response(400)
            self.end_headers()
            return
        payload_b64 = payload.get("payload_b64", "")
        try:
            payload_bytes = base64.b64decode(payload_b64)
        except Exception:
            payload_bytes = b""
        if payload_bytes == b"slow":
            time.sleep(0.2)
        response = {
            "status": "Ok",
            "codec": "arrow_ipc",
            "payload_b64": base64.b64encode(ARROW_BYTES).decode("ascii"),
            "metrics": {"db_row_count": 1},
        }
        data = json.dumps(response).encode("utf-8")
        try:
            self._send_bytes(data, "application/json")
        except BrokenPipeError:
            return None


def _browser_wasm_build_env(root: Path) -> dict[str, str]:
    env = os.environ.copy()
    env["PYTHONPATH"] = str(root / "src")
    env["MOLT_WASM_LINKED"] = "0"
    # Keep browser wasm test builds deterministic and bounded on laptops.
    env.setdefault("CARGO_BUILD_JOBS", "1")
    env.setdefault("MOLT_WASM_DISABLE_SCCACHE", "1")
    env.setdefault("MOLT_BUILD_LOCK_TIMEOUT", "45")
    env.setdefault("MOLT_CARGO_TIMEOUT", "900")
    env.setdefault("MOLT_BACKEND_DAEMON", "0")
    return env


def test_browser_host_direct_mode_bridges_isolate_import(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser host direct-mode isolate test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser host direct-mode isolate test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "browser_host_direct.py"
    src.write_text(
        "import asyncio\n"
        "\n"
        "async def main():\n"
        "    print('ok')\n"
        "\n"
        "asyncio.run(main())\n"
    )

    build_env = _browser_wasm_build_env(root)
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
            "--out-dir",
            str(tmp_path),
        ],
        cwd=root,
        env=build_env,
        capture_output=True,
        text=True,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "output.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    assert output_wasm.exists()
    assert runtime_wasm.exists()

    class _DirectHostHandler(BaseHTTPRequestHandler):
        def log_message(self, fmt: str, *args: object) -> None:
            return None

        def do_GET(self) -> None:  # noqa: N802
            if self.path == "/output.wasm":
                payload = output_wasm.read_bytes()
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

    server = ThreadingHTTPServer(("127.0.0.1", 0), _DirectHostHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        base_url = f"http://127.0.0.1:{server.server_address[1]}"
        browser_host_uri = (root / "wasm" / "browser_host.js").as_uri()
        script = tmp_path / "run_browser_direct.mjs"
        script.write_text(
            f"""
import {{ loadMoltWasm }} from '{browser_host_uri}';

const baseUrl = {base_url!r};
const host = await loadMoltWasm({{
  wasmUrl: `${{baseUrl}}/output.wasm`,
  runtimeUrl: `${{baseUrl}}/molt_runtime.wasm`,
  preferLinked: false,
}});
host.run();
""".lstrip()
        )
        run = subprocess.run(
            ["node", str(script)],
            cwd=root,
            capture_output=True,
            text=True,
        )
        assert run.returncode == 0, run.stderr
        lines = [line.strip() for line in run.stdout.splitlines() if line.strip()]
        assert lines == ["ok"]
    finally:
        server.shutdown()


def test_browser_host_direct_mode_run_bootstraps_split_runtime_once(
    tmp_path: Path,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser host direct-mode isolate test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser host direct-mode isolate test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "browser_host_bootstrap_once.py"
    src.write_text(
        "import abc\n"
        "print('after')\n",
        encoding="utf-8",
    )

    build_env = _browser_wasm_build_env(root)
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
            "--out-dir",
            str(tmp_path),
        ],
        cwd=root,
        env=build_env,
        capture_output=True,
        text=True,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "output.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    assert output_wasm.exists()
    assert runtime_wasm.exists()

    class _DirectHostHandler(BaseHTTPRequestHandler):
        def log_message(self, fmt: str, *args: object) -> None:
            return None

        def do_GET(self) -> None:  # noqa: N802
            if self.path == "/output.wasm":
                payload = output_wasm.read_bytes()
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

    server = ThreadingHTTPServer(("127.0.0.1", 0), _DirectHostHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        base_url = f"http://127.0.0.1:{server.server_address[1]}"
        browser_host_uri = (root / "wasm" / "browser_host.js").as_uri()
        script = tmp_path / "run_browser_bootstrap_once.mjs"
        script.write_text(
            f"""
import {{ loadMoltWasm }} from '{browser_host_uri}';

const baseUrl = {base_url!r};
const host = await loadMoltWasm({{
  wasmUrl: `${{baseUrl}}/output.wasm`,
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
            timeout=20,
        )
        assert run.returncode == 0, run.stderr
        lines = [line.strip() for line in run.stdout.splitlines() if line.strip()]
        assert lines == ["after"]
    finally:
        server.shutdown()
def test_browser_host_direct_mode_import_stat_constants(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser host direct-mode isolate test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser host direct-mode isolate test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "browser_host_stat.py"
    src.write_text(
        "import stat\n"
        "print(type(stat._constants).__name__)\n"
        "print(len(stat._constants))\n"
        "print(stat.S_IFDIR)\n"
    )

    build_env = _browser_wasm_build_env(root)
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
            "--out-dir",
            str(tmp_path),
        ],
        cwd=root,
        env=build_env,
        capture_output=True,
        text=True,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "output.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    assert output_wasm.exists()
    assert runtime_wasm.exists()

    class _DirectHostHandler(BaseHTTPRequestHandler):
        def log_message(self, fmt: str, *args: object) -> None:
            return None

        def do_GET(self) -> None:  # noqa: N802
            if self.path == "/output.wasm":
                payload = output_wasm.read_bytes()
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

    server = ThreadingHTTPServer(("127.0.0.1", 0), _DirectHostHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        base_url = f"http://127.0.0.1:{server.server_address[1]}"
        browser_host_uri = (root / "wasm" / "browser_host.js").as_uri()
        script = tmp_path / "run_browser_direct_stat.mjs"
        script.write_text(
            f"""
import {{ loadMoltWasm }} from '{browser_host_uri}';

const baseUrl = {base_url!r};
const host = await loadMoltWasm({{
  wasmUrl: `${{baseUrl}}/output.wasm`,
  runtimeUrl: `${{baseUrl}}/molt_runtime.wasm`,
  preferLinked: false,
}});
host.run();
""".lstrip()
        )
        run = subprocess.run(
            ["node", str(script)],
            cwd=root,
            capture_output=True,
            text=True,
        )
        assert run.returncode == 0, run.stderr
        assert [line.strip() for line in run.stdout.splitlines() if line.strip()] == [
            "tuple",
            "71",
            "16384",
        ]
    finally:
        server.shutdown()


def test_browser_host_direct_mode_can_invoke_export_with_host_args(
    tmp_path: Path,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser host direct-mode export test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser host direct-mode export test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "browser_export_probe.py"
    src.write_text(
        "def echo(width: int, prompt_ids: list[int], rgb: bytes, label: str):\n"
        "    print(width)\n"
        "    print(len(rgb))\n"
        "    print(label)\n"
        "    return prompt_ids\n",
        encoding="utf-8",
    )

    build_env = _browser_wasm_build_env(root)
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
            "--out-dir",
            str(tmp_path),
        ],
        cwd=root,
        env=build_env,
        capture_output=True,
        text=True,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "output.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    assert output_wasm.exists()
    assert runtime_wasm.exists()

    class _DirectHostHandler(BaseHTTPRequestHandler):
        def log_message(self, fmt: str, *args: object) -> None:
            return None

        def do_GET(self) -> None:  # noqa: N802
            if self.path == "/output.wasm":
                payload = output_wasm.read_bytes()
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

    server = ThreadingHTTPServer(("127.0.0.1", 0), _DirectHostHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        base_url = f"http://127.0.0.1:{server.server_address[1]}"
        browser_host_uri = (root / "wasm" / "browser_host.js").as_uri()
        script = tmp_path / "run_browser_export_call.mjs"
        script.write_text(
            f"""
import {{ loadMoltWasm }} from '{browser_host_uri}';

const baseUrl = {base_url!r};
const host = await loadMoltWasm({{
  wasmUrl: `${{baseUrl}}/output.wasm`,
  runtimeUrl: `${{baseUrl}}/molt_runtime.wasm`,
  preferLinked: false,
}});
const result = await host.invokeExport('browser_export_probe__echo', [
  896,
  [257, 258],
  new Uint8Array([1, 2, 3, 4]),
  'falcon',
]);
console.log(JSON.stringify(result));
""".lstrip(),
            encoding="utf-8",
        )
        run = subprocess.run(
            ["node", str(script)],
            cwd=root,
            capture_output=True,
            text=True,
        )
        assert run.returncode == 0, run.stderr
        lines = [line.strip() for line in run.stdout.splitlines() if line.strip()]
        assert lines[:3] == ["896", "4", "falcon"]
        payload = json.loads(lines[3])
        assert isinstance(payload["resultBits"], str)
        assert payload["resultJson"] == [257, 258]
    finally:
        server.shutdown()


def test_browser_host_direct_mode_scalar_and_none_results_do_not_poison_next_export(
    tmp_path: Path,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser host direct-mode export test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser host direct-mode export test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "browser_export_results_probe.py"
    src.write_text(
        "def ret_none():\n"
        "    return None\n"
        "def ret_int(value: int):\n"
        "    return value\n"
        "def ret_list(a: int, b: int):\n"
        "    return [a, b]\n",
        encoding="utf-8",
    )

    build_env = _browser_wasm_build_env(root)
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
            "--out-dir",
            str(tmp_path),
        ],
        cwd=root,
        env=build_env,
        capture_output=True,
        text=True,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "output.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    assert output_wasm.exists()
    assert runtime_wasm.exists()

    class _DirectHostHandler(BaseHTTPRequestHandler):
        def log_message(self, fmt: str, *args: object) -> None:
            return None

        def do_GET(self) -> None:  # noqa: N802
            if self.path == "/output.wasm":
                payload = output_wasm.read_bytes()
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

    server = ThreadingHTTPServer(("127.0.0.1", 0), _DirectHostHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        base_url = f"http://127.0.0.1:{server.server_address[1]}"
        browser_host_uri = (root / "wasm" / "browser_host.js").as_uri()
        script = tmp_path / "run_browser_export_results.mjs"
        script.write_text(
            f"""
import {{ loadMoltWasm }} from '{browser_host_uri}';

const baseUrl = {base_url!r};
const host = await loadMoltWasm({{
  wasmUrl: `${{baseUrl}}/output.wasm`,
  runtimeUrl: `${{baseUrl}}/molt_runtime.wasm`,
  preferLinked: false,
}});
const noneResult = await host.invokeExport('browser_export_results_probe__ret_none', []);
const intResult = await host.invokeExport('browser_export_results_probe__ret_int', [7]);
const listResult = await host.invokeExport('browser_export_results_probe__ret_list', [7, 8]);
console.log(JSON.stringify({{
  noneResult,
  intResult,
  listResult,
}}));
""".lstrip(),
            encoding="utf-8",
        )
        run = subprocess.run(
            ["node", str(script)],
            cwd=root,
            capture_output=True,
            text=True,
        )
        assert run.returncode == 0, run.stderr
        payload = json.loads(run.stdout)
        assert payload["noneResult"]["resultJson"] is None
        assert payload["intResult"]["resultJson"] == 7
        assert payload["listResult"]["resultJson"] == [7, 8]
    finally:
        server.shutdown()


def test_browser_host_direct_mode_import_asyncio_iov_max(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser host direct-mode isolate test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser host direct-mode isolate test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "browser_host_asyncio_iov.py"
    src.write_text(
        "import asyncio\n"
        "print(asyncio.selector_events.SC_IOV_MAX)\n"
    )

    build_env = _browser_wasm_build_env(root)
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
            "--out-dir",
            str(tmp_path),
        ],
        cwd=root,
        env=build_env,
        capture_output=True,
        text=True,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "output.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    assert output_wasm.exists()
    assert runtime_wasm.exists()

    class _DirectHostHandler(BaseHTTPRequestHandler):
        def log_message(self, fmt: str, *args: object) -> None:
            return None

        def do_GET(self) -> None:  # noqa: N802
            if self.path == "/output.wasm":
                payload = output_wasm.read_bytes()
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

    server = ThreadingHTTPServer(("127.0.0.1", 0), _DirectHostHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        base_url = f"http://127.0.0.1:{server.server_address[1]}"
        browser_host_uri = (root / "wasm" / "browser_host.js").as_uri()
        script = tmp_path / "run_browser_direct_asyncio_iov.mjs"
        script.write_text(
            f"""
import {{ loadMoltWasm }} from '{browser_host_uri}';

const baseUrl = {base_url!r};
const host = await loadMoltWasm({{
  wasmUrl: `${{baseUrl}}/output.wasm`,
  runtimeUrl: `${{baseUrl}}/molt_runtime.wasm`,
  preferLinked: false,
}});
host.run();
""".lstrip()
        )
        run = subprocess.run(
            ["node", str(script)],
            cwd=root,
            capture_output=True,
            text=True,
        )
        assert run.returncode == 0, run.stderr
        assert [line.strip() for line in run.stdout.splitlines() if line.strip()] == ["1024"]
    finally:
        server.shutdown()


def test_browser_direct_run_wasm_import_os_name(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser direct-mode os.name test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser direct-mode os.name test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "browser_direct_os_name.py"
    src.write_text(
        "import os\n"
        "print(os.name)\n"
    )

    build_env = _browser_wasm_build_env(root)
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
            "--out-dir",
            str(tmp_path),
        ],
        cwd=root,
        env=build_env,
        capture_output=True,
        text=True,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "output.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    assert output_wasm.exists()
    assert runtime_wasm.exists()

    run_env = os.environ.copy()
    run_env["MOLT_WASM_PREFER_LINKED"] = "0"
    run_env["MOLT_RUNTIME_WASM"] = str(runtime_wasm)
    run = subprocess.run(
        ["node", str(root / "wasm" / "run_wasm.js"), str(output_wasm)],
        cwd=root,
        env=run_env,
        capture_output=True,
        text=True,
    )
    assert run.returncode == 0, run.stderr
    assert [line.strip() for line in run.stdout.splitlines() if line.strip()] == ["posix"]


def test_browser_direct_run_wasm_bool_or_call_result(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser direct-mode bool-or test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser direct-mode bool-or test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "browser_direct_bool_or.py"
    src.write_text(
        "from _intrinsics import require_intrinsic\n"
        "cap = require_intrinsic('molt_capabilities_has')\n"
        "print(cap('time.wall'))\n"
        "print(cap('time'))\n"
        "print(bool(cap('time.wall') or cap('time')))\n"
    )

    build_env = _browser_wasm_build_env(root)
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
            "--out-dir",
            str(tmp_path),
        ],
        cwd=root,
        env=build_env,
        capture_output=True,
        text=True,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "output.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    assert output_wasm.exists()
    assert runtime_wasm.exists()

    run_env = os.environ.copy()
    run_env["MOLT_WASM_PREFER_LINKED"] = "0"
    run_env["MOLT_RUNTIME_WASM"] = str(runtime_wasm)
    run_env["MOLT_CAPABILITY_TIER"] = "full"
    run = subprocess.run(
        ["node", str(root / "wasm" / "run_wasm.js"), str(output_wasm)],
        cwd=root,
        env=run_env,
        capture_output=True,
        text=True,
    )
    assert run.returncode == 0, run.stderr
    assert [line.strip() for line in run.stdout.splitlines() if line.strip()] == [
        "True",
        "False",
        "True",
    ]


def test_browser_direct_run_wasm_namedtuple_replace(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser direct-mode namedtuple test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser direct-mode namedtuple test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "browser_direct_namedtuple.py"
    src.write_text(
        "from collections import namedtuple\n"
        "\n"
        "T = namedtuple('T', ['a', 'b'])\n"
        "print(T(1, 2)._replace(a=3))\n"
    )

    build_env = _browser_wasm_build_env(root)
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
            "--out-dir",
            str(tmp_path),
        ],
        cwd=root,
        env=build_env,
        capture_output=True,
        text=True,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "output.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    assert output_wasm.exists()
    assert runtime_wasm.exists()

    run_env = os.environ.copy()
    run_env["MOLT_WASM_PREFER_LINKED"] = "0"
    run_env["MOLT_RUNTIME_WASM"] = str(runtime_wasm)
    run = subprocess.run(
        ["node", str(root / "wasm" / "run_wasm.js"), str(output_wasm)],
        cwd=root,
        env=run_env,
        capture_output=True,
        text=True,
        timeout=20,
    )
    assert run.returncode == 0, run.stderr
    assert [line.strip() for line in run.stdout.splitlines() if line.strip()] == [
        "T(a=3, b=2)"
    ]


def test_browser_direct_run_wasm_slots_function_field_roundtrip(
    tmp_path: Path,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser direct-mode slots test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser direct-mode slots test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "browser_direct_slots_fn.py"
    src.write_text(
        "class Box:\n"
        "    __slots__ = ('value',)\n"
        "\n"
        "    def __init__(self):\n"
        "        self.value = None\n"
        "\n"
        "def ident(x):\n"
        "    return x\n"
        "\n"
        "box = Box()\n"
        "box.value = ident\n"
        "print(box.value is ident)\n"
        "print(box.value(7))\n"
    )

    build_env = _browser_wasm_build_env(root)
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
            "--out-dir",
            str(tmp_path),
        ],
        cwd=root,
        env=build_env,
        capture_output=True,
        text=True,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "output.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    assert output_wasm.exists()
    assert runtime_wasm.exists()

    run_env = os.environ.copy()
    run_env["MOLT_WASM_PREFER_LINKED"] = "0"
    run_env["MOLT_RUNTIME_WASM"] = str(runtime_wasm)
    run = subprocess.run(
        ["node", str(root / "wasm" / "run_wasm.js"), str(output_wasm)],
        cwd=root,
        env=run_env,
        capture_output=True,
        text=True,
        timeout=20,
    )
    assert run.returncode == 0, run.stderr
    assert [line.strip() for line in run.stdout.splitlines() if line.strip()] == [
        "True",
        "7",
    ]


def test_browser_direct_run_wasm_enumerate_tuple(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser direct-mode enumerate test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser direct-mode enumerate test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "browser_direct_enumerate.py"
    src.write_text(
        "print(list(enumerate(('a', 'b'))))\n"
    )

    build_env = _browser_wasm_build_env(root)
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
            "--out-dir",
            str(tmp_path),
        ],
        cwd=root,
        env=build_env,
        capture_output=True,
        text=True,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "output.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    assert output_wasm.exists()
    assert runtime_wasm.exists()

    run_env = os.environ.copy()
    run_env["MOLT_WASM_PREFER_LINKED"] = "0"
    run_env["MOLT_RUNTIME_WASM"] = str(runtime_wasm)
    run = subprocess.run(
        ["node", str(root / "wasm" / "run_wasm.js"), str(output_wasm)],
        cwd=root,
        env=run_env,
        capture_output=True,
        text=True,
        timeout=20,
    )
    assert run.returncode == 0, run.stderr
    assert [line.strip() for line in run.stdout.splitlines() if line.strip()] == [
        "[(0, 'a'), (1, 'b')]"
    ]


def test_browser_direct_run_wasm_dict_get_default(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser direct-mode dict.get test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser direct-mode dict.get test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "browser_direct_dict_get.py"
    src.write_text(
        "d = {'a': 3}\n"
        "print(d.get('a', 2))\n"
        "print(d.get('b', 2))\n"
    )

    build_env = _browser_wasm_build_env(root)
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
            "--out-dir",
            str(tmp_path),
        ],
        cwd=root,
        env=build_env,
        capture_output=True,
        text=True,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "output.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    assert output_wasm.exists()
    assert runtime_wasm.exists()

    run_env = os.environ.copy()
    run_env["MOLT_WASM_PREFER_LINKED"] = "0"
    run_env["MOLT_RUNTIME_WASM"] = str(runtime_wasm)
    run = subprocess.run(
        ["node", str(root / "wasm" / "run_wasm.js"), str(output_wasm)],
        cwd=root,
        env=run_env,
        capture_output=True,
        text=True,
        timeout=20,
    )
    assert run.returncode == 0, run.stderr
    assert [line.strip() for line in run.stdout.splitlines() if line.strip()] == [
        "3",
        "2",
    ]


def test_browser_direct_run_wasm_tuple_subclass_custom_repr(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser direct-mode tuple repr test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser direct-mode tuple repr test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "browser_direct_tuple_repr.py"
    src.write_text(
        "class T(tuple):\n"
        "    def __new__(cls, *args):\n"
        "        return tuple.__new__(cls, args)\n"
        "    def __repr__(self):\n"
        "        return f'T({self[0]}, {self[1]})'\n"
        "print(repr(T(1, 2)))\n"
    )

    build_env = _browser_wasm_build_env(root)
    build_env["MOLT_WASM_LINKED"] = "0"
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
            "--out-dir",
            str(tmp_path),
        ],
        cwd=root,
        env=build_env,
        capture_output=True,
        text=True,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "output.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    assert output_wasm.exists()
    assert runtime_wasm.exists()

    run_env = os.environ.copy()
    run_env["MOLT_WASM_PREFER_LINKED"] = "0"
    run_env["MOLT_RUNTIME_WASM"] = str(runtime_wasm)
    run = subprocess.run(
        ["node", str(root / "wasm" / "run_wasm.js"), str(output_wasm)],
        cwd=root,
        env=run_env,
        capture_output=True,
        text=True,
        timeout=20,
    )
    assert run.returncode == 0, run.stderr
    assert [line.strip() for line in run.stdout.splitlines() if line.strip()] == ["T(1, 2)"]


def test_browser_direct_run_wasm_try_except_clears_typeerror(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser direct-mode try/except test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser direct-mode try/except test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "browser_direct_try_except.py"
    src.write_text(
        "fn = None\n"
        "try:\n"
        "    fn()\n"
        "except Exception:\n"
        "    pass\n"
        "print('ok')\n"
    )

    build_env = os.environ.copy()
    build_env["PYTHONPATH"] = str(root / "src")
    build_env["MOLT_WASM_LINKED"] = "0"
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
            "--out-dir",
            str(tmp_path),
        ],
        cwd=root,
        env=build_env,
        capture_output=True,
        text=True,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "output.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    assert output_wasm.exists()
    assert runtime_wasm.exists()

    run_env = os.environ.copy()
    run_env["MOLT_WASM_PREFER_LINKED"] = "0"
    run_env["MOLT_RUNTIME_WASM"] = str(runtime_wasm)
    run = subprocess.run(
        ["node", str(root / "wasm" / "run_wasm.js"), str(output_wasm)],
        cwd=root,
        env=run_env,
        capture_output=True,
        text=True,
        timeout=20,
    )
    assert run.returncode == 0, run.stderr
    assert [line.strip() for line in run.stdout.splitlines() if line.strip()] == ["ok"]


def test_browser_direct_run_wasm_try_bare_except_clears_typeerror(
    tmp_path: Path,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser direct-mode bare except test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser direct-mode bare except test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "browser_direct_bare_except.py"
    src.write_text(
        "fn = None\n"
        "try:\n"
        "    fn()\n"
        "except:\n"
        "    pass\n"
        "print('ok')\n"
    )

    build_env = os.environ.copy()
    build_env["PYTHONPATH"] = str(root / "src")
    build_env["MOLT_WASM_LINKED"] = "0"
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
            "--out-dir",
            str(tmp_path),
        ],
        cwd=root,
        env=build_env,
        capture_output=True,
        text=True,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "output.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    assert output_wasm.exists()
    assert runtime_wasm.exists()

    run_env = os.environ.copy()
    run_env["MOLT_WASM_PREFER_LINKED"] = "0"
    run_env["MOLT_RUNTIME_WASM"] = str(runtime_wasm)
    run = subprocess.run(
        ["node", str(root / "wasm" / "run_wasm.js"), str(output_wasm)],
        cwd=root,
        env=run_env,
        capture_output=True,
        text=True,
        timeout=20,
    )
    assert run.returncode == 0, run.stderr
    assert [line.strip() for line in run.stdout.splitlines() if line.strip()] == ["ok"]


def test_wasm_browser_db_host_parity(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for wasm browser DB host test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for wasm browser DB host test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "db_browser_host.py"
    src.write_text(
        "import asyncio\n"
        "from molt import molt_db\n"
        "from molt.concurrency import CancellationToken\n"
        "\n"
        "async def main():\n"
        "    resp = await molt_db.db_query(b'fast')\n"
        "    print(resp.status)\n"
        "    print(resp.codec)\n"
        "    print(len(resp.payload or b''))\n"
        "    token = CancellationToken()\n"
        "    task = asyncio.create_task(molt_db.db_query(b'slow', token.token_id()))\n"
        "    await asyncio.sleep(0.01)\n"
        "    token.cancel()\n"
        "    resp2 = await task\n"
        "    print(resp2.status)\n"
        "\n"
        "asyncio.run(main())\n"
    )

    build_env = os.environ.copy()
    build_env["PYTHONPATH"] = str(root / "src")
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
            "--out-dir",
            str(tmp_path),
        ],
        cwd=root,
        env=build_env,
        capture_output=True,
        text=True,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "output.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    assert output_wasm.exists()
    assert runtime_wasm.exists()

    _DbHostHandler.output_wasm = output_wasm.read_bytes()
    _DbHostHandler.runtime_wasm = runtime_wasm.read_bytes()

    server = ThreadingHTTPServer(("127.0.0.1", 0), _DbHostHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        base_url = f"http://127.0.0.1:{server.server_address[1]}"
        browser_host_uri = (root / "wasm" / "browser_host.js").as_uri()
        script = tmp_path / "run_browser_db_host.mjs"
        script.write_text(
            f"""
import {{ loadMoltWasm }} from '{browser_host_uri}';

const baseUrl = {base_url!r};
const wasmUrl = `${{baseUrl}}/output.wasm`;
const runtimeUrl = `${{baseUrl}}/molt_runtime.wasm`;
const dbEndpoint = `${{baseUrl}}/db`;

const host = await loadMoltWasm({{
  wasmUrl,
  runtimeUrl,
  preferLinked: false,
  dbEndpoint,
}});
host.run();
""".lstrip()
        )
        run = subprocess.run(
            ["node", str(script)],
            cwd=root,
            capture_output=True,
            text=True,
        )
        assert run.returncode == 0, run.stderr
        lines = [line.strip() for line in run.stdout.splitlines() if line.strip()]
        assert lines == ["ok", "arrow_ipc", str(len(ARROW_BYTES)), "cancelled"]
    finally:
        server.shutdown()


def test_browser_host_default_urls_prefer_canonical_dist_and_explicit_sibling(
    tmp_path: Path,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for wasm browser host path resolution test")

    root = Path(__file__).resolve().parents[1]
    browser_host_uri = (root / "wasm" / "browser_host.js").as_uri()
    script = tmp_path / "resolve_browser_urls.mjs"
    script.write_text(
        f"""
import {{ resolveMoltWasmUrls }} from '{browser_host_uri}';

const canonical = resolveMoltWasmUrls({{}}, {repr((root / 'wasm' / 'browser_host.js').as_uri())});
const explicit = resolveMoltWasmUrls({{
  wasmUrl: 'https://example.com/build/output.wasm',
}}, {repr((root / 'wasm' / 'browser_host.js').as_uri())});

console.log(JSON.stringify({{ canonical, explicit }}));
""".lstrip()
    )

    run = subprocess.run(
        ["node", str(script)],
        cwd=root,
        capture_output=True,
        text=True,
    )
    assert run.returncode == 0, run.stderr
    payload = json.loads(run.stdout)
    canonical = payload["canonical"]
    explicit = payload["explicit"]
    assert canonical["wasmUrl"].endswith("/dist/output.wasm")
    assert canonical["linkedUrl"].endswith("/dist/output_linked.wasm")
    assert explicit["wasmUrl"] == "https://example.com/build/output.wasm"
    assert explicit["linkedUrl"] == "https://example.com/build/output_linked.wasm"
