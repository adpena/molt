from __future__ import annotations

import json
import os
import shutil
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest

from tests.wasm_linked_runner import _run_wasm_test_process, wasm_test_build_env


_VECTOR_ADD_GPU_PROGRAM = (
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
    "print(gpu.from_device(c))\n"
)


def _write_vector_add_gpu_program(path: Path, *, immutable: bool = False) -> None:
    program = _VECTOR_ADD_GPU_PROGRAM
    if immutable:
        program = program.replace(
            "c = gpu.alloc(4, float)",
            "c = gpu.to_device([11.0, 22.0, 33.0, 44.0])\noriginal = c._data",
        )
        program = program.replace(
            "vector_add[1, 4](a, b, c, 4)",
            "vector_add[1, 4](a, b, c, 0)\nassert c._data is original\nvector_add[1, 4](a, b, c, 4)\nassert isinstance(c._data, bytearray)\nassert c._data is not original",
        )
    path.write_text(program, encoding="utf-8")


def _compiled_gpu_build_timeout() -> float:
    raw = os.environ.get("MOLT_WASM_BROWSER_GPU_BUILD_TIMEOUT")
    if raw is None:
        return 2400.0
    try:
        return float(raw)
    except ValueError as exc:
        raise AssertionError(
            "MOLT_WASM_BROWSER_GPU_BUILD_TIMEOUT must be a numeric seconds value"
        ) from exc


@pytest.mark.parametrize("immutable", [False, True])
def test_browser_host_direct_mode_compiled_gpu_kernel_uses_webgpu_dispatch(
    tmp_path: Path,
    immutable: bool,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser host GPU direct-mode test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser host GPU direct-mode test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "browser_host_gpu.py"
    _write_vector_add_gpu_program(src, immutable=immutable)

    build_env = wasm_test_build_env(
        root,
        session_prefix="browser-gpu-host",
        session_id="test-browser-turboquant-webgpu",
        linked=False,
    )
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
        timeout=900,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "app.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    manifest_path = tmp_path / "manifest.json"
    assert output_wasm.exists()
    assert runtime_wasm.exists()
    assert manifest_path.exists()

    class _WasmHandler(BaseHTTPRequestHandler):
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
        script = tmp_path / "run_browser_gpu.mjs"
        script.write_text(
            f"""
import {{ loadMoltWasm }} from {browser_host_uri!r};

const baseUrl = {base_url!r};
const fakeState = {{ dispatchCount: 0, shaderCount: 0 }};

const writeI32 = (bytes, index, value) => new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).setInt32(index * 4, value, true);
const readI32 = (bytes, index) => new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getInt32(index * 4, true);

const host = await loadMoltWasm({{
  manifestUrl: `${{baseUrl}}/manifest.json`,
  preferLinked: false,
  env: {{ MOLT_GPU_BACKEND: 'webgpu' }},
  gpuKernelDispatcher: {{
    dispatchKernel(request) {{
      fakeState.dispatchCount += 1;
      fakeState.shaderCount += 1;
      if (request.bindings.length !== {5 if immutable else 4}) throw new Error('wrong physical binding count');
      if (request.source.includes('atomicStore(') !== {str(immutable).lower()}) throw new Error('wrong store occurrence contract');
      const a = request.bindings.find((binding) => binding.binding === 0).bytes;
      const b = request.bindings.find((binding) => binding.binding === 1).bytes;
      const c = request.bindings.find((binding) => binding.binding === 2).bytes;
      const n = readI32(request.bindings.find((binding) => binding.binding === 3).bytes, 0);
      const workgroupSizeMatch = request.source.match(/@workgroup_size\\((\\d+)\\)/);
      const workgroupSize = workgroupSizeMatch ? Number(workgroupSizeMatch[1]) : 1;
      const totalThreads = Number(request.grid) * workgroupSize;
      for (let tid = 0; tid < totalThreads && tid < n; tid += 1) {{
        writeI32(c, tid, readI32(a, tid) + readI32(b, tid));
        {"writeI32(request.bindings[4].bytes, 0, 1);" if immutable else ""}
      }}
    }},
  }},
}});
try {{ host.run(); }} finally {{ host.dispose(); }}
console.log(JSON.stringify(fakeState));
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
        lines = [line.strip() for line in run.stdout.splitlines() if line.strip()]
        assert lines[0] == "[11.0, 22.0, 33.0, 44.0]"
        assert json.loads(lines[1]) == {
            "dispatchCount": 2 if immutable else 1,
            "shaderCount": 2 if immutable else 1,
        }
    finally:
        server.shutdown()


@pytest.mark.parametrize("immutable", [False, True])
def test_browser_host_split_runtime_compiled_gpu_kernel_uses_webgpu_dispatch(
    tmp_path: Path,
    immutable: bool,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser host GPU split-runtime test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser host GPU split-runtime test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "browser_host_split_gpu.py"
    _write_vector_add_gpu_program(src, immutable=immutable)

    build_env = wasm_test_build_env(
        root,
        session_prefix="browser-gpu-host",
        session_id="test-browser-split-webgpu",
        linked=False,
    )
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
            str(tmp_path),
        ],
        cwd=root,
        env=build_env,
        capture_output=True,
        text=True,
        timeout=_compiled_gpu_build_timeout(),
    )
    assert build.returncode == 0, build.stderr

    app_wasm = tmp_path / "app.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    manifest_path = tmp_path / "manifest.json"
    assert app_wasm.exists()
    assert runtime_wasm.exists()
    assert manifest_path.exists()
    assert (tmp_path / "browser_gpu_dispatch.js").exists()
    assert (tmp_path / "browser_gpu_worker.js").exists()
    assert (tmp_path / "browser_target_features.js").exists()
    assert (tmp_path / "target_feature_manifest.json").exists()

    import molt.wasm_artifact as wasm_artifact

    runtime_env_imports = wasm_artifact._collect_wasm_module_import_names(
        runtime_wasm,
        "env",
    )
    assert "molt_gpu_webgpu_dispatch_host" in runtime_env_imports
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    assert manifest["target_features"]["profile"] == "wasm-browser-webgpu"
    assert manifest["target_features"]["required_host_imports"]["webgpu"] == [
        "molt_gpu_webgpu_dispatch_host"
    ]
    assert manifest["target_features"]["browser_probes"]["webgpu"]["required"] is True
    assert manifest["assets"]["browser_gpu_dispatch"]["path"] == (
        "browser_gpu_dispatch.js"
    )
    assert manifest["assets"]["browser_gpu_worker"]["path"] == "browser_gpu_worker.js"
    assert (
        manifest["assets"]["browser_target_features"]["path"]
        == "browser_target_features.js"
    )

    class _WasmHandler(BaseHTTPRequestHandler):
        def log_message(self, fmt: str, *args: object) -> None:
            return None

        def do_GET(self) -> None:  # noqa: N802
            path = self.path.split("?", 1)[0]
            files = {
                "/app.wasm": app_wasm,
                "/molt_runtime.wasm": runtime_wasm,
                "/manifest.json": manifest_path,
            }
            payload_path = files.get(path)
            if payload_path is None:
                self.send_response(404)
                self.end_headers()
                return
            payload = payload_path.read_bytes()
            self.send_response(200)
            content_type = (
                "application/wasm"
                if payload_path.suffix == ".wasm"
                else "application/json"
            )
            self.send_header("content-type", content_type)
            self.send_header("content-length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

    server = ThreadingHTTPServer(("127.0.0.1", 0), _WasmHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        base_url = f"http://127.0.0.1:{server.server_address[1]}"
        browser_host_uri = (root / "wasm" / "browser_host.js").as_uri()
        script = tmp_path / "run_browser_split_gpu.mjs"
        script.write_text(
            f"""
import {{ loadMoltWasm }} from {browser_host_uri!r};

const baseUrl = {base_url!r};
const fakeState = {{ dispatchCount: 0, shaderCount: 0 }};

const writeI32 = (bytes, index, value) => new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).setInt32(index * 4, value, true);
const readI32 = (bytes, index) => new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getInt32(index * 4, true);

const host = await loadMoltWasm({{
  manifestUrl: `${{baseUrl}}/manifest.json`,
  preferLinked: false,
  env: {{ MOLT_GPU_BACKEND: 'webgpu' }},
  gpuKernelDispatcher: {{
    dispatchKernel(request) {{
      fakeState.dispatchCount += 1;
      fakeState.shaderCount += 1;
      if (request.bindings.length !== {5 if immutable else 4}) throw new Error('wrong physical binding count');
      if (request.source.includes('atomicStore(') !== {str(immutable).lower()}) throw new Error('wrong store occurrence contract');
      const a = request.bindings.find((binding) => binding.binding === 0).bytes;
      const b = request.bindings.find((binding) => binding.binding === 1).bytes;
      const c = request.bindings.find((binding) => binding.binding === 2).bytes;
      const n = readI32(request.bindings.find((binding) => binding.binding === 3).bytes, 0);
      const workgroupSizeMatch = request.source.match(/@workgroup_size\\((\\d+)\\)/);
      const workgroupSize = workgroupSizeMatch ? Number(workgroupSizeMatch[1]) : 1;
      const totalThreads = Number(request.grid) * workgroupSize;
      for (let tid = 0; tid < totalThreads && tid < n; tid += 1) {{
        writeI32(c, tid, readI32(a, tid) + readI32(b, tid));
        {"writeI32(request.bindings[4].bytes, 0, 1);" if immutable else ""}
      }}
    }},
  }},
}});
try {{ host.run(); }} finally {{ host.dispose(); }}
console.log(JSON.stringify(fakeState));
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
        lines = [line.strip() for line in run.stdout.splitlines() if line.strip()]
        assert lines[0] == "[11.0, 22.0, 33.0, 44.0]"
        assert json.loads(lines[1]) == {
            "dispatchCount": 2 if immutable else 1,
            "shaderCount": 2 if immutable else 1,
        }
    finally:
        server.shutdown()


def test_browser_host_direct_mode_tensor_linear_uses_webgpu_dispatch(
    tmp_path: Path,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser host GPU direct-mode test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser host GPU direct-mode test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "browser_host_tensor_linear.py"
    src.write_text(
        "from molt.gpu.tensor import Tensor\n"
        "\n"
        "x = Tensor([[1.0, 2.0], [3.0, 4.0]])\n"
        "w = Tensor([[5.0, 6.0], [7.0, 8.0], [9.0, 10.0]])\n"
        "print(x.linear(w).to_list())\n",
        encoding="utf-8",
    )

    build_env = wasm_test_build_env(root, linked=False)
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
        timeout=900,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "app.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    manifest_path = tmp_path / "manifest.json"
    assert output_wasm.exists()
    assert runtime_wasm.exists()
    assert manifest_path.exists()

    class _WasmHandler(BaseHTTPRequestHandler):
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
        script = tmp_path / "run_browser_tensor_linear.mjs"
        script.write_text(
            f"""
import {{ loadMoltWasm }} from {browser_host_uri!r};

const baseUrl = {base_url!r};
const fakeState = {{ dispatchCount: 0 }};

const f32View = (bytes) => new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
const readF32 = (bytes, index) => f32View(bytes).getFloat32(index * 4, true);
const writeF32 = (bytes, index, value) => f32View(bytes).setFloat32(index * 4, value, true);
const readI32 = (bytes, index) => new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getInt32(index * 4, true);

const host = await loadMoltWasm({{
  manifestUrl: `${{baseUrl}}/manifest.json`,
  preferLinked: false,
  env: {{ MOLT_GPU_BACKEND: 'webgpu' }},
  gpuKernelDispatcher: {{
    dispatchKernel(request) {{
      fakeState.dispatchCount += 1;
      const bindings = new Map(request.bindings.map((binding) => [binding.name, binding]));
      const x = bindings.get('x').bytes;
      const weight = bindings.get('weight').bytes;
      const out = bindings.get('out').bytes;
      const outer = readI32(bindings.get('outer').bytes, 0);
      const inFeatures = readI32(bindings.get('in_features').bytes, 0);
      const outFeatures = readI32(bindings.get('out_features').bytes, 0);
      for (let row = 0; row < outer; row += 1) {{
        for (let col = 0; col < outFeatures; col += 1) {{
          let acc = 0.0;
          for (let k = 0; k < inFeatures; k += 1) {{
            acc += readF32(x, row * inFeatures + k) * readF32(weight, col * inFeatures + k);
          }}
          writeF32(out, row * outFeatures + col, acc);
        }}
      }}
    }},
  }},
}});
try {{ host.run(); }} finally {{ host.dispose(); }}
console.log(JSON.stringify(fakeState));
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
        lines = [line.strip() for line in run.stdout.splitlines() if line.strip()]
        assert lines[0] == "[[17.0, 23.0, 29.0], [39.0, 53.0, 67.0]]"
        assert json.loads(lines[1]) == {"dispatchCount": 1}
    finally:
        server.shutdown()


def test_browser_host_direct_mode_tinygrad_linear_uses_webgpu_dispatch(
    tmp_path: Path,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser host GPU direct-mode test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser host GPU direct-mode test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "browser_host_tinygrad_linear.py"
    src.write_text(
        "from tinygrad import Tensor, nn\n"
        "\n"
        "layer = nn.Linear(2, 3, bias=False)\n"
        "layer.load_weights([[5.0, 6.0], [7.0, 8.0], [9.0, 10.0]])\n"
        "x = Tensor([[1.0, 2.0], [3.0, 4.0]])\n"
        "print(layer(x).to_list())\n",
        encoding="utf-8",
    )

    build_env = wasm_test_build_env(root, linked=False)
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
        timeout=900,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "app.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    manifest_path = tmp_path / "manifest.json"
    assert output_wasm.exists()
    assert runtime_wasm.exists()
    assert manifest_path.exists()

    class _WasmHandler(BaseHTTPRequestHandler):
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
        script = tmp_path / "run_browser_tinygrad_linear.mjs"
        script.write_text(
            f"""
import {{ loadMoltWasm }} from {browser_host_uri!r};

const baseUrl = {base_url!r};
const fakeState = {{ dispatchCount: 0 }};

const f32View = (bytes) => new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
const readF32 = (bytes, index) => f32View(bytes).getFloat32(index * 4, true);
const writeF32 = (bytes, index, value) => f32View(bytes).setFloat32(index * 4, value, true);
const readI32 = (bytes, index) => new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getInt32(index * 4, true);

const host = await loadMoltWasm({{
  manifestUrl: `${{baseUrl}}/manifest.json`,
  preferLinked: false,
  env: {{ MOLT_GPU_BACKEND: 'webgpu' }},
  gpuKernelDispatcher: {{
    dispatchKernel(request) {{
      fakeState.dispatchCount += 1;
      const bindings = new Map(request.bindings.map((binding) => [binding.name, binding]));
      const x = bindings.get('x').bytes;
      const weight = bindings.get('weight').bytes;
      const out = bindings.get('out').bytes;
      const outer = readI32(bindings.get('outer').bytes, 0);
      const inFeatures = readI32(bindings.get('in_features').bytes, 0);
      const outFeatures = readI32(bindings.get('out_features').bytes, 0);
      for (let row = 0; row < outer; row += 1) {{
        for (let col = 0; col < outFeatures; col += 1) {{
          let acc = 0.0;
          for (let k = 0; k < inFeatures; k += 1) {{
            acc += readF32(x, row * inFeatures + k) * readF32(weight, col * inFeatures + k);
          }}
          writeF32(out, row * outFeatures + col, acc);
        }}
      }}
    }},
  }},
}});
try {{ host.run(); }} finally {{ host.dispose(); }}
console.log(JSON.stringify(fakeState));
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
        lines = [line.strip() for line in run.stdout.splitlines() if line.strip()]
        assert lines[0] == "[[17.0, 23.0, 29.0], [39.0, 53.0, 67.0]]"
        assert json.loads(lines[1]) == {"dispatchCount": 1}
    finally:
        server.shutdown()


def test_browser_host_direct_mode_molt_nn_linear_uses_webgpu_dispatch(
    tmp_path: Path,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser host GPU direct-mode test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser host GPU direct-mode test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "browser_host_molt_nn_linear.py"
    src.write_text(
        "from molt.gpu.nn import Linear\n"
        "from molt.gpu.tensor import Tensor\n"
        "\n"
        "layer = Linear(2, 3, bias=False)\n"
        "layer.load_weights([[5.0, 6.0], [7.0, 8.0], [9.0, 10.0]])\n"
        "x = Tensor([[1.0, 2.0], [3.0, 4.0]])\n"
        "print(layer(x).to_list())\n",
        encoding="utf-8",
    )

    build_env = wasm_test_build_env(root, linked=False)
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
        timeout=900,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "app.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    manifest_path = tmp_path / "manifest.json"
    assert output_wasm.exists()
    assert runtime_wasm.exists()
    assert manifest_path.exists()

    class _WasmHandler(BaseHTTPRequestHandler):
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
        script = tmp_path / "run_browser_molt_nn_linear.mjs"
        script.write_text(
            f"""
import {{ loadMoltWasm }} from {browser_host_uri!r};

const baseUrl = {base_url!r};
const fakeState = {{ dispatchCount: 0 }};

const f32View = (bytes) => new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
const readF32 = (bytes, index) => f32View(bytes).getFloat32(index * 4, true);
const writeF32 = (bytes, index, value) => f32View(bytes).setFloat32(index * 4, value, true);
const readI32 = (bytes, index) => new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getInt32(index * 4, true);

const host = await loadMoltWasm({{
  manifestUrl: `${{baseUrl}}/manifest.json`,
  preferLinked: false,
  env: {{ MOLT_GPU_BACKEND: 'webgpu' }},
  gpuKernelDispatcher: {{
    dispatchKernel(request) {{
      fakeState.dispatchCount += 1;
      const bindings = new Map(request.bindings.map((binding) => [binding.name, binding]));
      const x = bindings.get('x').bytes;
      const weight = bindings.get('weight').bytes;
      const out = bindings.get('out').bytes;
      const outer = readI32(bindings.get('outer').bytes, 0);
      const inFeatures = readI32(bindings.get('in_features').bytes, 0);
      const outFeatures = readI32(bindings.get('out_features').bytes, 0);
      for (let row = 0; row < outer; row += 1) {{
        for (let col = 0; col < outFeatures; col += 1) {{
          let acc = 0.0;
          for (let k = 0; k < inFeatures; k += 1) {{
            acc += readF32(x, row * inFeatures + k) * readF32(weight, col * inFeatures + k);
          }}
          writeF32(out, row * outFeatures + col, acc);
        }}
      }}
    }},
  }},
}});
try {{ host.run(); }} finally {{ host.dispose(); }}
console.log(JSON.stringify(fakeState));
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
        lines = [line.strip() for line in run.stdout.splitlines() if line.strip()]
        assert lines[0] == "[[17.0, 23.0, 29.0], [39.0, 53.0, 67.0]]"
        assert json.loads(lines[1]) == {"dispatchCount": 1}
    finally:
        server.shutdown()


def test_browser_host_direct_mode_tensor_linear_without_webgpu_fails_fast(
    tmp_path: Path,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser host GPU direct-mode test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser host GPU direct-mode test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "browser_host_tensor_linear_no_gpu.py"
    src.write_text(
        "from molt.gpu.tensor import Tensor\n"
        "\n"
        "x = Tensor([[1.0, 2.0], [3.0, 4.0]])\n"
        "w = Tensor([[5.0, 6.0], [7.0, 8.0], [9.0, 10.0]])\n"
        "print(x.linear(w).to_list())\n",
        encoding="utf-8",
    )

    build_env = wasm_test_build_env(root, linked=False)
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
        timeout=900,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "app.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    manifest_path = tmp_path / "manifest.json"
    assert output_wasm.exists()
    assert runtime_wasm.exists()
    assert manifest_path.exists()

    class _WasmHandler(BaseHTTPRequestHandler):
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
        script = tmp_path / "run_browser_tensor_linear_no_gpu.mjs"
        script.write_text(
            f"""
import {{ loadMoltWasm }} from {browser_host_uri!r};

const baseUrl = {base_url!r};
const host = await loadMoltWasm({{
  manifestUrl: `${{baseUrl}}/manifest.json`,
  preferLinked: false,
  env: {{ MOLT_GPU_BACKEND: 'webgpu' }},
}});
try {{ host.run(); }} finally {{ host.dispose(); }}
""".lstrip(),
            encoding="utf-8",
        )
        run = _run_wasm_test_process(
            ["node", str(script)],
            cwd=root,
            capture_output=True,
            text=True,
            timeout=30,
        )
        assert run.returncode != 0
        assert (
            "browser webgpu dispatch is unavailable" in run.stderr
            or "navigator.gpu is unavailable in the browser WebGPU host" in run.stderr
        )
    finally:
        server.shutdown()


def test_browser_host_direct_mode_tensor_linear_split_last_dim_uses_webgpu_dispatch(
    tmp_path: Path,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser host GPU direct-mode test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser host GPU direct-mode test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "browser_host_tensor_linear_split.py"
    src.write_text(
        "from molt.gpu.tensor import Tensor\n"
        "\n"
        "x = Tensor([[1.0, 2.0], [3.0, 4.0]])\n"
        "w = Tensor([[5.0, 6.0], [7.0, 8.0], [9.0, 10.0]])\n"
        "left, right = x.linear_split_last_dim(w, (2, 1))\n"
        "print(left.to_list())\n"
        "print(right.to_list())\n",
        encoding="utf-8",
    )

    build_env = wasm_test_build_env(root, linked=False)
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
        timeout=900,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "app.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    manifest_path = tmp_path / "manifest.json"
    assert output_wasm.exists()
    assert runtime_wasm.exists()
    assert manifest_path.exists()

    class _WasmHandler(BaseHTTPRequestHandler):
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
        script = tmp_path / "run_browser_tensor_linear_split.mjs"
        script.write_text(
            f"""
import {{ loadMoltWasm }} from {browser_host_uri!r};

const baseUrl = {base_url!r};
const fakeState = {{ dispatchCount: 0 }};

const f32View = (bytes) => new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
const readF32 = (bytes, index) => f32View(bytes).getFloat32(index * 4, true);
const writeF32 = (bytes, index, value) => f32View(bytes).setFloat32(index * 4, value, true);
const readI32 = (bytes, index) => new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getInt32(index * 4, true);

const host = await loadMoltWasm({{
  manifestUrl: `${{baseUrl}}/manifest.json`,
  preferLinked: false,
  env: {{ MOLT_GPU_BACKEND: 'webgpu' }},
  gpuKernelDispatcher: {{
    dispatchKernel(request) {{
      fakeState.dispatchCount += 1;
      const bindings = new Map(request.bindings.map((binding) => [binding.name, binding]));
      const x = bindings.get('x').bytes;
      const weight = bindings.get('weight').bytes;
      const out = bindings.get('out').bytes;
      const outer = readI32(bindings.get('outer').bytes, 0);
      const inFeatures = readI32(bindings.get('in_features').bytes, 0);
      const outFeatures = readI32(bindings.get('out_features').bytes, 0);
      for (let row = 0; row < outer; row += 1) {{
        for (let col = 0; col < outFeatures; col += 1) {{
          let acc = 0.0;
          for (let k = 0; k < inFeatures; k += 1) {{
            acc += readF32(x, row * inFeatures + k) * readF32(weight, col * inFeatures + k);
          }}
          writeF32(out, row * outFeatures + col, acc);
        }}
      }}
    }},
  }},
}});
try {{ host.run(); }} finally {{ host.dispose(); }}
console.log(JSON.stringify(fakeState));
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
        lines = [line.strip() for line in run.stdout.splitlines() if line.strip()]
        assert lines[0] == "[[17.0, 23.0], [39.0, 53.0]]"
        assert lines[1] == "[[29.0], [67.0]]"
        assert json.loads(lines[2]) == {"dispatchCount": 1}
    finally:
        server.shutdown()


def test_browser_host_direct_mode_tensor_linear_squared_relu_gate_uses_webgpu_dispatch(
    tmp_path: Path,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser host GPU direct-mode test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser host GPU direct-mode test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "browser_host_tensor_gate.py"
    src.write_text(
        "from molt.gpu.tensor import Tensor\n"
        "\n"
        "x = Tensor([[1.0, 2.0], [3.0, 4.0]])\n"
        "w = Tensor([[1.0, 0.0], [0.0, 1.0], [1.0, 1.0], [2.0, 0.0]])\n"
        "print(x.linear_squared_relu_gate_interleaved(w).to_list())\n",
        encoding="utf-8",
    )

    build_env = wasm_test_build_env(root, linked=False)
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
        timeout=900,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "app.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    manifest_path = tmp_path / "manifest.json"
    assert output_wasm.exists()
    assert runtime_wasm.exists()
    assert manifest_path.exists()

    class _WasmHandler(BaseHTTPRequestHandler):
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
        script = tmp_path / "run_browser_tensor_gate.mjs"
        script.write_text(
            f"""
import {{ loadMoltWasm }} from {browser_host_uri!r};

const baseUrl = {base_url!r};
const fakeState = {{ dispatchCount: 0 }};

const f32View = (bytes) => new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
const readF32 = (bytes, index) => f32View(bytes).getFloat32(index * 4, true);
const writeF32 = (bytes, index, value) => f32View(bytes).setFloat32(index * 4, value, true);
const readI32 = (bytes, index) => new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getInt32(index * 4, true);

const host = await loadMoltWasm({{
  manifestUrl: `${{baseUrl}}/manifest.json`,
  preferLinked: false,
  env: {{ MOLT_GPU_BACKEND: 'webgpu' }},
  gpuKernelDispatcher: {{
    dispatchKernel(request) {{
      fakeState.dispatchCount += 1;
      const bindings = new Map(request.bindings.map((binding) => [binding.name, binding]));
      const x = bindings.get('x').bytes;
      const weight = bindings.get('weight').bytes;
      const out = bindings.get('out').bytes;
      const outer = readI32(bindings.get('outer').bytes, 0);
      const inFeatures = readI32(bindings.get('in_features').bytes, 0);
      const hidden = readI32(bindings.get('hidden').bytes, 0);
      for (let row = 0; row < outer; row += 1) {{
        for (let hiddenIdx = 0; hiddenIdx < hidden; hiddenIdx += 1) {{
          let gate = 0.0;
          let up = 0.0;
          for (let k = 0; k < inFeatures; k += 1) {{
            gate += readF32(x, row * inFeatures + k) * readF32(weight, (2 * hiddenIdx) * inFeatures + k);
            up += readF32(x, row * inFeatures + k) * readF32(weight, (2 * hiddenIdx + 1) * inFeatures + k);
          }}
          const relu = Math.max(gate, 0.0);
          writeF32(out, row * hidden + hiddenIdx, relu * relu * up);
        }}
      }}
    }},
  }},
}});
try {{ host.run(); }} finally {{ host.dispose(); }}
console.log(JSON.stringify(fakeState));
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
        lines = [line.strip() for line in run.stdout.splitlines() if line.strip()]
        assert lines[0] == "[[2.0, 18.0], [36.0, 294.0]]"
        assert json.loads(lines[1]) == {"dispatchCount": 1}
    finally:
        server.shutdown()


def test_browser_host_direct_mode_tensor_attention_uses_webgpu_dispatch(
    tmp_path: Path,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser host GPU direct-mode test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser host GPU direct-mode test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "browser_host_tensor_attention.py"
    src.write_text(
        "import array\n"
        "from molt.gpu import to_device\n"
        "from molt.gpu.tensor import Tensor, tensor_scaled_dot_product_attention\n"
        "\n"
        "q = Tensor(to_device(array.array('f', [1.0, 0.0, 0.0, 1.0])), shape=(1, 1, 2, 2))\n"
        "k = Tensor(to_device(array.array('f', [1.0, 0.0, 0.0, 1.0])), shape=(1, 1, 2, 2))\n"
        "v = Tensor(to_device(array.array('f', [10.0, 1.0, 2.0, 20.0])), shape=(1, 1, 2, 2))\n"
        "mask = Tensor(to_device(array.array('f', [0.0, -1.0e9, -1.0e9, 0.0])), shape=(1, 1, 2, 2))\n"
        "print(tensor_scaled_dot_product_attention(q, k, v, mask, 1.0).to_list())\n",
        encoding="utf-8",
    )

    build_env = wasm_test_build_env(root, linked=False)
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
        timeout=900,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "app.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    manifest_path = tmp_path / "manifest.json"
    assert output_wasm.exists()
    assert runtime_wasm.exists()
    assert manifest_path.exists()

    class _WasmHandler(BaseHTTPRequestHandler):
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
        script = tmp_path / "run_browser_tensor_attention.mjs"
        script.write_text(
            f"""
import {{ loadMoltWasm }} from {browser_host_uri!r};

const baseUrl = {base_url!r};
const fakeState = {{ dispatchCount: 0 }};

const view = (bytes) => new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
const readF32 = (bytes, index) => view(bytes).getFloat32(index * 4, true);
const writeF32 = (bytes, index, value) => view(bytes).setFloat32(index * 4, value, true);
const readI32 = (bytes, index) => view(bytes).getInt32(index * 4, true);

const host = await loadMoltWasm({{
  manifestUrl: `${{baseUrl}}/manifest.json`,
  preferLinked: false,
  env: {{ MOLT_GPU_BACKEND: 'webgpu' }},
  gpuKernelDispatcher: {{
    dispatchKernel(request) {{
      fakeState.dispatchCount += 1;
      const bindings = new Map(request.bindings.map((binding) => [binding.name, binding]));
      const q = bindings.get('q').bytes;
      const k = bindings.get('k').bytes;
      const v = bindings.get('v').bytes;
      const out = bindings.get('out').bytes;
      const mask = bindings.get('mask')?.bytes || null;
      const batch = readI32(bindings.get('batch').bytes, 0);
      const heads = readI32(bindings.get('heads').bytes, 0);
      const seqQ = readI32(bindings.get('seq_q').bytes, 0);
      const seqK = readI32(bindings.get('seq_k').bytes, 0);
      const dim = readI32(bindings.get('dim').bytes, 0);
      const valueDim = readI32(bindings.get('value_dim').bytes, 0);
      const scale = readF32(bindings.get('scale').bytes, 0);
      const hasMask = readI32(bindings.get('has_mask').bytes, 0) !== 0;
      const total = batch * heads * seqQ * valueDim;
      for (let idx = 0; idx < total; idx += 1) {{
        const d = idx % valueDim;
        const qIdx = Math.floor(idx / valueDim) % seqQ;
        const h = Math.floor(idx / (valueDim * seqQ)) % heads;
        const b = Math.floor(idx / (valueDim * seqQ * heads));
        const qBase = ((b * heads + h) * seqQ + qIdx) * dim;
        let maxScore = -Infinity;
        for (let kIdx = 0; kIdx < seqK; kIdx += 1) {{
          const kBase = ((b * heads + h) * seqK + kIdx) * dim;
          let score = 0.0;
          for (let i = 0; i < dim; i += 1) {{
            score += readF32(q, qBase + i) * readF32(k, kBase + i);
          }}
          score *= scale;
          if (hasMask) {{
            score += readF32(mask, ((b * heads + h) * seqQ + qIdx) * seqK + kIdx);
          }}
          if (score > maxScore) maxScore = score;
        }}
        let sum = 0.0;
        let acc = 0.0;
        for (let kIdx = 0; kIdx < seqK; kIdx += 1) {{
          const kBase = ((b * heads + h) * seqK + kIdx) * dim;
          let score = 0.0;
          for (let i = 0; i < dim; i += 1) {{
            score += readF32(q, qBase + i) * readF32(k, kBase + i);
          }}
          score *= scale;
          if (hasMask) {{
            score += readF32(mask, ((b * heads + h) * seqQ + qIdx) * seqK + kIdx);
          }}
          const weight = Math.exp(score - maxScore);
          sum += weight;
          const vBase = ((b * heads + h) * seqK + kIdx) * valueDim;
          acc += weight * readF32(v, vBase + d);
        }}
        writeF32(out, idx, sum !== 0.0 ? acc / sum : 0.0);
      }}
    }},
  }},
}});
try {{ host.run(); }} finally {{ host.dispose(); }}
console.log(JSON.stringify(fakeState));
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
        lines = [line.strip() for line in run.stdout.splitlines() if line.strip()]
        assert lines[0] == "[[[[10.0, 1.0], [2.0, 20.0]]]]"
        assert json.loads(lines[1]) == {"dispatchCount": 1}
    finally:
        server.shutdown()


def test_browser_host_direct_mode_turboquant_attention_uses_webgpu_dispatch(
    tmp_path: Path,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser host GPU direct-mode test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser host GPU direct-mode test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "browser_host_turboquant_attention.py"
    src.write_text(
        "from molt.gpu.kv_cache import TurboQuantAttentionKVCache\n"
        "from molt.gpu.tensor import Tensor\n"
        "from molt.gpu.turboquant import TurboQuantCodec\n"
        "\n"
        "codec = TurboQuantCodec(dim=2, bits=3, seed=5, qjl_seed=19)\n"
        "cache = TurboQuantAttentionKVCache(codec)\n"
        "cache.append(\n"
        "    Tensor([0.6, -0.2, 0.1, 0.4], shape=(1, 1, 2, 2)),\n"
        "    Tensor([0.2, 0.1, -0.3, 0.4], shape=(1, 1, 2, 2)),\n"
        ")\n"
        "q = Tensor([0.5, -0.1], shape=(1, 1, 1, 2))\n"
        "print(cache.attention(q, scale=1.0).to_list())\n",
        encoding="utf-8",
    )

    build_env = wasm_test_build_env(root, linked=False)
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
        timeout=900,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "app.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    manifest_path = tmp_path / "manifest.json"
    assert output_wasm.exists()
    assert runtime_wasm.exists()
    assert manifest_path.exists()

    class _WasmHandler(BaseHTTPRequestHandler):
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
        script = tmp_path / "run_browser_turboquant_attention.mjs"
        script.write_text(
            f"""
import {{ loadMoltWasm }} from {browser_host_uri!r};

const baseUrl = {base_url!r};
const fakeState = {{ dispatchCount: 0 }};
const view = (bytes) => new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
const readF32 = (bytes, index) => view(bytes).getFloat32(index * 4, true);
const writeF32 = (bytes, index, value) => view(bytes).setFloat32(index * 4, value, true);
const readI32 = (bytes, index) => view(bytes).getInt32(index * 4, true);

const host = await loadMoltWasm({{
  manifestUrl: `${{baseUrl}}/manifest.json`,
  preferLinked: false,
  env: {{ MOLT_GPU_BACKEND: 'webgpu' }},
      gpuKernelDispatcher: {{
        dispatchKernel(request) {{
          fakeState.dispatchCount += 1;
          const bindings = new Map(request.bindings.map((binding) => [binding.name, binding]));
          const queryPair = bindings.get('query_pair').bytes;
          const keyMse = bindings.get('key_mse').bytes;
          const keySign = bindings.get('key_sign').bytes;
          const keyScale = bindings.get('key_scale').bytes;
          const valueRows = bindings.get('value_rows').bytes;
          const out = bindings.get('out').bytes;
          const mask = bindings.get('mask')?.bytes || null;
          const params = bindings.get('params').bytes;
          const batch = readI32(params, 0);
          const queryHeads = readI32(params, 1);
          const kvHeads = readI32(params, 2);
          const seqQ = readI32(params, 3);
          const seqK = readI32(params, 4);
          const dim = readI32(params, 5);
          const scale = readF32(params, 6);
          const hasMask = readI32(params, 7) !== 0;
          const queryTotal = batch * queryHeads * seqQ * dim;
          const total = batch * queryHeads * seqQ * dim;
          for (let idx = 0; idx < total; idx += 1) {{
            const d = idx % dim;
            const qIdx = Math.floor(idx / dim) % seqQ;
            const h = Math.floor(idx / (dim * seqQ)) % queryHeads;
        const b = Math.floor(idx / (dim * seqQ * queryHeads));
        const kvH = queryHeads === kvHeads ? h : Math.floor(h / (queryHeads / kvHeads));
        const qBase = ((b * queryHeads + h) * seqQ + qIdx) * dim;
        let maxScore = -Infinity;
            for (let kIdx = 0; kIdx < seqK; kIdx += 1) {{
              const keyBase = ((b * kvHeads + kvH) * seqK + kIdx) * dim;
              let score = 0.0;
              let residual = 0.0;
              for (let i = 0; i < dim; i += 1) {{
                score += readF32(queryPair, qBase + i) * readF32(keyMse, keyBase + i);
                residual += readF32(queryPair, queryTotal + qBase + i) * readF32(keySign, keyBase + i);
              }}
              score = (score + residual * readF32(keyScale, ((b * kvHeads + kvH) * seqK + kIdx))) * scale;
              if (hasMask) {{
                score += readF32(mask, ((b * queryHeads + h) * seqQ + qIdx) * seqK + kIdx);
          }}
          if (score > maxScore) maxScore = score;
        }}
        let sum = 0.0;
        let acc = 0.0;
            for (let kIdx = 0; kIdx < seqK; kIdx += 1) {{
              const keyBase = ((b * kvHeads + kvH) * seqK + kIdx) * dim;
              let score = 0.0;
              let residual = 0.0;
              for (let i = 0; i < dim; i += 1) {{
                score += readF32(queryPair, qBase + i) * readF32(keyMse, keyBase + i);
                residual += readF32(queryPair, queryTotal + qBase + i) * readF32(keySign, keyBase + i);
              }}
              score = (score + residual * readF32(keyScale, ((b * kvHeads + kvH) * seqK + kIdx))) * scale;
              if (hasMask) {{
                score += readF32(mask, ((b * queryHeads + h) * seqQ + qIdx) * seqK + kIdx);
          }}
          const weight = Math.exp(score - maxScore);
          sum += weight;
          const vBase = ((b * kvHeads + kvH) * seqK + kIdx) * dim;
          acc += weight * readF32(valueRows, vBase + d);
        }}
        writeF32(out, idx, sum !== 0.0 ? acc / sum : 0.0);
      }}
    }},
  }},
}});
try {{ host.run(); }} finally {{ host.dispose(); }}
console.log(JSON.stringify(fakeState));
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
        lines = [line.strip() for line in run.stdout.splitlines() if line.strip()]
        values = json.loads(lines[0])
        assert values[0][0][0] == pytest.approx(
            [0.019662416654559325, 0.2214766675114854]
        )
        assert json.loads(lines[1]) == {"dispatchCount": 1}
    finally:
        server.shutdown()


@pytest.mark.parametrize(
    "failure",
    [
        "none",
        "pipeline-reject",
        "pipeline-validation",
        "allocation-memory",
        "upload-internal",
        "dispatch-validation",
        "readback-validation",
        "extent",
        "map-second",
        "loss",
        "uncaptured",
        "no-write-failure",
        "no-write-success",
        "no-bindings-success",
        "grid",
    ],
)
def test_browser_gpu_real_worker_completes_synchronous_import_atomically(
    tmp_path: Path,
    failure: str,
) -> None:
    node = shutil.which("node")
    if node is None:
        pytest.skip("node is required for the actual worker transport control")
    root = Path(__file__).resolve().parents[1]
    dispatch_uri = (root / "wasm/browser_gpu_dispatch.js").as_uri()
    bootstrap = tmp_path / "worker-bootstrap.mjs"
    bootstrap.write_text(
        """
import { parentPort, workerData } from 'node:worker_threads';
globalThis.addEventListener = (kind, callback) => {
  if (kind === 'message') parentPort.on('message', data => callback({ data }));
};
globalThis.GPUBufferUsage = { STORAGE: 1, COPY_DST: 2, COPY_SRC: 4, MAP_READ: 8 };
globalThis.GPUMapMode = { READ: 1 };
let source = '';
let bound = [];
let maps = 0;
let lose;
let uncaptured;
const scopes = [];
const record = (kind) => {
  const scope = [...scopes].reverse().find(scope => scope.kind === kind);
  if (!scope) throw new Error('missing error scope for ' + kind);
  scope.error = new Error('independent ' + source + ' failure');
};
const device = {
  limits: { maxComputeWorkgroupsPerDimension: 256, maxComputeWorkgroupSizeX: 256, maxComputeInvocationsPerWorkgroup: 256 },
  lost: new Promise(resolve => { lose = resolve; }),
  addEventListener(kind, callback) { if (kind === 'uncapturederror') uncaptured = callback; },
  pushErrorScope(kind) { scopes.push({kind, error: null}); },
  async popErrorScope() {
    if (!scopes.length) throw new Error('unbalanced device scope');
    return scopes.pop().error;
  },
  createShaderModule(spec) {
    source = spec.code;
    if (source === 'pipeline-validation') record('validation');
    return spec;
  },
  async createComputePipelineAsync() {
    if (source === 'pipeline-reject') throw new Error('independent pipeline rejection');
    return { getBindGroupLayout() { return {}; } };
  },
  createBuffer({size, usage}) {
    if (source === 'allocation-memory') record('out-of-memory');
    if (source === 'readback-validation' && (usage & 8)) record('validation');
    const bytes = new Uint8Array(size);
    return {
      bytes,
      async mapAsync() { maps += 1; if (source === 'map-second' && maps === 2) throw new Error('second readback failed'); },
      unmap() {}, destroy() {},
      getMappedRange() {
        return source === 'extent' && (usage & 8) ? bytes.buffer.slice(0, size - 1) : bytes.buffer;
      },
    };
  },
  createBindGroup({entries}) { bound = entries; return {}; },
  createCommandEncoder() {
    const copies = [];
    return {
      beginComputePass() { return { setPipeline() {}, setBindGroup() {}, dispatchWorkgroups() {
        if (source === 'dispatch-validation') record('validation');
      }, end() {} }; },
      copyBufferToBuffer(from, a, to, b, length) { copies.push(() => to.bytes.set(from.bytes.subarray(a, a + length), b)); },
      finish() { return () => {
        if (!source.startsWith('no-write') && source !== 'no-bindings-success') {
          bound[1].resource.buffer.bytes.set([42, 0, 0, 0]);
          bound[2].resource.buffer.bytes.set([1, 0, 0, 0]);
        }
        for (const copy of copies) copy();
      }; },
    };
  },
  queue: {
    writeBuffer(buffer, offset, bytes) {
      if (source === 'upload-internal') record('internal');
      buffer.bytes.set(bytes, offset);
    },
    submit(commands) { for (const command of commands) command(); },
    async onSubmittedWorkDone() {
      if (source === 'no-write-failure') throw new Error('queue completion failed without readbacks');
      if (source === 'loss') { lose({reason: 'unknown', message: 'independent device loss'}); await Promise.resolve(); }
      if (source === 'uncaptured') uncaptured({error: new Error('independent uncaptured error')});
    },
  },
};
Object.defineProperty(globalThis, 'navigator', { value: { gpu: {
  async requestAdapter() { return { async requestDevice() { return device; } }; },
} } });
await import(workerData.url);
""".lstrip(),
        encoding="utf-8",
    )
    script = tmp_path / "worker-control.mjs"
    script.write_text(
        """
import { Worker as NodeWorker } from 'node:worker_threads';
import { createBrowserGpuHost } from DISPATCH_URI;
// The host advertises WebGPU availability; the child bootstrap owns the device.
Object.defineProperty(globalThis, 'navigator', { value: { gpu: {} }, configurable: true });
let workersCreated = 0;
globalThis.Worker = class {
  constructor(url) {
    workersCreated += 1;
    this.worker = new NodeWorker(new URL(BOOTSTRAP_URI), { workerData: { url: url.href } });
  }
  addEventListener(name, callback) {
    this.worker.on(name, value => callback(name === 'message' ? { data: value } : value));
  }
  postMessage(value) { this.worker.postMessage(value); }
  terminate() { return this.worker.terminate(); }
};
const memory = new WebAssembly.Memory({initial: 1});
const bytes = new Uint8Array(memory.buffer);
const put = (offset, text) => { const encoded = new TextEncoder().encode(text); bytes.set(encoded, offset); return encoded.length; };
bytes.set([10, 0, 0, 0], 512);
bytes.set([9, 0, 0, 0], 520);
bytes.set([0, 0, 0, 0], 528);
const selectedFailure = FAILURE;
const sourceLength = put(8, selectedFailure);
const entryLength = put(64, 'kernel');
const recordLength = put(1024, JSON.stringify({bindings: selectedFailure === 'no-bindings-success' ? [] : [
  {binding: 0, name: 'input', access: 'read', ptr: 512, len: 4},
  {binding: 1, name: 'output', access: selectedFailure.startsWith('no-write') ? 'read' : 'read_write', ptr: 520, len: 4},
  {binding: 2, name: 'store_occurrence', access: selectedFailure.startsWith('no-write') ? 'read' : 'read_write', ptr: 528, len: 4},
]}));
const host = createBrowserGpuHost({memory}, {gpuKernelTimeoutMs: 5000});
let result;
try { result = host.gpuWebGpuDispatchHost(8, sourceLength, 64, entryLength, 1024, recordLength, selectedFailure === 'grid' ? 257 : 1, 1, 4096, 512, 4080); }
finally { host.dispose(); }
const errorLength = new DataView(memory.buffer).getUint32(4080, true);
const detail = new TextDecoder().decode(bytes.subarray(4096, 4096 + errorLength));
console.log(JSON.stringify({result, workersCreated, detail, input: [...bytes.slice(512, 516)], output: [...bytes.slice(520, 524)], flag: [...bytes.slice(528, 532)]}));
""".replace("DISPATCH_URI", json.dumps(dispatch_uri))
        .replace("BOOTSTRAP_URI", json.dumps(bootstrap.as_uri()))
        .replace("FAILURE", json.dumps(failure)),
        encoding="utf-8",
    )
    completed = _run_wasm_test_process(
        [node, str(script)],
        cwd=root,
        env=os.environ.copy(),
        capture_output=True,
        text=True,
        timeout=30,
    )
    assert completed.returncode == 0, completed.stderr
    observed = json.loads(completed.stdout.strip())
    assert observed["workersCreated"] == 1
    assert observed["input"] == [10, 0, 0, 0]
    if failure == "none":
        assert observed == {
            "result": 0,
            "workersCreated": 1,
            "detail": "",
            "input": [10, 0, 0, 0],
            "output": [42, 0, 0, 0],
            "flag": [1, 0, 0, 0],
        }
    else:
        assert observed["result"] == (
            0 if failure in {"no-write-success", "no-bindings-success"} else -22
        )
        # Nonthrowing validation/loss/queue errors must not become success or timeout.
        assert observed["output"] == [9, 0, 0, 0]
        assert observed["flag"] == [0, 0, 0, 0]
        if failure in {"no-write-success", "no-bindings-success"}:
            assert observed["detail"] == ""
        else:
            # An unrelated EINVAL must not stand in for the injected failure.
            expected_detail = {
                "pipeline-reject": "independent pipeline rejection",
                "pipeline-validation": "independent pipeline-validation failure",
                "allocation-memory": "independent allocation-memory failure",
                "upload-internal": "independent upload-internal failure",
                "dispatch-validation": "independent dispatch-validation failure",
                "readback-validation": "independent readback-validation failure",
                "extent": "readback differs from physical dispatch plan",
                "map-second": "second readback failed",
                "loss": "independent device loss",
                "uncaptured": "independent uncaptured error",
                "no-write-failure": "queue completion failed without readbacks",
                "grid": "grid exceeds device dispatch capability",
            }[failure]
            assert expected_detail in observed["detail"]
