from __future__ import annotations

import json
import shutil
from pathlib import Path

import pytest

from tests.native_process_guard import run_native_test_process

ROOT = Path(__file__).resolve().parents[1]


def _run_compute_engine_script(script: str) -> dict[str, object]:
    if shutil.which("node") is None:
        pytest.skip("node is required for browser compute-engine tests")
    run = run_native_test_process(
        ["node", "--input-type=module", "-e", script],
        cwd=str(ROOT),
        check=True,
        capture_output=True,
        text=True,
        timeout=10,
    )
    return json.loads(run.stdout)


def test_compute_engine_rejects_unknown_forced_backend() -> None:
    result = _run_compute_engine_script(
        """
        const { ComputeEngine } = await import('./deploy/browser/compute-engine.js');
        try {
          const engine = await ComputeEngine.create({ forceBackend: 'bogus-backend' });
          process.stdout.write(JSON.stringify({
            ok: true,
            backend: engine.backendName,
          }));
        } catch (err) {
          process.stdout.write(JSON.stringify({
            ok: false,
            message: err.message,
          }));
        }
        """
    )

    assert result["ok"] is False
    assert "bogus-backend" in result["message"]
    assert "requested" in result["message"]


def test_simd_probe_accepts_a_valid_simd_module() -> None:
    result = _run_compute_engine_script("""
        const { WasmSimdEngine } = await import('./deploy/browser/compute-engine.js');
        process.stdout.write(JSON.stringify({ available: await WasmSimdEngine.probe() }));
    """)
    assert result == {"available": True}


def test_forced_simd_reports_all_missing_capabilities_without_retry() -> None:
    result = _run_compute_engine_script("""
        const { ComputeEngine, WasmSimdEngine } = await import('./deploy/browser/compute-engine.js');
        const urls = [];
        globalThis.fetch = async url => {
            urls.push(url);
            return { ok: true, arrayBuffer: async () => new Uint8Array([0,97,115,109,1,0,0,0]).buffer };
        };
        let message;
        try { await ComputeEngine.create({forceBackend:'wasm-simd', wasmUrl:'test://missing'}); }
        catch (error) { message = error.message; }
        process.stdout.write(JSON.stringify({ message, urls, required: WasmSimdEngine.requiredExports() }));
    """)
    assert result["urls"] == ["test://missing"]
    for name in [*result["required"], "memory", "__heap_base"]:
        assert name in result["message"]


def test_forced_simd_selects_a_module_with_every_capability() -> None:
    result = _run_compute_engine_script("""
        const { ComputeEngine, WasmSimdEngine } = await import('./deploy/browser/compute-engine.js');
        const memory = new WebAssembly.Memory({ initial: 1 });
        const exports = {
            memory,
            __heap_base: new WebAssembly.Global({ value: 'i32', mutable: false }, 1024),
        };
        for (const name of WasmSimdEngine.requiredExports()) exports[name] = () => {};
        const urls = [];
        globalThis.fetch = async url => {
            urls.push(url);
            return { ok: true, arrayBuffer: async () => new Uint8Array([0,97,115,109,1,0,0,0]).buffer };
        };
        WebAssembly.instantiate = async () => ({ instance: { exports } });
        const engine = await ComputeEngine.create({forceBackend:'wasm-simd', wasmUrl:'test://complete'});
        process.stdout.write(JSON.stringify({ backend: engine.backendName, urls }));
    """)
    assert result == {"backend": "wasm-simd", "urls": ["test://complete"]}


def test_automatic_backend_selection_has_one_scalar_fallback_owner() -> None:
    result = _run_compute_engine_script("""
        const { ComputeEngine } = await import('./deploy/browser/compute-engine.js');
        let requests = 0;
        globalThis.fetch = async () => { requests++; return {ok:false, status:404}; };
        const engine = await ComputeEngine.create();
        const sum = await engine.add(new Float32Array([1,2]), new Float32Array([3,4]), 2);
        process.stdout.write(JSON.stringify({backend: engine.backendName, requests, sum: [...sum]}));
    """)
    assert result == {"backend": "scalar", "requests": 1, "sum": [4, 6]}


def test_empty_rows_return_before_any_backend_dispatch() -> None:
    result = _run_compute_engine_script("""
        const { ScalarEngine, WebGPUEngine, WebGL2Engine } = await import('./deploy/browser/compute-engine.js');
        // Delegate tripwire: a zero-sized result must not require any GPU call.
        const unavailable = new Proxy({}, {get() { throw new Error('empty result dispatched'); }});
        const engines = [new ScalarEngine(), new WebGPUEngine(unavailable), new WebGL2Engine(unavailable)];
        const empty = new Float32Array(), lengths = [];
        for (const engine of engines) {
            lengths.push((await engine.softmax(empty, 0, 0xffffffff)).length);
            lengths.push((await engine.rmsNorm(empty, empty, 0, 0xffffffff)).length);
        }
        process.stdout.write(JSON.stringify({lengths}));
    """)
    assert result == {"lengths": [0, 0, 0, 0, 0, 0]}
