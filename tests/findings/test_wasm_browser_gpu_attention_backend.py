"""HF-866: a browser program runs tensor attention on WebGPU without env.read.

Oracle: the masked attention of these 2x2 heads is the identity on V, so the
program prints [[[[10.0, 1.0], [2.0, 20.0]]]] after exactly one WebGPU
dispatch, which the host's fake dispatcher computes with the reference
softmax. The sibling tensor-linear programs dispatch to WebGPU under the same
default browser capability policy.
"""

from __future__ import annotations

import json
import shutil
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest

from tests.findings import finding
from tests.wasm_linked_runner import _run_wasm_test_process, wasm_test_build_env


@finding("HF-866")
def test_browser_host_direct_mode_tensor_attention_uses_webgpu_dispatch(
    tmp_path: Path,
) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser host GPU direct-mode test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for browser host GPU direct-mode test")

    root = Path(__file__).resolve().parents[2]
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
        role="tool",
        cwd=root,
        env=build_env,
        capture_output=True,
        text=True,
        timeout=900,
    )
    if build.returncode != 0:
        raise RuntimeError(f"attention program build failed:\n{build.stderr[-2000:]}")

    output_wasm = tmp_path / "app.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    manifest_path = tmp_path / "manifest.json"
    assert output_wasm.exists()
    assert runtime_wasm.exists()
    assert manifest_path.exists()

    class _WasmHandler(BaseHTTPRequestHandler):
        def log_message(self, format: str, *args: object) -> None:  # noqa: A002
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
            role="guest-host",
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
