import base64
import json
import os
import shutil
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import ClassVar

import pytest

from tests.wasm_linked_runner import _run_wasm_test_process, wasm_test_build_env


ARROW_BYTES = b"ARROW1"


class _DbHostHandler(BaseHTTPRequestHandler):
    output_wasm: ClassVar[bytes] = b""
    runtime_wasm: ClassVar[bytes] = b""
    manifest: ClassVar[bytes] = b""

    def log_message(self, fmt: str, *args: object) -> None:
        return None

    def _send_bytes(self, payload: bytes, content_type: str) -> None:
        self.send_response(200)
        self.send_header("content-type", content_type)
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def do_GET(self) -> None:  # noqa: N802
        if self.path == "/app.wasm":
            self._send_bytes(self.output_wasm, "application/wasm")
            return
        if self.path == "/molt_runtime.wasm":
            self._send_bytes(self.runtime_wasm, "application/wasm")
            return
        if self.path == "/manifest.json":
            self._send_bytes(self.manifest, "application/json")
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
    return wasm_test_build_env(root, linked=False)


def test_browser_host_manifest_module_integrity_is_fail_closed(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser module-integrity proof")
    root = Path(__file__).resolve().parents[1]
    browser_host_uri = (root / "wasm" / "browser_host.js").as_uri()
    script = tmp_path / "verify_browser_module_integrity.mjs"
    script.write_text(
        f"""
import {{ webcrypto }} from 'node:crypto';
import {{
  decodeOwnedExportResult,
  loadMoltWasm,
  makeDeferredLinkedSelfImportBridge,
  tryDecodeListIntBits,
  verifyManifestModuleBytes,
}} from '{browser_host_uri}';

const original = new TextEncoder().encode('canonical module bytes');
const bytes = original.buffer.slice(
  original.byteOffset,
  original.byteOffset + original.byteLength,
);
const digest = await webcrypto.subtle.digest('SHA-256', bytes);
const sha256 = Array.from(new Uint8Array(digest), (byte) =>
  byte.toString(16).padStart(2, '0')
).join('');
const descriptor = {{ path: 'module.wasm', size: bytes.byteLength, sha256 }};
await verifyManifestModuleBytes(
  bytes,
  descriptor,
  'linked wasm',
  'https://example.invalid/module.wasm',
  webcrypto,
);

const corrupted = bytes.slice(0);
new Uint8Array(corrupted)[0] ^= 0xff;
const rejected = async (operation, pattern) => {{
  try {{
    await operation();
  }} catch (error) {{
    if (!pattern.test(String(error?.message || error))) throw error;
    return;
  }}
  throw new Error(`integrity failure did not reject: ${{pattern}}`);
}};
const fakeMemory = new WebAssembly.Memory({{ initial: 1 }});
const releasedItems = [];
const nonIntRuntime = {{
  exports: {{
    molt_len: () => 0x7ff9000000000001n,
    molt_index: () => 0x1234n,
    molt_dec_ref_obj: (bits) => releasedItems.push(bits),
  }},
}};
if (tryDecodeListIntBits(nonIntRuntime, fakeMemory, 0x4444n) !== null) {{
  throw new Error('non-int list item unexpectedly decoded');
}}
if (releasedItems.length !== 1 || releasedItems[0] !== 0x1234n) {{
  throw new Error('owned non-int index result was not released exactly once');
}}
let lenFailureObserved = false;
try {{
  tryDecodeListIntBits({{
    exports: {{
      molt_len: () => {{ throw new Error('len exploded'); }},
      molt_index: () => 0n,
    }},
  }}, fakeMemory, 0x4444n);
}} catch (error) {{
  if (!/len exploded/.test(String(error?.message || error))) throw error;
  lenFailureObserved = true;
}}
if (!lenFailureObserved) throw new Error('molt_len failure was suppressed');
const releasedResults = [];
let decodeFailureObserved = false;
try {{
  decodeOwnedExportResult({{
    exports: {{
      molt_type_tag_of_bits: () => 99,
      molt_object_repr: () => {{ throw new Error('repr exploded'); }},
      molt_dec_ref_obj: (bits) => releasedResults.push(bits),
    }},
  }}, fakeMemory, 0x5678n, 'adversarial');
}} catch (error) {{
  if (!/repr exploded/.test(String(error?.message || error))) throw error;
  decodeFailureObserved = true;
}}
if (!decodeFailureObserved) throw new Error('result decode failure was suppressed');
if (releasedResults.length !== 1 || releasedResults[0] !== 0x5678n) {{
  throw new Error('owned export result was not released exactly once');
}}
let linkedInstance = null;
const isolateImportBridge = makeDeferredLinkedSelfImportBridge(
  () => linkedInstance,
  'molt_isolate_import',
  'synthetic linked',
);
await rejected(
  async () => isolateImportBridge(41n),
  /molt_isolate_import used before synthetic linked instantiation/,
);
linkedInstance = {{
  exports: {{ molt_isolate_import: (value) => value + 1n }},
}};
if (isolateImportBridge(41n) !== 42n) {{
  throw new Error('synthetic linked isolate self-import did not delegate');
}}
await rejected(
  () => verifyManifestModuleBytes(
    corrupted,
    descriptor,
    'linked wasm',
    'https://example.invalid/module.wasm',
    webcrypto,
  ),
  /linked wasm SHA-256 mismatch/,
);
await rejected(
  () => verifyManifestModuleBytes(
    bytes,
    {{ ...descriptor, size: bytes.byteLength + 1 }},
    'app wasm',
    'https://example.invalid/module.wasm',
    webcrypto,
  ),
  /app wasm size mismatch/,
);
await rejected(
  () => verifyManifestModuleBytes(
    bytes,
    descriptor,
    'runtime wasm',
    'https://example.invalid/module.wasm',
    {{}},
  ),
  /runtime wasm integrity verification requires WebCrypto SHA-256/,
);
await rejected(
  () => verifyManifestModuleBytes(
    bytes,
    {{ ...descriptor, path: 'substitute.wasm' }},
    'linked wasm',
    'https://example.invalid/module.wasm',
    webcrypto,
  ),
  /linked wasm path mismatch/,
);

const linkedManifest = {{
  mode: 'linked',
  abi: {{ runtime_imports: {{ names: [] }} }},
  modules: {{ linked: descriptor }},
}};
await rejected(
  () => loadMoltWasm({{
    manifest: linkedManifest,
    manifestUrl: 'https://example.invalid/manifest.json',
    preferLinked: false,
  }}),
  /linked browser manifest cannot enter split-runtime mode/,
);

const fetched = [];
globalThis.fetch = async (url) => {{
  fetched.push(String(url));
  return {{ ok: false }};
}};
await rejected(
  () => loadMoltWasm({{
    manifest: linkedManifest,
    manifestUrl: 'https://example.invalid/manifest.json',
  }}),
  /linked browser manifest module is unavailable/,
);
const splitManifest = {{
  mode: 'split-runtime',
  abi: {{ runtime_imports: {{ names: [] }} }},
  modules: {{ app: descriptor, runtime: descriptor }},
}};
await rejected(
  () => loadMoltWasm({{
    manifest: splitManifest,
    manifestUrl: 'https://example.invalid/manifest.json',
    preferLinked: true,
  }}),
  /Failed to load wasm/,
);
if (fetched.some((url) => url.endsWith('/forbidden-linked.wasm'))) {{
  throw new Error('split-runtime manifest probed the linked fallback lane');
}}
await rejected(
  () => loadMoltWasm({{
    manifest: {{ ...splitManifest, mode: 'unknown' }},
    manifestUrl: 'https://example.invalid/manifest.json',
  }}),
  /browser host manifest has unsupported mode/,
);
console.log('module-integrity-ok');
""".lstrip(),
        encoding="utf-8",
    )
    run = _run_wasm_test_process(
        ["node", str(script)],
        cwd=root,
        capture_output=True,
        text=True,
    )
    assert run.returncode == 0, run.stderr
    assert run.stdout.strip() == "module-integrity-ok"


def test_browser_host_direct_mode_bridges_isolate_import(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser host direct-mode isolate test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser host direct-mode isolate test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "browser_host_direct.py"
    src.write_text(
        "import asyncio\n\nasync def main():\n    print('ok')\n\nasyncio.run(main())\n"
    )

    build_env = _browser_wasm_build_env(root)
    build = _run_wasm_test_process(
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

    output_wasm = tmp_path / "app.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    manifest_path = tmp_path / "manifest.json"
    assert output_wasm.exists()
    assert runtime_wasm.exists()
    assert manifest_path.exists()

    class _DirectHostHandler(BaseHTTPRequestHandler):
        def log_message(self, fmt: str, *args: object) -> None:
            return None

        def do_GET(self) -> None:  # noqa: N802
            if self.path == "/app.wasm":
                payload = output_wasm.read_bytes()
            elif self.path == "/molt_runtime.wasm":
                payload = runtime_wasm.read_bytes()
            elif self.path == "/manifest.json":
                payload = manifest_path.read_bytes()
            else:
                self.send_response(404)
                self.end_headers()
                return
            self.send_response(200)
            content_type = (
                "application/json"
                if self.path == "/manifest.json"
                else "application/wasm"
            )
            self.send_header("content-type", content_type)
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
  manifestUrl: `${{baseUrl}}/manifest.json`,
  preferLinked: false,
}});
host.run();
""".lstrip()
        )
        run = _run_wasm_test_process(
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
        "import abc\nprint('after')\n",
        encoding="utf-8",
    )

    build_env = _browser_wasm_build_env(root)
    build = _run_wasm_test_process(
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

    output_wasm = tmp_path / "app.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    manifest_path = tmp_path / "manifest.json"
    assert output_wasm.exists()
    assert runtime_wasm.exists()
    assert manifest_path.exists()

    class _DirectHostHandler(BaseHTTPRequestHandler):
        def log_message(self, fmt: str, *args: object) -> None:
            return None

        def do_GET(self) -> None:  # noqa: N802
            if self.path == "/app.wasm":
                payload = output_wasm.read_bytes()
            elif self.path == "/molt_runtime.wasm":
                payload = runtime_wasm.read_bytes()
            elif self.path == "/manifest.json":
                payload = manifest_path.read_bytes()
            else:
                self.send_response(404)
                self.end_headers()
                return
            self.send_response(200)
            content_type = (
                "application/json"
                if self.path == "/manifest.json"
                else "application/wasm"
            )
            self.send_header("content-type", content_type)
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
  manifestUrl: `${{baseUrl}}/manifest.json`,
  preferLinked: false,
}});
host.run();
""".lstrip(),
            encoding="utf-8",
        )
        run = _run_wasm_test_process(
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
    build = _run_wasm_test_process(
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

    output_wasm = tmp_path / "app.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    manifest_path = tmp_path / "manifest.json"
    assert output_wasm.exists()
    assert runtime_wasm.exists()
    assert manifest_path.exists()

    class _DirectHostHandler(BaseHTTPRequestHandler):
        def log_message(self, fmt: str, *args: object) -> None:
            return None

        def do_GET(self) -> None:  # noqa: N802
            if self.path == "/app.wasm":
                payload = output_wasm.read_bytes()
            elif self.path == "/molt_runtime.wasm":
                payload = runtime_wasm.read_bytes()
            elif self.path == "/manifest.json":
                payload = manifest_path.read_bytes()
            else:
                self.send_response(404)
                self.end_headers()
                return
            self.send_response(200)
            content_type = (
                "application/json"
                if self.path == "/manifest.json"
                else "application/wasm"
            )
            self.send_header("content-type", content_type)
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
  manifestUrl: `${{baseUrl}}/manifest.json`,
  preferLinked: false,
}});
host.run();
""".lstrip()
        )
        run = _run_wasm_test_process(
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
    build = _run_wasm_test_process(
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
            "--linked",
            "--require-linked",
            "--out-dir",
            str(tmp_path),
        ],
        cwd=root,
        env=build_env,
        capture_output=True,
        text=True,
    )
    assert build.returncode == 0, build.stderr

    linked_wasm = tmp_path / "output_linked.wasm"
    manifest_path = tmp_path / "manifest.json"
    assert linked_wasm.exists()
    assert manifest_path.exists()
    corrupt_linked_bytes = bytearray(linked_wasm.read_bytes())
    corrupt_linked_bytes[-1] ^= 0xFF

    class _DirectHostHandler(BaseHTTPRequestHandler):
        def log_message(self, fmt: str, *args: object) -> None:
            return None

        def do_GET(self) -> None:  # noqa: N802
            if self.path == "/output_linked.wasm":
                payload = linked_wasm.read_bytes()
            elif self.path == "/output_linked.wasm?corrupt=1":
                payload = bytes(corrupt_linked_bytes)
            elif self.path == "/manifest.json":
                payload = manifest_path.read_bytes()
            elif self.path == "/manifest.json?corrupt=linked":
                manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
                manifest["modules"]["linked"]["path"] = "output_linked.wasm?corrupt=1"
                payload = json.dumps(manifest).encode("utf-8")
            else:
                self.send_response(404)
                self.end_headers()
                return
            self.send_response(200)
            content_type = (
                "application/json"
                if self.path.startswith("/manifest.json")
                else "application/wasm"
            )
            self.send_header("content-type", content_type)
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
let corruptLinkedRejected = false;
try {{
  await loadMoltWasm({{
    manifestUrl: `${{baseUrl}}/manifest.json?corrupt=linked`,
    preferLinked: true,
  }});
}} catch (error) {{
  if (!/linked wasm SHA-256 mismatch/.test(String(error?.message || error))) throw error;
  corruptLinkedRejected = true;
}}
if (!corruptLinkedRejected) throw new Error('corrupt linked wasm was admitted');
const host = await loadMoltWasm({{
  manifestUrl: `${{baseUrl}}/manifest.json`,
  preferLinked: true,
}});
if (!host.linked) throw new Error('linked execution-boundary proof loaded split wasm');
const runtime = host.__debugState.runtimeInstance;
const manifest = await (await fetch(`${{baseUrl}}/manifest.json`)).json();
const enterName = manifest?.abi?.runtime_imports?.export_names?.runtime_execution_enter;
const leaveName = manifest?.abi?.runtime_imports?.export_names?.runtime_execution_leave;
if (typeof runtime.exports[enterName] !== 'function') {{
  throw new Error(`linked runtime missing mapped execution enter export: ${{enterName}}`);
}}
if (typeof runtime.exports[leaveName] !== 'function') {{
  throw new Error(`linked runtime missing mapped execution leave export: ${{leaveName}}`);
}}
const outerToken = runtime.exports[enterName]();
const result = await host.invokeExport('browser_export_probe__echo', [
  896,
  [257, 258],
  new Uint8Array([1, 2, 3, 4]),
  'falcon',
]);
let missingRejected = false;
try {{
  await host.invokeExport('browser_export_probe__missing', []);
}} catch (_err) {{
  missingRejected = true;
}}
if (!missingRejected) throw new Error('missing export did not reject');
runtime.exports[leaveName](outerToken);
const postTrapToken = runtime.exports[enterName]();
runtime.exports[leaveName](postTrapToken);
console.log(JSON.stringify(result));
""".lstrip(),
            encoding="utf-8",
        )
        run = _run_wasm_test_process(
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
        assert payload["resultRepr"] == "[257, 258]"
    finally:
        server.shutdown()


def test_browser_host_direct_mode_can_invoke_export_with_host_args_split_runtime(
    tmp_path: Path,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser host direct-mode export test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser host direct-mode export test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "browser_export_probe_split.py"
    src.write_text(
        "def echo(width: int, prompt_ids: list[int], rgb: bytes, label: str):\n"
        "    print(width)\n"
        "    print(len(rgb))\n"
        "    print(label)\n"
        "    return prompt_ids\n",
        encoding="utf-8",
    )

    build_env = _browser_wasm_build_env(root)
    build = _run_wasm_test_process(
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
        env=build_env,
        capture_output=True,
        text=True,
    )
    assert build.returncode == 0, build.stderr

    app_wasm = tmp_path / "app.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    manifest_path = tmp_path / "manifest.json"
    assert app_wasm.exists()
    assert runtime_wasm.exists()
    assert manifest_path.exists()
    corrupt_app_bytes = bytearray(app_wasm.read_bytes())
    corrupt_app_bytes[-1] ^= 0xFF
    corrupt_runtime_bytes = bytearray(runtime_wasm.read_bytes())
    corrupt_runtime_bytes[-1] ^= 0xFF

    class _DirectHostHandler(BaseHTTPRequestHandler):
        def log_message(self, fmt: str, *args: object) -> None:
            return None

        def do_GET(self) -> None:  # noqa: N802
            if self.path == "/app.wasm":
                payload = app_wasm.read_bytes()
            elif self.path == "/app.wasm?corrupt=1":
                payload = bytes(corrupt_app_bytes)
            elif self.path == "/molt_runtime.wasm":
                payload = runtime_wasm.read_bytes()
            elif self.path == "/molt_runtime.wasm?corrupt=1":
                payload = bytes(corrupt_runtime_bytes)
            elif self.path == "/manifest.json":
                payload = manifest_path.read_bytes()
            elif self.path in {
                "/manifest.json?corrupt=app",
                "/manifest.json?corrupt=runtime",
            }:
                manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
                module = self.path.rsplit("=", 1)[1]
                manifest["modules"][module]["path"] += "?corrupt=1"
                payload = json.dumps(manifest).encode("utf-8")
            else:
                self.send_response(404)
                self.end_headers()
                return
            self.send_response(200)
            content_type = (
                "application/json"
                if self.path.startswith("/manifest.json")
                else "application/wasm"
            )
            self.send_header("content-type", content_type)
            self.send_header("content-length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

    server = ThreadingHTTPServer(("127.0.0.1", 0), _DirectHostHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        base_url = f"http://127.0.0.1:{server.server_address[1]}"
        browser_host_uri = (root / "wasm" / "browser_host.js").as_uri()
        script = tmp_path / "run_browser_export_call_split.mjs"
        script.write_text(
            f"""
import {{ loadMoltWasm }} from '{browser_host_uri}';

const baseUrl = {base_url!r};
const rejected = async (operation, pattern) => {{
  try {{
    await operation();
  }} catch (error) {{
    if (!pattern.test(String(error?.message || error))) throw error;
    return;
  }}
  throw new Error(`corrupt split module was admitted: ${{pattern}}`);
}};
await rejected(
  () => loadMoltWasm({{
    manifestUrl: `${{baseUrl}}/manifest.json?corrupt=app`,
    preferLinked: false,
  }}),
  /app wasm SHA-256 mismatch/,
);
await rejected(
  () => loadMoltWasm({{
    manifestUrl: `${{baseUrl}}/manifest.json?corrupt=runtime`,
    preferLinked: false,
  }}),
  /runtime wasm SHA-256 mismatch/,
);
const host = await loadMoltWasm({{
  manifestUrl: `${{baseUrl}}/manifest.json`,
  preferLinked: false,
}});
if (host.linked) throw new Error('split execution-boundary proof loaded linked wasm');
const runtime = host.__debugState.runtimeInstance;
const manifest = await (await fetch(`${{baseUrl}}/manifest.json`)).json();
const enterName = manifest?.abi?.runtime_imports?.export_names?.runtime_execution_enter;
const leaveName = manifest?.abi?.runtime_imports?.export_names?.runtime_execution_leave;
if (typeof runtime.exports[enterName] !== 'function') {{
  throw new Error(`split runtime missing mapped execution enter export: ${{enterName}}`);
}}
if (typeof runtime.exports[leaveName] !== 'function') {{
  throw new Error(`split runtime missing mapped execution leave export: ${{leaveName}}`);
}}
const outerToken = runtime.exports[enterName]();
host.run();
const result = await host.invokeExport('browser_export_probe_split__echo', [
  896,
  [257, 258],
  new Uint8Array([1, 2, 3, 4]),
  'falcon',
]);
let missingRejected = false;
try {{
  await host.invokeExport('browser_export_probe_split__missing', []);
}} catch (_err) {{
  missingRejected = true;
}}
if (!missingRejected) throw new Error('missing export did not reject');
runtime.exports[leaveName](outerToken);
const postTrapToken = runtime.exports[enterName]();
runtime.exports[leaveName](postTrapToken);
console.log(JSON.stringify(result));
""".lstrip(),
            encoding="utf-8",
        )
        run = _run_wasm_test_process(
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
        assert payload["resultRepr"] == "[257, 258]"
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
    build = _run_wasm_test_process(
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

    output_wasm = tmp_path / "app.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    manifest_path = tmp_path / "manifest.json"
    assert output_wasm.exists()
    assert runtime_wasm.exists()
    assert manifest_path.exists()

    class _DirectHostHandler(BaseHTTPRequestHandler):
        def log_message(self, fmt: str, *args: object) -> None:
            return None

        def do_GET(self) -> None:  # noqa: N802
            if self.path == "/app.wasm":
                payload = output_wasm.read_bytes()
            elif self.path == "/molt_runtime.wasm":
                payload = runtime_wasm.read_bytes()
            elif self.path == "/manifest.json":
                payload = manifest_path.read_bytes()
            else:
                self.send_response(404)
                self.end_headers()
                return
            self.send_response(200)
            content_type = (
                "application/json"
                if self.path == "/manifest.json"
                else "application/wasm"
            )
            self.send_header("content-type", content_type)
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
  manifestUrl: `${{baseUrl}}/manifest.json`,
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
        run = _run_wasm_test_process(
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
        assert payload["listResult"]["resultRepr"] == "[7, 8]"
    finally:
        server.shutdown()


def test_browser_host_direct_mode_import_asyncio_iov_max(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser host direct-mode isolate test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser host direct-mode isolate test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "browser_host_asyncio_iov.py"
    src.write_text("import asyncio\nprint(asyncio.selector_events.SC_IOV_MAX)\n")

    build_env = _browser_wasm_build_env(root)
    build = _run_wasm_test_process(
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

    output_wasm = tmp_path / "app.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    manifest_path = tmp_path / "manifest.json"
    assert output_wasm.exists()
    assert runtime_wasm.exists()
    assert manifest_path.exists()

    class _DirectHostHandler(BaseHTTPRequestHandler):
        def log_message(self, fmt: str, *args: object) -> None:
            return None

        def do_GET(self) -> None:  # noqa: N802
            if self.path == "/app.wasm":
                payload = output_wasm.read_bytes()
            elif self.path == "/molt_runtime.wasm":
                payload = runtime_wasm.read_bytes()
            elif self.path == "/manifest.json":
                payload = manifest_path.read_bytes()
            else:
                self.send_response(404)
                self.end_headers()
                return
            self.send_response(200)
            content_type = (
                "application/json"
                if self.path == "/manifest.json"
                else "application/wasm"
            )
            self.send_header("content-type", content_type)
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
  manifestUrl: `${{baseUrl}}/manifest.json`,
  preferLinked: false,
}});
host.run();
""".lstrip()
        )
        run = _run_wasm_test_process(
            ["node", str(script)],
            cwd=root,
            capture_output=True,
            text=True,
        )
        assert run.returncode == 0, run.stderr
        assert [line.strip() for line in run.stdout.splitlines() if line.strip()] == [
            "1024"
        ]
    finally:
        server.shutdown()


def test_browser_direct_run_wasm_import_os_name(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser direct-mode os.name test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser direct-mode os.name test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "browser_direct_os_name.py"
    src.write_text("import os\nprint(os.name)\n")

    build_env = _browser_wasm_build_env(root)
    build = _run_wasm_test_process(
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

    output_wasm = tmp_path / "app.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    manifest_path = tmp_path / "manifest.json"
    assert output_wasm.exists()
    assert runtime_wasm.exists()
    assert manifest_path.exists()

    run_env = os.environ.copy()
    run = _run_wasm_test_process(
        ["node", str(root / "wasm" / "run_wasm.js"), str(manifest_path)],
        cwd=root,
        env=run_env,
        capture_output=True,
        text=True,
    )
    assert run.returncode == 0, run.stderr
    assert [line.strip() for line in run.stdout.splitlines() if line.strip()] == [
        "posix"
    ]


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
    build = _run_wasm_test_process(
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

    output_wasm = tmp_path / "app.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    manifest_path = tmp_path / "manifest.json"
    assert output_wasm.exists()
    assert runtime_wasm.exists()
    assert manifest_path.exists()

    run_env = os.environ.copy()
    run_env["MOLT_CAPABILITY_TIER"] = "full"
    run = _run_wasm_test_process(
        ["node", str(root / "wasm" / "run_wasm.js"), str(manifest_path)],
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
    build = _run_wasm_test_process(
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

    output_wasm = tmp_path / "app.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    manifest_path = tmp_path / "manifest.json"
    assert output_wasm.exists()
    assert runtime_wasm.exists()
    assert manifest_path.exists()

    run_env = os.environ.copy()
    run = _run_wasm_test_process(
        ["node", str(root / "wasm" / "run_wasm.js"), str(manifest_path)],
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
    build = _run_wasm_test_process(
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

    output_wasm = tmp_path / "app.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    manifest_path = tmp_path / "manifest.json"
    assert output_wasm.exists()
    assert runtime_wasm.exists()
    assert manifest_path.exists()

    run_env = os.environ.copy()
    run = _run_wasm_test_process(
        ["node", str(root / "wasm" / "run_wasm.js"), str(manifest_path)],
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
    src.write_text("print(list(enumerate(('a', 'b'))))\n")

    build_env = _browser_wasm_build_env(root)
    build = _run_wasm_test_process(
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

    output_wasm = tmp_path / "app.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    manifest_path = tmp_path / "manifest.json"
    assert output_wasm.exists()
    assert runtime_wasm.exists()
    assert manifest_path.exists()

    run_env = os.environ.copy()
    run = _run_wasm_test_process(
        ["node", str(root / "wasm" / "run_wasm.js"), str(manifest_path)],
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
    src.write_text("d = {'a': 3}\nprint(d.get('a', 2))\nprint(d.get('b', 2))\n")

    build_env = _browser_wasm_build_env(root)
    build = _run_wasm_test_process(
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

    output_wasm = tmp_path / "app.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    manifest_path = tmp_path / "manifest.json"
    assert output_wasm.exists()
    assert runtime_wasm.exists()
    assert manifest_path.exists()

    run_env = os.environ.copy()
    run = _run_wasm_test_process(
        ["node", str(root / "wasm" / "run_wasm.js"), str(manifest_path)],
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
    build = _run_wasm_test_process(
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

    output_wasm = tmp_path / "app.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    manifest_path = tmp_path / "manifest.json"
    assert output_wasm.exists()
    assert runtime_wasm.exists()
    assert manifest_path.exists()

    run_env = os.environ.copy()
    run = _run_wasm_test_process(
        ["node", str(root / "wasm" / "run_wasm.js"), str(manifest_path)],
        cwd=root,
        env=run_env,
        capture_output=True,
        text=True,
        timeout=20,
    )
    assert run.returncode == 0, run.stderr
    assert [line.strip() for line in run.stdout.splitlines() if line.strip()] == [
        "T(1, 2)"
    ]


def test_browser_direct_run_wasm_try_except_clears_typeerror(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser direct-mode try/except test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser direct-mode try/except test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "browser_direct_try_except.py"
    src.write_text(
        "fn = None\ntry:\n    fn()\nexcept Exception:\n    pass\nprint('ok')\n"
    )

    build_env = _browser_wasm_build_env(root)
    build = _run_wasm_test_process(
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

    output_wasm = tmp_path / "app.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    manifest_path = tmp_path / "manifest.json"
    assert output_wasm.exists()
    assert runtime_wasm.exists()
    assert manifest_path.exists()

    run_env = os.environ.copy()
    run = _run_wasm_test_process(
        ["node", str(root / "wasm" / "run_wasm.js"), str(manifest_path)],
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
    src.write_text("fn = None\ntry:\n    fn()\nexcept:\n    pass\nprint('ok')\n")

    build_env = _browser_wasm_build_env(root)
    build = _run_wasm_test_process(
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

    output_wasm = tmp_path / "app.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    manifest_path = tmp_path / "manifest.json"
    assert output_wasm.exists()
    assert runtime_wasm.exists()
    assert manifest_path.exists()

    run_env = os.environ.copy()
    run = _run_wasm_test_process(
        ["node", str(root / "wasm" / "run_wasm.js"), str(manifest_path)],
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

    build_env = _browser_wasm_build_env(root)
    build = _run_wasm_test_process(
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

    output_wasm = tmp_path / "app.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    manifest_path = tmp_path / "manifest.json"
    assert output_wasm.exists()
    assert runtime_wasm.exists()
    assert manifest_path.exists()

    _DbHostHandler.output_wasm = output_wasm.read_bytes()
    _DbHostHandler.runtime_wasm = runtime_wasm.read_bytes()
    _DbHostHandler.manifest = manifest_path.read_bytes()

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
const dbEndpoint = `${{baseUrl}}/db`;

const host = await loadMoltWasm({{
  manifestUrl: `${{baseUrl}}/manifest.json`,
  preferLinked: false,
  dbEndpoint,
}});
host.run();
""".lstrip()
        )
        run = _run_wasm_test_process(
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


def test_browser_host_module_urls_are_manifest_relative_without_sibling_guessing(
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

const split = resolveMoltWasmUrls({{
  mode: 'split-runtime',
  modules: {{
    app: {{ path: 'modules/app-prod.wasm' }},
    runtime: {{ path: '../runtime/runtime-prod.wasm' }},
  }},
}}, 'https://example.com/releases/v9/manifest.json');
const linked = resolveMoltWasmUrls({{
  mode: 'linked',
  modules: {{ linked: {{ path: 'artifacts/program.wasm' }} }},
}}, 'https://example.com/releases/v9/manifest.json');

console.log(JSON.stringify({{ split, linked }}));
""".lstrip()
    )

    run = _run_wasm_test_process(
        ["node", str(script)],
        cwd=root,
        capture_output=True,
        text=True,
    )
    assert run.returncode == 0, run.stderr
    payload = json.loads(run.stdout)
    assert payload["split"] == {
        "wasmUrl": "https://example.com/releases/v9/modules/app-prod.wasm",
        "runtimeUrl": "https://example.com/releases/runtime/runtime-prod.wasm",
        "linkedUrl": None,
    }
    assert payload["linked"] == {
        "wasmUrl": None,
        "runtimeUrl": None,
        "linkedUrl": "https://example.com/releases/v9/artifacts/program.wasm",
    }
