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
