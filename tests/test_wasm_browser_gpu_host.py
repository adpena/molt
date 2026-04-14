from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest


def test_browser_host_direct_mode_compiled_gpu_kernel_uses_webgpu_dispatch(
    tmp_path: Path,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser host GPU direct-mode test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser host GPU direct-mode test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "browser_host_gpu.py"
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

    build_env = os.environ.copy()
    build_env["PYTHONPATH"] = str(root / "src")
    build_env["MOLT_WASM_LINKED"] = "0"
    build_env["MOLT_SESSION_ID"] = "test-browser-turboquant-webgpu"
    build_env["CARGO_TARGET_DIR"] = str(root / "target" / "test-browser-turboquant-webgpu")
    build_env["MOLT_DIFF_CARGO_TARGET_DIR"] = build_env["CARGO_TARGET_DIR"]
    build_env["MOLT_CACHE"] = str(root / ".molt_cache")
    build_env["MOLT_DIFF_ROOT"] = str(root / "tmp" / "diff")
    build_env["MOLT_DIFF_TMPDIR"] = str(root / "tmp")
    build_env["UV_CACHE_DIR"] = str(root / ".uv-cache")
    build_env["TMPDIR"] = str(root / "tmp")
    build_env["MOLT_BACKEND_DAEMON"] = "0"
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
        timeout=900,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "output.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    assert output_wasm.exists()
    assert runtime_wasm.exists()

    class _WasmHandler(BaseHTTPRequestHandler):
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

const readF32 = (bytes, index) => new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getFloat32(index * 4, true);
const writeF32 = (bytes, index, value) => new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).setFloat32(index * 4, value, true);
const readI32 = (bytes, index) => new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getInt32(index * 4, true);

const host = await loadMoltWasm({{
  wasmUrl: `${{baseUrl}}/output.wasm`,
  runtimeUrl: `${{baseUrl}}/molt_runtime.wasm`,
  preferLinked: false,
  env: {{ MOLT_GPU_BACKEND: 'webgpu' }},
  gpuKernelDispatcher: {{
    dispatchKernel(request) {{
      fakeState.dispatchCount += 1;
      fakeState.shaderCount += 1;
      const a = request.bindings.find((binding) => binding.binding === 0).bytes;
      const b = request.bindings.find((binding) => binding.binding === 1).bytes;
      const c = request.bindings.find((binding) => binding.binding === 2).bytes;
      const n = readI32(request.bindings.find((binding) => binding.binding === 3).bytes, 0);
      const workgroupSizeMatch = request.source.match(/@workgroup_size\\((\\d+)\\)/);
      const workgroupSize = workgroupSizeMatch ? Number(workgroupSizeMatch[1]) : 1;
      const totalThreads = Number(request.grid) * workgroupSize;
      for (let tid = 0; tid < totalThreads && tid < n; tid += 1) {{
        writeF32(c, tid, readF32(a, tid) + readF32(b, tid));
      }}
    }},
  }},
}});
host.run();
console.log(JSON.stringify(fakeState));
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
        assert lines[0] == "[11.0, 22.0, 33.0, 44.0]"
        assert json.loads(lines[1]) == {"dispatchCount": 1, "shaderCount": 1}
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
        timeout=900,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "output.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    assert output_wasm.exists()
    assert runtime_wasm.exists()

    class _WasmHandler(BaseHTTPRequestHandler):
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
  wasmUrl: `${{baseUrl}}/output.wasm`,
  runtimeUrl: `${{baseUrl}}/molt_runtime.wasm`,
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
host.run();
console.log(JSON.stringify(fakeState));
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
        timeout=900,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "output.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    assert output_wasm.exists()
    assert runtime_wasm.exists()

    class _WasmHandler(BaseHTTPRequestHandler):
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
  wasmUrl: `${{baseUrl}}/output.wasm`,
  runtimeUrl: `${{baseUrl}}/molt_runtime.wasm`,
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
host.run();
console.log(JSON.stringify(fakeState));
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
        timeout=900,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "output.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    assert output_wasm.exists()
    assert runtime_wasm.exists()

    class _WasmHandler(BaseHTTPRequestHandler):
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
  wasmUrl: `${{baseUrl}}/output.wasm`,
  runtimeUrl: `${{baseUrl}}/molt_runtime.wasm`,
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
host.run();
console.log(JSON.stringify(fakeState));
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
        timeout=900,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "output.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    assert output_wasm.exists()
    assert runtime_wasm.exists()

    class _WasmHandler(BaseHTTPRequestHandler):
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
  wasmUrl: `${{baseUrl}}/output.wasm`,
  runtimeUrl: `${{baseUrl}}/molt_runtime.wasm`,
  preferLinked: false,
  env: {{ MOLT_GPU_BACKEND: 'webgpu' }},
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
        timeout=900,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "output.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    assert output_wasm.exists()
    assert runtime_wasm.exists()

    class _WasmHandler(BaseHTTPRequestHandler):
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
  wasmUrl: `${{baseUrl}}/output.wasm`,
  runtimeUrl: `${{baseUrl}}/molt_runtime.wasm`,
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
host.run();
console.log(JSON.stringify(fakeState));
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
        timeout=900,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "output.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    assert output_wasm.exists()
    assert runtime_wasm.exists()

    class _WasmHandler(BaseHTTPRequestHandler):
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
  wasmUrl: `${{baseUrl}}/output.wasm`,
  runtimeUrl: `${{baseUrl}}/molt_runtime.wasm`,
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
host.run();
console.log(JSON.stringify(fakeState));
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
        timeout=900,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "output.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    assert output_wasm.exists()
    assert runtime_wasm.exists()

    class _WasmHandler(BaseHTTPRequestHandler):
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
  wasmUrl: `${{baseUrl}}/output.wasm`,
  runtimeUrl: `${{baseUrl}}/molt_runtime.wasm`,
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
host.run();
console.log(JSON.stringify(fakeState));
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
        timeout=900,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "output.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    assert output_wasm.exists()
    assert runtime_wasm.exists()

    class _WasmHandler(BaseHTTPRequestHandler):
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
  wasmUrl: `${{baseUrl}}/output.wasm`,
  runtimeUrl: `${{baseUrl}}/molt_runtime.wasm`,
  preferLinked: false,
  env: {{ MOLT_GPU_BACKEND: 'webgpu' }},
  gpuKernelDispatcher: {{
    dispatchKernel(request) {{
      fakeState.dispatchCount += 1;
      const bindings = new Map(request.bindings.map((binding) => [binding.name, binding]));
      const rotatedQ = bindings.get('rotated_q').bytes;
      const querySketch = bindings.get('query_sketch').bytes;
      const keyMse = bindings.get('key_mse').bytes;
      const keySign = bindings.get('key_sign').bytes;
      const keyScale = bindings.get('key_scale').bytes;
      const valueRows = bindings.get('value_rows').bytes;
      const out = bindings.get('out').bytes;
      const mask = bindings.get('mask')?.bytes || null;
      const batch = readI32(bindings.get('batch').bytes, 0);
      const queryHeads = readI32(bindings.get('query_heads').bytes, 0);
      const kvHeads = readI32(bindings.get('kv_heads').bytes, 0);
      const seqQ = readI32(bindings.get('seq_q').bytes, 0);
      const seqK = readI32(bindings.get('seq_k').bytes, 0);
      const dim = readI32(bindings.get('dim').bytes, 0);
      const scale = readF32(bindings.get('scale').bytes, 0);
      const hasMask = readI32(bindings.get('has_mask').bytes, 0) !== 0;
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
            score += readF32(rotatedQ, qBase + i) * readF32(keyMse, keyBase + i);
            residual += readF32(querySketch, qBase + i) * readF32(keySign, keyBase + i);
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
            score += readF32(rotatedQ, qBase + i) * readF32(keyMse, keyBase + i);
            residual += readF32(querySketch, qBase + i) * readF32(keySign, keyBase + i);
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
host.run();
console.log(JSON.stringify(fakeState));
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
        values = json.loads(lines[0])
        assert values[0][0][0] == pytest.approx([0.019662416654559325, 0.2214766675114854])
        assert json.loads(lines[1]) == {"dispatchCount": 1}
    finally:
        server.shutdown()
