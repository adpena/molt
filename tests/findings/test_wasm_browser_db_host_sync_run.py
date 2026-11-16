"""HF-699: a browser-host program answers its DB queries inside ``run()``.

Oracle: the DB endpoint this test serves answers ``fast`` with an Arrow IPC
payload and lets the cancelled ``slow`` query end as ``cancelled``, so the
program must print ``ok``, ``arrow_ipc``, the payload length and
``cancelled``. ``run()`` executes the program in one synchronous JavaScript
turn, where a fetch promise cannot settle; the host answers each query at
once with ``MOLT_BROWSER_DB_SYNC_RUN`` instead of letting the program wait
until the guard kills it.
"""

from __future__ import annotations

import shutil
import sys
import threading
from http.server import ThreadingHTTPServer
from pathlib import Path

import pytest

from tests.findings import finding
from tests.test_wasm_browser_db_host import (
    ARROW_BYTES,
    _browser_wasm_build_env,
    _DbHostHandler,
)
from tests.wasm_linked_runner import _run_wasm_test_process

PROGRAM = """\
import asyncio
from molt import molt_db
from moltlib.concurrency import CancellationToken

async def main():
    resp = await molt_db.db_query(b'fast')
    print(resp.status)
    print(resp.codec)
    print(len(resp.payload or b''))
    token = CancellationToken()
    task = asyncio.create_task(molt_db.db_query(b'slow', token.token_id()))
    await asyncio.sleep(0.01)
    token.cancel()
    resp2 = await task
    print(resp2.status)

asyncio.run(main())
"""


@finding("HF-699")
def test_wasm_browser_db_host_parity(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for wasm browser DB host test")
    root = Path(__file__).resolve().parents[2]
    src = tmp_path / "db_browser_host.py"
    src.write_text(PROGRAM, encoding="utf-8")
    build = _run_wasm_test_process(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            str(src),
            "--capabilities",
            "db.read,time",
            "--build-profile",
            "dev",
            "--profile",
            "browser",
            "--target",
            "wasm",
            "--out-dir",
            str(tmp_path),
        ],
        role="tool",
        cwd=root,
        env=_browser_wasm_build_env(root),
        capture_output=True,
        text=True,
    )
    if build.returncode != 0:
        raise RuntimeError(f"browser DB program build failed:\n{build.stderr[-2000:]}")
    _DbHostHandler.output_wasm = (tmp_path / "app.wasm").read_bytes()
    _DbHostHandler.runtime_wasm = (tmp_path / "molt_runtime.wasm").read_bytes()
    _DbHostHandler.manifest = (tmp_path / "manifest.json").read_bytes()

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
const host = await loadMoltWasm({{
  manifestUrl: `${{baseUrl}}/manifest.json`,
  preferLinked: false,
  dbEndpoint: `${{baseUrl}}/db`,
}});
try {{ host.run(); }} finally {{ host.dispose(); }}
""".lstrip(),
            encoding="utf-8",
        )
        run = _run_wasm_test_process(
            ["node", str(script)],
            role="guest-host",
            cwd=root,
            capture_output=True,
            text=True,
            timeout=300,
        )
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
    lines = [line.strip() for line in run.stdout.splitlines() if line.strip()]
    assert (run.returncode, lines) == (
        0,
        ["ok", "arrow_ipc", str(len(ARROW_BYTES)), "cancelled"],
    ), run.stderr[-2000:]
