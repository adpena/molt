from __future__ import annotations

import importlib.util
import json
import shutil
import subprocess
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DRIVER_DIR = ROOT / "drivers" / "falcon" / "browser_webgpu"
DEPLOY_PY = DRIVER_DIR / "deploy.py"
BENCH_PY = DRIVER_DIR / "bench_hostfed.py"
BROWSER_JS = DRIVER_DIR / "browser.js"
VERIFY_PY = DRIVER_DIR / "verify.py"


def _load_module(path: Path, name: str):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_falcon_driver_deploy_surface_is_target_root_driven(tmp_path: Path) -> None:
    target_root = tmp_path / "falcon-target"
    artifact_dir = target_root / "dist" / "browser_split"
    artifact_dir.mkdir(parents=True)
    (artifact_dir / "app.wasm").write_bytes(b"\0asm\x01\x00\x00\x00")
    (artifact_dir / "molt_runtime.wasm").write_bytes(b"\0asm\x01\x00\x00\x00")
    (target_root / "config.json").write_text('{"dim":2}\n', encoding="utf-8")
    weights_dir = target_root / "weights"
    weights_dir.mkdir()
    (weights_dir / "layer0.safetensors").write_bytes(b"weights")

    deploy = _load_module(DEPLOY_PY, "falcon_driver_deploy")
    surface = deploy.build_deploy_surface(
        config_path=DRIVER_DIR / "wrangler.jsonc",
        target_root=target_root,
    )

    assert surface["target"] == "falcon.browser_webgpu"
    assert surface["target_root"] == str(target_root)
    assert surface["artifacts"]["app_wasm"] == str(artifact_dir / "app.wasm")
    assert surface["artifacts"]["runtime_wasm"] == str(artifact_dir / "molt_runtime.wasm")
    assert surface["artifacts"]["config_json"] == str(target_root / "config.json")
    assert surface["status"] == "manifest_ready"
    immutable = surface["artifact_manifest"]["immutable"]
    assert immutable["app_wasm"]["sha256"]
    assert immutable["runtime_wasm"]["sha256"]
    assert immutable["config_json"]["sha256"]
    assert immutable["browser_loader"]["relative_path"] == "browser.js"
    assert immutable["worker_entrypoint"]["relative_path"] == "worker.ts"
    assert immutable["wrangler_config"]["relative_path"] == "wrangler.jsonc"
    assert surface["artifact_manifest"]["weights"][0]["relative_path"] == "layer0.safetensors"


def test_falcon_driver_deploy_script_emits_json(tmp_path: Path) -> None:
    target_root = tmp_path / "falcon-target"
    artifact_dir = target_root / "dist" / "browser_split"
    artifact_dir.mkdir(parents=True)
    (artifact_dir / "app.wasm").write_bytes(b"\0asm\x01\x00\x00\x00")
    (artifact_dir / "molt_runtime.wasm").write_bytes(b"\0asm\x01\x00\x00\x00")
    (target_root / "config.json").write_text('{"dim":2}\n', encoding="utf-8")
    weights_dir = target_root / "weights"
    weights_dir.mkdir()
    (weights_dir / "layer0.safetensors").write_bytes(b"weights")

    res = subprocess.run(
        [sys.executable, str(DEPLOY_PY), "--target-root", str(target_root)],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    assert res.returncode == 0, res.stderr
    payload = json.loads(res.stdout)
    assert payload["target"] == "falcon.browser_webgpu"
    assert payload["target_root"] == str(target_root)
    assert payload["artifact_manifest"]["immutable"]["app_wasm"]["sha256"]


def test_falcon_driver_materialize_bundle_emits_manifest_and_assets(
    tmp_path: Path,
) -> None:
    target_root = tmp_path / "falcon-target"
    artifact_dir = target_root / "dist" / "browser_split"
    artifact_dir.mkdir(parents=True)
    (artifact_dir / "app.wasm").write_bytes(b"\0asm\x01\x00\x00\x00")
    (artifact_dir / "molt_runtime.wasm").write_bytes(b"\0asm\x01\x00\x00\x00")
    (target_root / "config.json").write_text('{"dim":2}\n', encoding="utf-8")
    weights_dir = target_root / "weights"
    weights_dir.mkdir()
    (weights_dir / "layer0.safetensors").write_bytes(b"weights")

    deploy = _load_module(DEPLOY_PY, "falcon_driver_deploy_materialize")
    bundle = deploy.materialize_deploy_bundle(
        config_path=DRIVER_DIR / "wrangler.jsonc",
        target_root=target_root,
        weights_base_url="https://weights.example.invalid/falcon",
    )

    bundle_root = Path(bundle["bundle_root"])
    assert (bundle_root / "assets" / "app.wasm").exists()
    assert (bundle_root / "assets" / "molt_runtime.wasm").exists()
    assert (bundle_root / "assets" / "browser.js").exists()
    assert (bundle_root / "assets" / "browser_host.js").exists()
    assert (bundle_root / "assets" / "molt_vfs_browser.js").exists()
    assert (bundle_root / "assets" / "config.json").exists()
    materialized_browser_js = (bundle_root / "assets" / "browser.js").read_text(encoding="utf-8")
    assert 'import { loadMoltWasm } from "./browser_host.js";' in materialized_browser_js
    manifest = json.loads((bundle_root / "assets" / "driver-manifest.base.json").read_text(encoding="utf-8"))
    assert manifest["target"] == "falcon.browser_webgpu"
    assert manifest["artifacts"]["app_wasm"]["url"] == "/app.wasm"
    assert manifest["artifacts"]["runtime_wasm"]["url"] == "/molt_runtime.wasm"
    assert manifest["artifacts"]["config_json"]["url"] == "/config.json"
    assert manifest["weights"]["base_url"] == "https://weights.example.invalid/falcon"
    assert manifest["weights"]["files"][0]["path"] == "layer0.safetensors"
    wrangler = json.loads((bundle_root / "wrangler.jsonc").read_text(encoding="utf-8"))
    assert wrangler["main"] == "./drivers/falcon/browser_webgpu/worker.ts"
    assert wrangler["assets"]["directory"] == "./assets"
    assert wrangler["assets"]["binding"] == "ASSETS"
    assert wrangler["assets"]["run_worker_first"] == ["/driver-manifest.json"]
    if shutil.which("wrangler") is not None:
        check = subprocess.run(
            ["wrangler", "check", "--config", str(bundle_root / "wrangler.jsonc")],
            cwd=bundle_root,
            text=True,
            capture_output=True,
            check=False,
        )
        assert check.returncode == 0, check.stdout + check.stderr


def test_falcon_driver_materialize_bundle_requires_weights_base_url(
    tmp_path: Path,
) -> None:
    target_root = tmp_path / "falcon-target"
    artifact_dir = target_root / "dist" / "browser_split"
    artifact_dir.mkdir(parents=True)
    (artifact_dir / "app.wasm").write_bytes(b"\0asm\x01\x00\x00\x00")
    (artifact_dir / "molt_runtime.wasm").write_bytes(b"\0asm\x01\x00\x00\x00")
    (target_root / "config.json").write_text('{"dim":2}\n', encoding="utf-8")
    weights_dir = target_root / "weights"
    weights_dir.mkdir()
    (weights_dir / "layer0.safetensors").write_bytes(b"weights")

    deploy = _load_module(DEPLOY_PY, "falcon_driver_deploy_require_base_url")
    try:
        deploy.materialize_deploy_bundle(
            config_path=DRIVER_DIR / "wrangler.jsonc",
            target_root=target_root,
            weights_base_url=None,
        )
    except ValueError as exc:
        assert "weights_base_url" in str(exc)
    else:
        raise AssertionError("expected materialize_deploy_bundle to reject missing weights_base_url")


def test_falcon_driver_bench_script_help() -> None:
    res = subprocess.run(
        [sys.executable, str(BENCH_PY), "--help"],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    assert res.returncode == 0, res.stderr
    assert "--target-root" in res.stdout


def test_falcon_driver_verify_wrapper_emits_json(tmp_path: Path) -> None:
    target_root = tmp_path / "falcon-target"
    artifact_dir = target_root / "dist" / "browser_split"
    artifact_dir.mkdir(parents=True)
    (artifact_dir / "app.wasm").write_bytes(b"\0asm\x01\x00\x00\x00")
    (artifact_dir / "molt_runtime.wasm").write_bytes(b"\0asm\x01\x00\x00\x00")
    (target_root / "config.json").write_text('{"dim":2}\n', encoding="utf-8")
    weights_dir = target_root / "weights"
    weights_dir.mkdir()
    (weights_dir / "layer0.safetensors").write_bytes(b"weights")

    res = subprocess.run(
        [
            sys.executable,
            str(VERIFY_PY),
            "--target-root",
            str(target_root),
            "--weights-base-url",
            "https://weights.example.invalid/falcon",
            "--wrangler",
            "true",
        ],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    assert res.returncode == 0, res.stderr
    payload = json.loads(res.stdout)
    assert payload["target"] == "falcon.browser_webgpu"
    assert payload["bundle"]["bundle_root"]
    assert payload["wrangler_check"]["returncode"] == 0
    assert payload["wrangler_dry_run"]["returncode"] == 0


def test_falcon_browser_driver_init_and_ocr_tokens_roundtrip(tmp_path: Path) -> None:
    src = tmp_path / "main_molt.py"
    src.write_text(
        "_initialized = 0\n"
        "def init(weights_bytes: bytes, config_json: str):\n"
        "    global _initialized\n"
        "    print(len(weights_bytes))\n"
        "    print(config_json)\n"
        "    _initialized = 1\n"
        "def ocr_tokens(width: int, height: int, rgb: bytes, prompt_ids: list[int], max_new_tokens: int):\n"
        "    print(width)\n"
        "    print(height)\n"
        "    print(len(rgb))\n"
        "    print(prompt_ids)\n"
        "    print(max_new_tokens)\n"
        "    if not _initialized:\n"
        "        raise RuntimeError('not initialized')\n"
        "    return prompt_ids\n",
        encoding="utf-8",
    )
    build_env = {
        **dict(__import__('os').environ),
        "PYTHONPATH": str(ROOT / "src"),
        "MOLT_WASM_LINKED": "0",
    }
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
        cwd=ROOT,
        env=build_env,
        text=True,
        capture_output=True,
        check=False,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "output.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    weights_bin = tmp_path / "weights.bin"
    weights_bin.write_bytes(b"weights")
    config_json = tmp_path / "config.json"
    config_json.write_text('{"dim":2}\n', encoding="utf-8")

    class _ArtifactHandler(BaseHTTPRequestHandler):
        def log_message(self, fmt: str, *args: object) -> None:
            return None

        def do_GET(self) -> None:  # noqa: N802
            mapping = {
                "/output.wasm": output_wasm,
                "/molt_runtime.wasm": runtime_wasm,
                "/weights.bin": weights_bin,
                "/config.json": config_json,
            }
            target = mapping.get(self.path)
            if target is None:
                self.send_response(404)
                self.end_headers()
                return
            payload = target.read_bytes()
            ctype = "application/wasm" if target.suffix == ".wasm" else "application/octet-stream"
            if target.suffix == ".json":
                ctype = "application/json"
            self.send_response(200)
            self.send_header("content-type", ctype)
            self.send_header("content-length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

    server = ThreadingHTTPServer(("127.0.0.1", 0), _ArtifactHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        base_url = f"http://127.0.0.1:{server.server_address[1]}"
        script = tmp_path / "driver_roundtrip.mjs"
        script.write_text(
            f"""
import {{ initFalconBrowserWebGpu }} from {BROWSER_JS.as_uri()!r};
const session = await initFalconBrowserWebGpu({{
  wasmUrl: {f"{base_url}/output.wasm"!r},
  runtimeUrl: {f"{base_url}/molt_runtime.wasm"!r},
  weightsUrl: {f"{base_url}/weights.bin"!r},
  configUrl: {f"{base_url}/config.json"!r},
}});
const tokens = await session.ocrTokens({{
  width: 32,
  height: 16,
  rgb: new Uint8Array([1,2,3,4,5,6]),
  promptIds: [257, 258],
  maxNewTokens: 3,
  exportName: 'main_molt__ocr_tokens',
}});
console.log(JSON.stringify(tokens));
""".lstrip(),
            encoding="utf-8",
        )
        run = subprocess.run(
            ["node", str(script)],
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=False,
        )
        assert run.returncode == 0, run.stderr
        lines = [line.strip() for line in run.stdout.splitlines() if line.strip()]
        assert lines[:7] == ["7", '{"dim":2}', "32", "16", "6", "[257, 258]", "3"]
        assert json.loads(lines[7]) == [257, 258]
    finally:
        server.shutdown()


def test_falcon_browser_driver_init_from_manifest_roundtrip(tmp_path: Path) -> None:
    src = tmp_path / "main_molt.py"
    src.write_text(
        "_initialized = 0\n"
        "def init(weights_bytes: bytes, config_json: str):\n"
        "    global _initialized\n"
        "    _initialized = 1\n"
        "def ocr_tokens(width: int, height: int, rgb: bytes, prompt_ids: list[int], max_new_tokens: int):\n"
        "    if not _initialized:\n"
        "        raise RuntimeError('not initialized')\n"
        "    return prompt_ids\n",
        encoding="utf-8",
    )
    build_env = {
        **dict(__import__("os").environ),
        "PYTHONPATH": str(ROOT / "src"),
        "MOLT_WASM_LINKED": "0",
    }
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
        cwd=ROOT,
        env=build_env,
        text=True,
        capture_output=True,
        check=False,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "output.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    weights_bin = tmp_path / "weights.bin"
    weights_bin.write_bytes(b"weights")
    config_json = tmp_path / "config.json"
    config_json.write_text('{"dim":2}\n', encoding="utf-8")
    manifest_json = tmp_path / "driver-manifest.base.json"
    manifest_json.write_text(
        json.dumps(
            {
                "target": "falcon.browser_webgpu",
                "artifacts": {
                    "app_wasm": {"url": "/output.wasm"},
                    "runtime_wasm": {"url": "/molt_runtime.wasm"},
                    "config_json": {"url": "/config.json"},
                },
                "weights": {
                    "base_url": None,
                    "files": [{"path": "weights.bin", "url": "/weights.bin"}],
                },
            }
        )
        + "\n",
        encoding="utf-8",
    )

    class _ArtifactHandler(BaseHTTPRequestHandler):
        def log_message(self, fmt: str, *args: object) -> None:
            return None

        def do_GET(self) -> None:  # noqa: N802
            mapping = {
                "/output.wasm": output_wasm,
                "/molt_runtime.wasm": runtime_wasm,
                "/weights.bin": weights_bin,
                "/config.json": config_json,
                "/driver-manifest.base.json": manifest_json,
            }
            target = mapping.get(self.path)
            if target is None:
                self.send_response(404)
                self.end_headers()
                return
            payload = target.read_bytes()
            ctype = "application/wasm" if target.suffix == ".wasm" else "application/octet-stream"
            if target.suffix == ".json":
                ctype = "application/json"
            self.send_response(200)
            self.send_header("content-type", ctype)
            self.send_header("content-length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

    server = ThreadingHTTPServer(("127.0.0.1", 0), _ArtifactHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        base_url = f"http://127.0.0.1:{server.server_address[1]}"
        script = tmp_path / "driver_manifest_roundtrip.mjs"
        script.write_text(
            f"""
import {{ initFalconBrowserWebGpu }} from {BROWSER_JS.as_uri()!r};
const session = await initFalconBrowserWebGpu({{
  manifestUrl: {f"{base_url}/driver-manifest.base.json"!r},
}});
const tokens = await session.ocrTokens({{
  width: 32,
  height: 16,
  rgb: new Uint8Array([1,2,3,4,5,6]),
  promptIds: [257, 258],
  maxNewTokens: 3,
  exportName: 'main_molt__ocr_tokens',
}});
console.log(JSON.stringify(tokens));
""".lstrip(),
            encoding="utf-8",
        )
        run = subprocess.run(
            ["node", str(script)],
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=False,
        )
        assert run.returncode == 0, run.stderr
        assert json.loads(run.stdout.strip()) == [257, 258]
    finally:
        server.shutdown()


def test_falcon_browser_driver_accepts_same_origin_relative_manifest_url(
    tmp_path: Path,
) -> None:
    manifest_json = tmp_path / "driver-manifest.base.json"
    manifest_json.write_text(
        json.dumps(
            {
                "target": "falcon.browser_webgpu",
                "artifacts": {
                    "app_wasm": {"url": "/app.wasm"},
                    "runtime_wasm": {"url": "/molt_runtime.wasm"},
                    "config_json": {"url": "/config.json"},
                },
                "weights": {
                    "base_url": "https://weights.example.invalid/falcon",
                    "files": [{"path": "weights.bin", "url": "weights.bin"}],
                },
            }
        )
        + "\n",
        encoding="utf-8",
    )
    script = tmp_path / "relative_manifest_url.mjs"
    script.write_text(
        f"""
import {{ initFalconBrowserWebGpu }} from {BROWSER_JS.as_uri()!r};
globalThis.fetch = async (url) => {{
  if (String(url) === "https://example.invalid/driver-manifest.base.json") {{
    return {{
      ok: true,
      async json() {{
        return {manifest_json.read_text(encoding="utf-8").strip()};
      }},
    }};
  }}
  throw new Error(`resolved-fetch:${{String(url)}}`);
}};
globalThis.location = new URL("https://example.invalid/index.html");
try {{
  await initFalconBrowserWebGpu({{ manifestUrl: "/driver-manifest.base.json" }});
}} catch (error) {{
  console.log(String(error));
}}
""".lstrip(),
        encoding="utf-8",
    )
    run = subprocess.run(
        ["node", str(script)],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    assert run.returncode == 0, run.stderr
    assert "Relative manifestUrl requires" not in run.stdout
    assert "resolved-fetch:" in run.stdout
