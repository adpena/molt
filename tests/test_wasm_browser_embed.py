from __future__ import annotations

import json
import os
import shutil
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest

from molt.dx import development_artifact_env
from tests.wasm_linked_runner import _run_wasm_test_process


def _browser_wasm_build_env(root: Path) -> dict[str, str]:
    env = development_artifact_env(
        root,
        os.environ,
        session_prefix="test-wasm-browser-embed",
        session_id=os.environ.get("MOLT_SESSION_ID") or "test-wasm-browser-embed",
        create_dirs=True,
    )
    env.setdefault("CARGO_BUILD_JOBS", "1")
    env.setdefault("MOLT_WASM_DISABLE_SCCACHE", "1")
    env.setdefault("MOLT_BUILD_LOCK_TIMEOUT", "45")
    env.setdefault("MOLT_CARGO_TIMEOUT", "900")
    env.setdefault("MOLT_BACKEND_DAEMON", "0")
    return env


class _StaticDirHandler(BaseHTTPRequestHandler):
    root: Path

    def log_message(self, fmt: str, *args: object) -> None:
        return None

    def do_GET(self) -> None:  # noqa: N802
        rel = self.path.lstrip("/") or "index.html"
        path = self.root / rel
        if not path.is_file():
            self.send_response(404)
            self.end_headers()
            return
        payload = path.read_bytes()
        if path.suffix == ".wasm":
            content_type = "application/wasm"
        elif path.suffix == ".js":
            content_type = "text/javascript"
        elif path.suffix == ".json":
            content_type = "application/json"
        else:
            content_type = "application/octet-stream"
        self.send_response(200)
        self.send_header("content-type", content_type)
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)


@pytest.mark.slow
def test_browser_embed_forward_roundtrips_float32_typed_arrays(
    tmp_path: Path,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser embed typed-array test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser embed typed-array test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "browser_embed_forward.py"
    src.write_text(
        "from array import array\n"
        "\n"
        "def forward(raw: bytes):\n"
        "    values = array('f')\n"
        "    values.frombytes(raw)\n"
        "    out = array('f')\n"
        "    for value in values:\n"
        "        out.append(value * 1.5 + 0.25)\n"
        "    return out.tobytes()\n",
        encoding="utf-8",
    )
    out_dir = tmp_path / "out"
    out_dir.mkdir()

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
            "--wasm-profile",
            "pure",
            "--type-hints",
            "ignore",
            "--split-runtime",
            "--out-dir",
            str(out_dir),
        ],
        cwd=root,
        env=_browser_wasm_build_env(root),
        capture_output=True,
        text=True,
        timeout=1800,
    )
    assert build.returncode == 0, build.stderr
    assert (out_dir / "app.wasm").exists()
    assert (out_dir / "molt_runtime.wasm").exists()
    assert (out_dir / "manifest.json").exists()
    assert (out_dir / "browser_embed.js").exists()
    manifest = json.loads((out_dir / "manifest.json").read_text(encoding="utf-8"))
    assert manifest["assets"]["browser_embed"]["path"] == "browser_embed.js"
    browser_abi = manifest["abi"]["browser_embed"]
    assert browser_abi["call_indirect_imports"] == [
        f"molt_call_indirect{arity}" for arity in range(14)
    ]
    assert browser_abi["table_layout"]["legacy_table_base"] == 256
    assert "fast_list_append" in browser_abi["runtime_import_fallbacks"]
    runtime_imports = manifest["abi"]["runtime_imports"]
    assert runtime_imports["signatures"]["molt_exception_init"] == {
        "params": ["i64", "i64"],
        "result": "i64",
    }
    assert runtime_imports["runtime_export_signatures"]["molt_exception_init"] == {
        "params": ["i64", "i64"],
        "result": "i64",
    }

    handler = type("_EmbedHandler", (_StaticDirHandler,), {"root": out_dir})
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        base_url = f"http://127.0.0.1:{server.server_address[1]}/"
        embed_uri = (out_dir / "browser_embed.js").as_uri()
        script = tmp_path / "run_browser_embed_forward.mjs"
        script.write_text(
            f"""
import {{ loadMoltBrowserKernel }} from {embed_uri!r};

const kernel = await loadMoltBrowserKernel({{
  baseUrl: {base_url!r},
  exportName: 'forward',
  resultType: 'float32',
}});
const input = new Float32Array([1.25, -2.5, 0, 4.75]);
const output = await kernel.forward(input);
console.log(JSON.stringify({{
  ctor: output.constructor.name,
  exportName: kernel.exportName,
  values: Array.from(output),
}}));
""".lstrip(),
            encoding="utf-8",
        )
        run = _run_wasm_test_process(
            ["node", str(script)],
            cwd=root,
            capture_output=True,
            text=True,
            timeout=120,
        )
        assert run.returncode == 0, run.stderr
        payload = json.loads(run.stdout)
        assert payload == {
            "ctor": "Float32Array",
            "exportName": "browser_embed_forward__forward",
            "values": [2.125, -3.5, 0.25, 7.375],
        }
    finally:
        server.shutdown()


@pytest.mark.slow
def test_browser_embed_pact_ndimage_primitives_match_scipy_oracle(
    tmp_path: Path,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser embed typed-array test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser embed typed-array test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "browser_embed_ndimage_primitives.py"
    src.write_text(
        "from array import array\n"
        "from scipy.ndimage import (\n"
        "    distance_transform_edt,\n"
        "    gaussian_filter,\n"
        "    label,\n"
        "    maximum_filter,\n"
        "    minimum_filter,\n"
        ")\n"
        "\n"
        "\n"
        "def _grid(values, start):\n"
        "    rows = []\n"
        "    idx = start\n"
        "    for _r in range(3):\n"
        "        row = []\n"
        "        for _c in range(3):\n"
        "            row.append(float(values[idx]))\n"
        "            idx += 1\n"
        "        rows.append(row)\n"
        "    return rows\n"
        "\n"
        "\n"
        "def _mask(values, start):\n"
        "    rows = []\n"
        "    idx = start\n"
        "    for _r in range(3):\n"
        "        row = []\n"
        "        for _c in range(3):\n"
        "            row.append(values[idx] > 0.5)\n"
        "            idx += 1\n"
        "        rows.append(row)\n"
        "    return rows\n"
        "\n"
        "\n"
        "def _append_grid(out, grid):\n"
        "    for row in grid:\n"
        "        for value in row:\n"
        "            out.append(float(value))\n"
        "\n"
        "\n"
        "def forward(raw: bytes):\n"
        "    values = array('f')\n"
        "    values.frombytes(raw)\n"
        "    field = _grid(values, 0)\n"
        "    mask = _mask(values, 9)\n"
        "    labeled, count = label(mask)\n"
        "    out = array('f')\n"
        "    _append_grid(out, distance_transform_edt(mask))\n"
        "    _append_grid(out, gaussian_filter(field, 1.0))\n"
        "    _append_grid(out, maximum_filter(field, size=3))\n"
        "    _append_grid(out, minimum_filter(field, size=3))\n"
        "    _append_grid(out, labeled)\n"
        "    out.append(float(count))\n"
        "    return out.tobytes()\n",
        encoding="utf-8",
    )
    out_dir = tmp_path / "out"
    out_dir.mkdir()

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
            "--wasm-profile",
            "pure",
            "--type-hints",
            "ignore",
            "--split-runtime",
            "--out-dir",
            str(out_dir),
        ],
        cwd=root,
        env=_browser_wasm_build_env(root),
        capture_output=True,
        text=True,
        timeout=1800,
    )
    assert build.returncode == 0, build.stderr

    expected = [
        1.0,
        0.0,
        1.0,
        1.0,
        0.0,
        0.0,
        0.0,
        1.0,
        1.0,
        4.055906295776367,
        4.441690444946289,
        5.113442897796631,
        4.2879791259765625,
        5.049917221069336,
        5.550393104553223,
        4.577517032623291,
        5.620102882385254,
        6.303050994873047,
        9.0,
        9.0,
        9.0,
        9.0,
        9.0,
        9.0,
        9.0,
        9.0,
        9.0,
        1.0,
        1.0,
        1.0,
        1.0,
        1.0,
        1.0,
        2.0,
        2.0,
        3.0,
        1.0,
        0.0,
        2.0,
        1.0,
        0.0,
        0.0,
        0.0,
        3.0,
        3.0,
        3.0,
    ]

    handler = type("_EmbedHandler", (_StaticDirHandler,), {"root": out_dir})
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        base_url = f"http://127.0.0.1:{server.server_address[1]}/"
        embed_uri = (out_dir / "browser_embed.js").as_uri()
        script = tmp_path / "run_browser_embed_ndimage_primitives.mjs"
        script.write_text(
            f"""
import {{ loadMoltBrowserKernel }} from {embed_uri!r};

const kernel = await loadMoltBrowserKernel({{
  baseUrl: {base_url!r},
  exportName: 'forward',
  resultType: 'float32',
}});
const input = new Float32Array([
  5, 1, 7,
  2, 9, 3,
  4, 6, 8,
  1, 0, 1,
  1, 0, 0,
  0, 1, 1,
]);
const output = await kernel.forward(input);
console.log(JSON.stringify({{
  ctor: output.constructor.name,
  exportName: kernel.exportName,
  values: Array.from(output),
}}));
""".lstrip(),
            encoding="utf-8",
        )
        run = _run_wasm_test_process(
            ["node", str(script)],
            cwd=root,
            capture_output=True,
            text=True,
            timeout=120,
        )
        assert run.returncode == 0, run.stderr
        payload = json.loads(run.stdout)
        assert payload["ctor"] == "Float32Array"
        assert payload["exportName"] == "browser_embed_ndimage_primitives__forward"
        actual = payload["values"]
        assert len(actual) == len(expected)
        for idx, (actual_value, expected_value) in enumerate(zip(actual, expected)):
            assert actual_value == pytest.approx(expected_value, abs=1e-5), idx
    finally:
        server.shutdown()
