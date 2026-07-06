from __future__ import annotations

import hashlib
import json
import os
import shutil
import struct
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest

from molt._wasm_abi_generated import WASM_RESERVED_RUNTIME_CALLABLES
from molt.dx import development_artifact_env, session_artifact_component
from tests.wasm_linked_runner import _run_wasm_test_process
from tests.wasm_import_fixtures import build_wasm_tag_import_before_memory


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


def _browser_embed_forward_package_dir(root: Path, env: dict[str, str]) -> Path:
    ext_root = Path(env.get("MOLT_EXT_ROOT") or root).expanduser()
    repo_key = hashlib.sha256(str(root.resolve()).encode("utf-8")).hexdigest()[:12]
    worker = os.environ.get("PYTEST_XDIST_WORKER", "local")
    lane = session_artifact_component(worker or "local")
    return (
        ext_root
        / "tmp"
        / "test-wasm-browser-embed"
        / repo_key
        / lane
        / "browser_embed_forward"
    )


def _wasm_u32(value: int) -> bytes:
    if value < 0:
        raise ValueError(value)
    out = bytearray()
    remaining = value
    while True:
        byte = remaining & 0x7F
        remaining >>= 7
        out.append(byte | 0x80 if remaining else byte)
        if not remaining:
            return bytes(out)


def _wasm_sleb(value: int) -> bytes:
    out = bytearray()
    remaining = value
    while True:
        byte = remaining & 0x7F
        remaining >>= 7
        done = (remaining == 0 and (byte & 0x40) == 0) or (
            remaining == -1 and (byte & 0x40) != 0
        )
        out.append(byte if done else byte | 0x80)
        if done:
            return bytes(out)


def _wasm_str(text: str) -> bytes:
    data = text.encode("utf-8")
    return _wasm_u32(len(data)) + data


def _wasm_vec(items: list[bytes]) -> bytes:
    return _wasm_u32(len(items)) + b"".join(items)


def _wasm_section(section_id: int, payload: bytes) -> bytes:
    return bytes([section_id]) + _wasm_u32(len(payload)) + payload


def _wasm_func_type(params: list[int], results: list[int]) -> bytes:
    return bytes([0x60]) + _wasm_vec([bytes([param]) for param in params]) + _wasm_vec(
        [bytes([result]) for result in results]
    )


def _wasm_import_func(module: str, name: str, type_index: int) -> bytes:
    return _wasm_str(module) + _wasm_str(name) + bytes([0x00]) + _wasm_u32(type_index)


def _wasm_import_memory(module: str, name: str, min_pages: int) -> bytes:
    return (
        _wasm_str(module)
        + _wasm_str(name)
        + bytes([0x02, 0x00])
        + _wasm_u32(min_pages)
    )


def _wasm_export(name: str, kind: int, index: int) -> bytes:
    return _wasm_str(name) + bytes([kind]) + _wasm_u32(index)


def _wasm_i32_const(value: int) -> bytes:
    return bytes([0x41]) + _wasm_sleb(value)


def _wasm_i64_const(value: int) -> bytes:
    return bytes([0x42]) + _wasm_sleb(value)


def _wasm_data_segment(offset: int, payload: bytes) -> bytes:
    offset_expr = _wasm_i32_const(offset) + bytes([0x0B])
    return bytes([0x00]) + offset_expr + _wasm_u32(len(payload)) + payload


def _minimal_wasm_module() -> bytes:
    return b"\x00asm\x01\x00\x00\x00"


def _webgpu_embed_manifest() -> dict[str, object]:
    return {
        "version": 2,
        "mode": "split-runtime",
        "shared_memory_initial_pages": 1,
        "shared_table_initial": 1,
        "wasm_table_base": None,
        "abi": {
            "browser_embed": {
                "call_indirect_imports": ["molt_call_indirect0"],
                "runtime_import_fallbacks": {},
                "reserved_runtime_callables": [],
                "table_layout": {
                    "legacy_table_base": 256,
                    "reserved_runtime_callable_base": 33,
                    "reserved_runtime_callable_count": 0,
                },
            },
            "runtime_imports": {
                "module": "molt_runtime",
                "names": [],
                "signatures": {},
                "result_kinds": {},
                "export_names": {},
                "runtime_export_signatures": {},
            },
            "table_refs": {"app": {}, "runtime": {}},
        },
        "modules": {
            "app": {"path": "app.wasm"},
            "runtime": {"path": "molt_runtime.wasm"},
        },
        "target_features": {
            "authority": "target_feature_manifest",
            "profile": "wasm-browser-webgpu",
            "required_host_imports": {
                "webgpu": ["molt_gpu_webgpu_dispatch_host"],
            },
            "browser_probes": {
                "webgpu": {
                    "required": True,
                },
            },
        },
    }


def _build_webgpu_dispatch_app_wasm() -> tuple[bytes, dict[str, int]]:
    layout = {
        "source_ptr": 64,
        "entry_ptr": 256,
        "input_ptr": 1024,
        "output_ptr": 2048,
        "launch_ptr": 4096,
        "err_ptr": 8192,
        "err_cap": 128,
        "out_err_len_ptr": 8320,
    }
    source = b"@compute @workgroup_size(4) fn main() {}"
    entry = b"main"
    input_payload = struct.pack("<ff", 1.5, -2.0)
    launch = json.dumps(
        {
            "bindings": [
                {
                    "binding": 0,
                    "name": "input",
                    "access": "read",
                    "ptr": layout["input_ptr"],
                    "len": len(input_payload),
                },
                {
                    "binding": 1,
                    "name": "output",
                    "access": "read_write",
                    "ptr": layout["output_ptr"],
                    "len": len(input_payload),
                },
            ]
        },
        separators=(",", ":"),
    ).encode("utf-8")
    layout["source_len"] = len(source)
    layout["entry_len"] = len(entry)
    layout["launch_len"] = len(launch)

    i32 = 0x7F
    i64 = 0x7E
    dispatch_type = _wasm_func_type(
        [i32, i32, i32, i32, i32, i32, i64, i64, i32, i32, i32],
        [i32],
    )
    run_type = _wasm_func_type([], [i32])
    type_section = _wasm_section(1, _wasm_vec([dispatch_type, run_type]))
    import_section = _wasm_section(
        2,
        _wasm_vec(
            [
                _wasm_import_func("env", "molt_gpu_webgpu_dispatch_host", 0),
                _wasm_import_memory("env", "memory", 1),
            ]
        ),
    )
    function_section = _wasm_section(3, _wasm_vec([_wasm_u32(1)]))
    export_section = _wasm_section(
        7,
        _wasm_vec(
            [
                _wasm_export("run_gpu", 0x00, 1),
                _wasm_export("memory", 0x02, 0),
            ]
        ),
    )
    instructions = b"".join(
        [
            _wasm_i32_const(layout["source_ptr"]),
            _wasm_i32_const(layout["source_len"]),
            _wasm_i32_const(layout["entry_ptr"]),
            _wasm_i32_const(layout["entry_len"]),
            _wasm_i32_const(layout["launch_ptr"]),
            _wasm_i32_const(layout["launch_len"]),
            _wasm_i64_const(2),
            _wasm_i64_const(4),
            _wasm_i32_const(layout["err_ptr"]),
            _wasm_i32_const(layout["err_cap"]),
            _wasm_i32_const(layout["out_err_len_ptr"]),
            bytes([0x10]) + _wasm_u32(0),
            bytes([0x0B]),
        ]
    )
    body = _wasm_u32(0) + instructions
    code_section = _wasm_section(10, _wasm_vec([_wasm_u32(len(body)) + body]))
    data_section = _wasm_section(
        11,
        _wasm_vec(
            [
                _wasm_data_segment(layout["source_ptr"], source),
                _wasm_data_segment(layout["entry_ptr"], entry),
                _wasm_data_segment(layout["input_ptr"], input_payload),
                _wasm_data_segment(layout["launch_ptr"], launch),
            ]
        ),
    )
    return (
        _minimal_wasm_module()
        + type_section
        + import_section
        + function_section
        + export_section
        + code_section
        + data_section,
        layout,
    )


def test_browser_host_import_parser_handles_tag_imports_before_memory(
    tmp_path: Path,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser embed parser test")

    root = Path(__file__).resolve().parents[1]
    wasm_path = tmp_path / "tag_import_before_memory.wasm"
    wasm_path.write_bytes(build_wasm_tag_import_before_memory())
    browser_host_uri = (root / "wasm" / "browser_host.js").as_uri()
    script = tmp_path / "parse_browser_host_imports.mjs"
    script.write_text(
        f"""
import {{ parseWasmImports }} from {browser_host_uri!r};
import fs from 'node:fs';

const imports = parseWasmImports(fs.readFileSync(process.argv[2]));
console.log(JSON.stringify(imports));
""".lstrip(),
        encoding="utf-8",
    )

    run = _run_wasm_test_process(
        ["node", str(script), str(wasm_path)],
        cwd=root,
        env=os.environ,
        timeout=30,
    )
    assert run.returncode == 0, run.stderr

    parsed = json.loads(run.stdout)
    assert parsed["memory"] == {"min": 1, "max": None}
    assert parsed["funcImports"] == []
    assert parsed["tagImports"] == [
        {
            "module": "env",
            "name": "molt_exception",
            "attribute": 0,
            "typeIndex": 0,
            "parameters": [],
            "results": [],
        }
    ]


def test_browser_embed_import_parser_handles_tag_imports_before_memory(
    tmp_path: Path,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser embed parser test")

    root = Path(__file__).resolve().parents[1]
    wasm_path = tmp_path / "tag_import_before_memory.wasm"
    wasm_path.write_bytes(build_wasm_tag_import_before_memory())
    browser_embed_uri = (root / "wasm" / "browser_embed.js").as_uri()
    script = tmp_path / "parse_browser_embed_imports.mjs"
    script.write_text(
        f"""
import {{ parseMoltWasmImports }} from {browser_embed_uri!r};
import fs from 'node:fs';

const imports = parseMoltWasmImports(fs.readFileSync(process.argv[2]));
console.log(JSON.stringify(imports));
""".lstrip(),
        encoding="utf-8",
    )

    run = _run_wasm_test_process(
        ["node", str(script), str(wasm_path)],
        cwd=root,
        env=os.environ,
        timeout=30,
    )
    assert run.returncode == 0, run.stderr

    parsed = json.loads(run.stdout)
    assert parsed["memory"] == {"min": 1, "max": None}
    assert parsed["funcImports"] == []
    assert parsed["tagImports"] == [
        {
            "module": "env",
            "name": "molt_exception",
            "attribute": 0,
            "typeIndex": 0,
            "parameters": [],
            "results": [],
        }
    ]


def test_browser_host_runtime_import_bridge_requires_manifest_abi() -> None:
    root = Path(__file__).resolve().parents[1]
    browser_host = (root / "wasm" / "browser_host.js").read_text(encoding="utf-8")

    assert "if (!manifestNames.has(entry.name))" in browser_host
    assert "if (!signature || !Array.isArray(signature.params))" in browser_host
    assert "if (!resultKind)" in browser_host
    assert "fn = resolveFallback(entry.name);" in browser_host
    assert "const hasFallback" not in browser_host
    assert "if (!hasManifestImport && !hasFallback)" not in browser_host


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


def test_browser_embed_routes_webgpu_import_through_shared_dispatch_host(
    tmp_path: Path,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser WebGPU embed import test")

    root = Path(__file__).resolve().parents[1]
    package_dir = tmp_path / "webgpu_embed"
    package_dir.mkdir()
    app_wasm, layout = _build_webgpu_dispatch_app_wasm()
    (package_dir / "app.wasm").write_bytes(app_wasm)
    (package_dir / "molt_runtime.wasm").write_bytes(_minimal_wasm_module())
    manifest = _webgpu_embed_manifest()
    (package_dir / "manifest.json").write_text(
        json.dumps(manifest, sort_keys=True),
        encoding="utf-8",
    )

    handler = type("_WebGpuEmbedHandler", (_StaticDirHandler,), {"root": package_dir})
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        base_url = f"http://127.0.0.1:{server.server_address[1]}/"
        embed_uri = (root / "wasm" / "browser_embed.js").as_uri()
        script = tmp_path / "run_webgpu_embed_import.mjs"
        script.write_text(
            f"""
import {{ loadMoltBrowserEmbed }} from {embed_uri!r};

const manifest = {json.dumps(manifest, sort_keys=True)};
const layout = {json.dumps(layout, sort_keys=True)};
const dispatches = [];
const embed = await loadMoltBrowserEmbed({{
  baseUrl: {base_url!r},
  manifest,
  gpuKernelDispatcher: {{
    dispatchKernel(request) {{
      dispatches.push({{
        source: request.source,
        entry: request.entry,
        grid: request.grid,
        workgroupSize: request.workgroupSize,
        bindingNames: request.bindings.map((binding) => binding.name),
      }});
      const input = request.bindings.find((binding) => binding.name === 'input').bytes;
      const output = request.bindings.find((binding) => binding.name === 'output').bytes;
      const inView = new DataView(input.buffer, input.byteOffset, input.byteLength);
      const outView = new DataView(output.buffer, output.byteOffset, output.byteLength);
      outView.setFloat32(0, inView.getFloat32(0, true) * 3, true);
      outView.setFloat32(4, inView.getFloat32(4, true) * 3, true);
    }},
  }},
}});
const rc = embed.appInstance.exports.run_gpu();
const view = new DataView(embed.memory.buffer);
console.log(JSON.stringify({{
  rc,
  errLen: view.getUint32(layout.out_err_len_ptr, true),
  output: [
    view.getFloat32(layout.output_ptr, true),
    view.getFloat32(layout.output_ptr + 4, true),
  ],
  dispatches,
}}));
""".lstrip(),
            encoding="utf-8",
        )
        run = _run_wasm_test_process(
            ["node", str(script)],
            cwd=root,
            env=os.environ,
            timeout=30,
        )
        assert run.returncode == 0, run.stderr
        assert json.loads(run.stdout) == {
            "rc": 0,
            "errLen": 0,
            "output": [4.5, -6.0],
            "dispatches": [
                {
                    "source": "@compute @workgroup_size(4) fn main() {}",
                    "entry": "main",
                    "grid": 2,
                    "workgroupSize": 4,
                    "bindingNames": ["input", "output"],
                }
            ],
        }
    finally:
        server.shutdown()


def test_browser_embed_rejects_webgpu_target_feature_manifest_drift(
    tmp_path: Path,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser WebGPU target-feature drift test")

    root = Path(__file__).resolve().parents[1]
    package_dir = tmp_path / "webgpu_embed_bad_manifest"
    package_dir.mkdir()
    app_wasm, _layout = _build_webgpu_dispatch_app_wasm()
    (package_dir / "app.wasm").write_bytes(app_wasm)
    (package_dir / "molt_runtime.wasm").write_bytes(_minimal_wasm_module())
    manifest = _webgpu_embed_manifest()
    assert isinstance(manifest["target_features"], dict)
    manifest["target_features"]["profile"] = "wasm-browser"
    (package_dir / "manifest.json").write_text(
        json.dumps(manifest, sort_keys=True),
        encoding="utf-8",
    )

    handler = type(
        "_WebGpuBadManifestHandler",
        (_StaticDirHandler,),
        {"root": package_dir},
    )
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        base_url = f"http://127.0.0.1:{server.server_address[1]}/"
        embed_uri = (root / "wasm" / "browser_embed.js").as_uri()
        script = tmp_path / "run_webgpu_bad_manifest.mjs"
        script.write_text(
            f"""
import {{ loadMoltBrowserEmbed }} from {embed_uri!r};

try {{
  await loadMoltBrowserEmbed({{
    baseUrl: {base_url!r},
    manifest: {json.dumps(manifest, sort_keys=True)},
  }});
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
            env=os.environ,
            timeout=30,
        )
        assert run.returncode == 0, run.stderr
        assert "target_features.profile must be wasm-browser-webgpu" in run.stdout
    finally:
        server.shutdown()


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


def test_browser_embed_object_call_native_callable_import_adapter(
    tmp_path: Path,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser embed native callable adapter test")

    root = Path(__file__).resolve().parents[1]
    embed_uri = (root / "wasm" / "browser_embed.js").as_uri()
    script = tmp_path / "run_object_call_native_callable.mjs"
    script.write_text(
        f"""
import {{ createMoltNativeCallableImports }} from {embed_uri!r};

const symbol = 'molt_scipy_ndimage_distance_transform_edt';
const imports = createMoltNativeCallableImports(
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
                abi: 'molt.object_call_v1',
                binding: 'direct_symbol',
                signature: {{
                  params: ['molt.value...'],
                  result: 'molt.value',
                }},
                exports: ['scipy.ndimage.distance_transform_edt'],
              }},
            }},
          }},
        }},
      }},
    }},
    nativeCallables: {{
      [symbol]: (maskBits, ctx) => {{
        if (ctx.abi !== 'molt.object_call_v1') throw new Error('wrong abi');
        if (ctx.arity !== 1) throw new Error('wrong arity');
        if (typeof maskBits !== 'bigint') throw new Error('arg not bigint');
        return maskBits + 7n;
      }},
    }},
  }},
);

console.log(imports[symbol](35n).toString());
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
    assert run.stdout.strip() == "42"


def test_browser_embed_pyinit_native_callable_import_adapter(
    tmp_path: Path,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser embed native callable adapter test")

    root = Path(__file__).resolve().parents[1]
    embed_uri = (root / "wasm" / "browser_embed.js").as_uri()
    script = tmp_path / "run_pyinit_native_callable.mjs"
    script.write_text(
        f"""
import {{ createMoltNativeCallableImports }} from {embed_uri!r};

const symbol = 'PyInit__nd_image';
const imports = createMoltNativeCallableImports(
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
                abi: 'molt.pyinit_module_v1',
                binding: 'direct_symbol',
                signature: {{
                  params: [],
                  result: 'molt.pyobject_ptr',
                }},
                exports: [{{ qualified_name: 'scipy.ndimage._nd_image' }}],
              }},
            }},
          }},
        }},
      }},
    }},
    nativeCallables: {{
      [symbol]: (ctx) => {{
        if (ctx.abi !== 'molt.pyinit_module_v1') throw new Error('wrong abi');
        if (ctx.arity !== 0) throw new Error('wrong arity');
        return 0x12345678;
      }},
    }},
  }},
);

console.log(imports[symbol]().toString(16));
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
    assert run.stdout.strip() == "12345678"


def test_browser_embed_object_call_native_callable_result_must_be_handle(
    tmp_path: Path,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser embed native callable adapter test")

    root = Path(__file__).resolve().parents[1]
    embed_uri = (root / "wasm" / "browser_embed.js").as_uri()
    script = tmp_path / "run_bad_object_call_native_callable.mjs"
    script.write_text(
        f"""
import {{ createMoltNativeCallableImports }} from {embed_uri!r};

const symbol = 'molt_scipy_ndimage_bad_result';
const imports = createMoltNativeCallableImports(
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
                abi: 'molt.object_call_v1',
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
      [symbol]: () => 'not-a-handle',
    }},
  }},
);

try {{
  imports[symbol](1n);
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
    assert "molt.object_call_v1 result must be a Molt value handle" in run.stdout


def test_browser_embed_object_callargs_native_callable_import_adapter(
    tmp_path: Path,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser embed native callable adapter test")

    root = Path(__file__).resolve().parents[1]
    embed_uri = (root / "wasm" / "browser_embed.js").as_uri()
    script = tmp_path / "run_object_callargs_native_callable.mjs"
    script.write_text(
        f"""
import {{ createMoltNativeCallableImports }} from {embed_uri!r};

const symbol = 'molt_scipy_ndimage_gaussian_filter';
const imports = createMoltNativeCallableImports(
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
                abi: 'molt.object_callargs_v1',
                binding: 'direct_symbol',
                signature: {{
                  params: ['molt.callargs'],
                  result: 'molt.value',
                }},
                exports: ['scipy.ndimage.gaussian_filter'],
              }},
            }},
          }},
        }},
      }},
    }},
    nativeCallables: {{
      [symbol]: (callargsBits, ctx) => {{
        if (ctx.abi !== 'molt.object_callargs_v1') throw new Error('wrong abi');
        if (ctx.arity !== 1) throw new Error('wrong arity');
        if (typeof callargsBits !== 'bigint') throw new Error('arg not bigint');
        return callargsBits + 5n;
      }},
    }},
  }},
);

console.log(imports[symbol](37n).toString());
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
    assert run.stdout.strip() == "42"


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
    build_env = _browser_wasm_build_env(root)
    package_dir = _browser_embed_forward_package_dir(root, build_env)
    package_dir.mkdir(parents=True, exist_ok=True)

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
            str(package_dir),
        ],
        cwd=root,
        env=build_env,
        capture_output=True,
        text=True,
        timeout=1800,
    )
    assert build.returncode == 0, build.stderr
    out_dir = tmp_path / "out"
    shutil.copytree(package_dir, out_dir)
    assert (out_dir / "app.wasm").exists()
    assert (out_dir / "molt_runtime.wasm").exists()
    assert (out_dir / "manifest.json").exists()
    assert (out_dir / "browser_embed.js").exists()
    assert (out_dir / "browser_gpu_dispatch.js").exists()
    assert (out_dir / "browser_gpu_worker.js").exists()
    assert (out_dir / "browser_target_features.js").exists()
    assert (out_dir / "target_feature_constants.generated.js").exists()
    assert (out_dir / "loader_bridge.js").exists()
    assert (out_dir / "target_feature_manifest.json").exists()
    manifest = json.loads((out_dir / "manifest.json").read_text(encoding="utf-8"))
    assert manifest["assets"]["browser_embed"]["path"] == "browser_embed.js"
    assert manifest["assets"]["browser_gpu_dispatch"]["path"] == "browser_gpu_dispatch.js"
    assert manifest["assets"]["browser_gpu_dispatch"]["size"] == (
        out_dir / "browser_gpu_dispatch.js"
    ).stat().st_size
    assert manifest["assets"]["browser_gpu_worker"]["path"] == "browser_gpu_worker.js"
    assert manifest["assets"]["browser_gpu_worker"]["size"] == (
        out_dir / "browser_gpu_worker.js"
    ).stat().st_size
    assert (
        manifest["assets"]["browser_target_features"]["path"]
        == "browser_target_features.js"
    )
    assert manifest["assets"]["browser_target_features"]["size"] == (
        out_dir / "browser_target_features.js"
    ).stat().st_size
    assert manifest["assets"]["target_feature_constants"]["path"] == (
        "target_feature_constants.generated.js"
    )
    assert manifest["assets"]["target_feature_constants"]["size"] == (
        out_dir / "target_feature_constants.generated.js"
    ).stat().st_size
    assert manifest["assets"]["loader_bridge"]["path"] == "loader_bridge.js"
    target_feature_asset = manifest["assets"]["target_feature_manifest"]
    assert target_feature_asset["path"] == "target_feature_manifest.json"
    assert (
        target_feature_asset["size"]
        == (out_dir / "target_feature_manifest.json").stat().st_size
    )
    assert len(target_feature_asset["sha256"]) == 64
    assert manifest["target_features"]["authority"] == "target_feature_manifest"
    assert manifest["target_features"]["profile"] == "wasm-browser"
    assert manifest["target_features"]["manifest_asset"] == target_feature_asset
    browser_abi = manifest["abi"]["browser_embed"]
    assert browser_abi["call_indirect_imports"] == [
        f"molt_call_indirect{arity}" for arity in range(14)
    ]
    assert browser_abi["table_layout"]["legacy_table_base"] == 256
    assert browser_abi["reserved_runtime_callables"] == [
        {
            "index": index,
            "runtime_export": runtime_name,
            "import_name": import_name,
            "arity": arity,
            "dispatch": dispatch,
        }
        for (
            index,
            runtime_name,
            import_name,
            arity,
            dispatch,
        ) in WASM_RESERVED_RUNTIME_CALLABLES
    ]
    assert "fast_list_append" in browser_abi["runtime_import_fallbacks"]
    runtime_imports = manifest["abi"]["runtime_imports"]
    import molt.wasm_artifact as wasm_artifact

    app_runtime_import_names = wasm_artifact._collect_wasm_module_import_names(
        out_dir / "app.wasm",
        "molt_runtime",
    )
    assert runtime_imports["module"] == "molt_runtime"
    assert runtime_imports["names"] == sorted(app_runtime_import_names)
    runtime_import_name_set = set(runtime_imports["names"])
    assert runtime_import_name_set
    assert set(runtime_imports["export_names"]).issubset(runtime_import_name_set)
    assert set(runtime_imports["signatures"]) == runtime_import_name_set
    assert set(runtime_imports["result_kinds"]) == runtime_import_name_set
    assert set(runtime_imports["runtime_export_signatures"]).issubset(
        runtime_import_name_set
    )
    for name, signature in runtime_imports["signatures"].items():
        assert isinstance(signature["params"], list), name
        assert isinstance(signature["result"], str), name
        assert runtime_imports["result_kinds"][name]
    assert "molt_exception_init" not in runtime_imports["signatures"]
    assert "molt_exception_init" not in runtime_imports["export_names"]
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
