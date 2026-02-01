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
