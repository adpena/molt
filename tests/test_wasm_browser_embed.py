from __future__ import annotations

import json
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
        session_prefix="test-wasm-browser-embed",
        session_id="test-wasm-browser-embed",
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


def test_browser_embed_forward_f32_native_callable_import_adapter(
    tmp_path: Path,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser embed native callable adapter test")

    root = Path(__file__).resolve().parents[1]
    embed_uri = (root / "wasm" / "browser_embed.js").as_uri()
    script = tmp_path / "run_forward_f32_native_callable.mjs"
    script.write_text(
        f"""
import {{ createMoltNativeCallableImports }} from {embed_uri!r};

const symbol = 'molt_nativepkg_ndimage_distance_transform_edt';
const memory = new WebAssembly.Memory({{ initial: 1 }});
const inputPtr = 1024;
const outputPtr = 2048;

const input = new Float32Array([1.0, -2.0, 0.5]);
new Float32Array(memory.buffer, inputPtr, input.length).set(input);

const runtimeInstance = {{
  exports: {{}},
}};

const imports = createMoltNativeCallableImports(
  {{ memory, runtimeInstance }},
  {{ funcImports: [{{ module: 'molt_native', name: symbol }}] }},
  {{
    manifest: {{
      abi: {{
        browser_embed: {{
          native_callables: {{
            module: 'molt_native',
            symbols: {{
              [symbol]: {{
                abi: 'molt.forward_f32_v1',
                binding: 'direct_symbol',
                signature: {{
                  params: ['bytes.float32'],
                  result: 'bytes.float32',
                }},
                exports: ['nativepkg.ndimage.distance_transform_edt'],
              }},
            }},
          }},
        }},
      }},
    }},
    nativeCallables: {{
      [symbol]: (values, output, ctx) => {{
        if (ctx.abi !== 'molt.forward_f32_v1') throw new Error('wrong abi');
        for (let i = 0; i < values.length; i += 1) {{
          output[i] = values[i] * 2 + 0.25;
        }}
      }},
    }},
  }},
);

const status = imports[symbol](inputPtr, BigInt(input.byteLength), outputPtr);
if (status !== 0) throw new Error(`unexpected native status ${{status}}`);
const output = new Float32Array(memory.buffer, outputPtr, input.length);
console.log(JSON.stringify(Array.from(output)));
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
    assert json.loads(run.stdout) == [2.25, -3.75, 1.25]


def test_browser_embed_native_callable_import_must_be_manifest_declared(
    tmp_path: Path,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser embed native callable adapter test")

    root = Path(__file__).resolve().parents[1]
    embed_uri = (root / "wasm" / "browser_embed.js").as_uri()
    script = tmp_path / "run_missing_native_callable_manifest.mjs"
    script.write_text(
        f"""
import {{ createMoltNativeCallableImports }} from {embed_uri!r};

try {{
  createMoltNativeCallableImports(
    {{ memory: new WebAssembly.Memory({{ initial: 1 }}), runtimeInstance: null }},
    {{ funcImports: [{{ module: 'molt_native', name: 'molt_nativepkg_missing' }}] }},
    {{
      requireNativeCallableManifest: true,
      manifest: {{
        abi: {{
          browser_embed: {{
            native_callables: {{
              module: 'molt_native',
              symbols: {{}},
            }},
          }},
        }},
      }},
      nativeCallables: {{
        molt_nativepkg_missing: () => 0n,
      }},
    }},
  );
  console.log('unexpected-ok');
}} catch (err) {{
  console.log(String(err.message || err));
}}
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
    assert "missing from manifest.abi.browser_embed.native_callables.symbols" in (
        run.stdout
    )


def test_browser_embed_native_callable_signature_must_match_abi(
    tmp_path: Path,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser embed native callable adapter test")

    root = Path(__file__).resolve().parents[1]
    embed_uri = (root / "wasm" / "browser_embed.js").as_uri()
    script = tmp_path / "run_bad_native_callable_signature.mjs"
    script.write_text(
        f"""
import {{ createMoltNativeCallableImports }} from {embed_uri!r};

const symbol = 'molt_nativepkg_bad_signature';
try {{
  createMoltNativeCallableImports(
    {{ memory: new WebAssembly.Memory({{ initial: 1 }}), runtimeInstance: null }},
    {{ funcImports: [{{ module: 'molt_native', name: symbol }}] }},
    {{
      manifest: {{
        abi: {{
          browser_embed: {{
            native_callables: {{
              module: 'molt_native',
              symbols: {{
                [symbol]: {{
                  abi: 'molt.forward_f32_v1',
                  binding: 'direct_symbol',
                  signature: {{
                    params: ['molt.value...'],
                    result: 'molt.value',
                  }},
                }},
              }},
            }},
          }},
        }},
      }},
      nativeCallables: {{
        [symbol]: () => 0n,
      }},
    }},
  );
  console.log('unexpected-ok');
}} catch (err) {{
  console.log(String(err.message || err));
}}
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
    assert "signature must match molt.forward_f32_v1" in run.stdout


@pytest.mark.slow
def test_browser_embed_forward_roundtrips_float32_typed_arrays(
    tmp_path: Path,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser embed typed-array test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser embed typed-array test")

    root = Path(__file__).resolve().parents[1]
    src = root / "examples" / "browser_embed_forward" / "forward.py"
    assert src.exists()
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
    assert "molt_exception_init" not in runtime_imports["signatures"]
    assert "molt_exception_init" not in runtime_imports["runtime_export_signatures"]

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
            "exportName": "forward__forward",
            "values": [2.125, -3.5, 0.25, 7.375],
        }
    finally:
        server.shutdown()
