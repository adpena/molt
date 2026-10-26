from __future__ import annotations

from molt._wasm_abi_generated import WASM_EXECUTION_MANIFEST_VERSION

import hashlib
import importlib.util
import json
import shutil
import subprocess
import sys
import threading

import pytest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from molt.browser_asset_closure import (
    BROWSER_HOST_ENTRY_ASSETS,
    canonical_wasm_loader_asset_bytes,
    wasm_loader_asset_closure,
)
from tests.process_guard_common import run_guarded_test_process


ROOT = Path(__file__).resolve().parents[1]
DRIVER_DIR = ROOT / "drivers" / "falcon" / "browser_webgpu"
DEPLOY_PY = DRIVER_DIR / "deploy.py"
BENCH_PY = DRIVER_DIR / "bench_hostfed.py"
BROWSER_JS = DRIVER_DIR / "browser.js"
VERIFY_PY = DRIVER_DIR / "verify.py"


def _run_test_process(args, **kwargs):
    return run_guarded_test_process(
        args,
        prefix="MOLT_FALCON_TEST",
        default_timeout=900.0,
        **kwargs,
    )


def _load_module(path: Path, name: str):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _write_execution_fixture(root: Path) -> None:
    """Build-shaped bytes, independent of the deploy closure reader."""

    def descriptor(name: str) -> dict[str, object]:
        payload = (root / name).read_bytes()
        return {
            "path": name,
            "size": len(payload),
            "sha256": hashlib.sha256(payload).hexdigest(),
        }

    assets = {}
    for name in wasm_loader_asset_closure(ROOT / "wasm", BROWSER_HOST_ENTRY_ASSETS):
        target = root / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(canonical_wasm_loader_asset_bytes(ROOT / "wasm" / name))
        assets[name] = descriptor(name)
    # A non-loader declared input must travel with the build too.
    (root / "bundle.tar").write_bytes(b"independent bundle fixture")
    assets["bundle"] = descriptor("bundle.tar")
    (root / "manifest.json").write_text(
        json.dumps(
            {
                "version": WASM_EXECUTION_MANIFEST_VERSION,
                "mode": "split-runtime",
                "modules": {
                    "app": descriptor("app.wasm"),
                    "runtime": descriptor("molt_runtime.wasm"),
                },
                "assets": assets,
                "abi": {"app_exports": {"sentinel": "must survive unchanged"}},
                "entry": {"module": "app", "function": "molt_main"},
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )


def test_falcon_driver_deploy_surface_is_target_root_driven(tmp_path: Path) -> None:
    target_root = tmp_path / "falcon-target"
    artifact_dir = target_root / "dist" / "browser_split"
    artifact_dir.mkdir(parents=True)
    (artifact_dir / "app.wasm").write_bytes(b"\0asm\x01\x00\x00\x00")
    (artifact_dir / "molt_runtime.wasm").write_bytes(b"\0asm\x01\x00\x00\x00")
    _write_execution_fixture(artifact_dir)
    (target_root / "config.json").write_text('{"dim":2}\n', encoding="utf-8")
    weights_dir = target_root / "weights"
    weights_dir.mkdir()
    (weights_dir / "layer0.safetensors").write_bytes(b"weights")
    (weights_dir / "tokenizer.json").write_text(
        '{"model":{"vocab":{}}}\n', encoding="utf-8"
    )

    deploy = _load_module(DEPLOY_PY, "falcon_driver_deploy")
    surface = deploy.build_deploy_surface(
        config_path=DRIVER_DIR / "wrangler.jsonc",
        target_root=target_root,
    )

    assert surface["target"] == "falcon.browser_webgpu"
    assert surface["target_root"] == str(target_root)
    assert surface["artifacts"]["execution_manifest"] == str(
        artifact_dir / "manifest.json"
    )
    assert surface["artifacts"]["config_json"] == str(target_root / "config.json")
    assert surface["artifacts"]["tokenizer_json"] == str(weights_dir / "tokenizer.json")
    assert surface["status"] == "manifest_ready"
    immutable = surface["artifact_manifest"]["immutable"]
    assert immutable["execution_manifest"]["sha256"]
    assert immutable["config_json"]["sha256"]
    assert immutable["tokenizer_json"]["sha256"]
    assert immutable["browser_loader"]["relative_path"] == "browser.js"
    assert immutable["worker_entrypoint"]["relative_path"] == "worker.ts"
    assert immutable["wrangler_config"]["relative_path"] == "wrangler.jsonc"
    assert (
        surface["artifact_manifest"]["weights"][0]["relative_path"]
        == "layer0.safetensors"
    )


def test_falcon_driver_deploy_surface_accepts_weights_config_layout(
    tmp_path: Path,
) -> None:
    target_root = tmp_path / "falcon-target"
    artifact_dir = target_root / "dist" / "browser_split"
    artifact_dir.mkdir(parents=True)
    (artifact_dir / "app.wasm").write_bytes(b"\0asm\x01\x00\x00\x00")
    (artifact_dir / "molt_runtime.wasm").write_bytes(b"\0asm\x01\x00\x00\x00")
    _write_execution_fixture(artifact_dir)
    weights_dir = target_root / "weights"
    weights_dir.mkdir()
    (weights_dir / "config.json").write_text('{"dim":2}\n', encoding="utf-8")
    (weights_dir / "layer0.safetensors").write_bytes(b"weights")
    (weights_dir / "tokenizer.json").write_text(
        '{"model":{"vocab":{}}}\n', encoding="utf-8"
    )

    deploy = _load_module(DEPLOY_PY, "falcon_driver_deploy_weights_config")
    surface = deploy.build_deploy_surface(
        config_path=DRIVER_DIR / "wrangler.jsonc",
        target_root=target_root,
    )

    assert surface["artifacts"]["config_json"] == str(weights_dir / "config.json")
    assert (
        surface["artifact_manifest"]["immutable"]["config_json"]["relative_path"]
        == "weights/config.json"
    )


def test_falcon_driver_deploy_surface_accepts_alternate_weights_root(
    tmp_path: Path,
) -> None:
    target_root = tmp_path / "falcon-target"
    artifact_dir = target_root / "dist" / "browser_split"
    artifact_dir.mkdir(parents=True)
    (artifact_dir / "app.wasm").write_bytes(b"\0asm\x01\x00\x00\x00")
    (artifact_dir / "molt_runtime.wasm").write_bytes(b"\0asm\x01\x00\x00\x00")
    _write_execution_fixture(artifact_dir)
    (target_root / "config.json").write_text('{"dim":2}\n', encoding="utf-8")

    default_weights = target_root / "weights"
    default_weights.mkdir()
    (default_weights / "layer0.safetensors").write_bytes(b"old")
    (default_weights / "tokenizer.json").write_text(
        '{"model":{"vocab":{}}}\n', encoding="utf-8"
    )

    alt_weights = tmp_path / "falcon-weights-f16"
    alt_weights.mkdir()
    (alt_weights / "model.safetensors").write_bytes(b"new-weights")
    (alt_weights / "tokenizer.json").write_text(
        '{"model":{"vocab":{}}}\n', encoding="utf-8"
    )

    deploy = _load_module(DEPLOY_PY, "falcon_driver_deploy_alt_weights")
    surface = deploy.build_deploy_surface(
        config_path=DRIVER_DIR / "wrangler.jsonc",
        target_root=target_root,
        weights_root=alt_weights,
    )

    assert surface["artifacts"]["weights_dir"] == str(alt_weights.resolve())
    assert surface["artifacts"]["tokenizer_json"] == str(
        (alt_weights / "tokenizer.json").resolve()
    )
    assert (
        surface["artifact_manifest"]["weights"][0]["relative_path"]
        == "model.safetensors"
    )


def test_falcon_driver_deploy_surface_accepts_alternate_weights_root_config_layout(
    tmp_path: Path,
) -> None:
    target_root = tmp_path / "falcon-target"
    artifact_dir = target_root / "dist" / "browser_split"
    artifact_dir.mkdir(parents=True)
    (artifact_dir / "app.wasm").write_bytes(b"\0asm\x01\x00\x00\x00")
    (artifact_dir / "molt_runtime.wasm").write_bytes(b"\0asm\x01\x00\x00\x00")
    _write_execution_fixture(artifact_dir)

    alt_weights = tmp_path / "falcon-weights-f16"
    alt_weights.mkdir()
    (alt_weights / "config.json").write_text('{"dim":2}\n', encoding="utf-8")
    (alt_weights / "model.safetensors").write_bytes(b"new-weights")
    (alt_weights / "tokenizer.json").write_text(
        '{"model":{"vocab":{}}}\n', encoding="utf-8"
    )

    deploy = _load_module(DEPLOY_PY, "falcon_driver_deploy_alt_weights_config")
    surface = deploy.build_deploy_surface(
        config_path=DRIVER_DIR / "wrangler.jsonc",
        target_root=target_root,
        weights_root=alt_weights,
    )

    assert surface["artifacts"]["config_json"] == str(
        (alt_weights / "config.json").resolve()
    )
    assert (
        surface["artifact_manifest"]["immutable"]["config_json"]["relative_path"]
        == "config.json"
    )


def test_falcon_driver_deploy_script_emits_json(tmp_path: Path) -> None:
    target_root = tmp_path / "falcon-target"
    artifact_dir = target_root / "dist" / "browser_split"
    artifact_dir.mkdir(parents=True)
    (artifact_dir / "app.wasm").write_bytes(b"\0asm\x01\x00\x00\x00")
    (artifact_dir / "molt_runtime.wasm").write_bytes(b"\0asm\x01\x00\x00\x00")
    _write_execution_fixture(artifact_dir)
    (target_root / "config.json").write_text('{"dim":2}\n', encoding="utf-8")
    weights_dir = target_root / "weights"
    weights_dir.mkdir()
    (weights_dir / "layer0.safetensors").write_bytes(b"weights")
    (weights_dir / "tokenizer.json").write_text(
        '{"model":{"vocab":{}}}\n', encoding="utf-8"
    )

    res = _run_test_process(
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
    assert payload["artifact_manifest"]["immutable"]["execution_manifest"]["sha256"]


def test_falcon_driver_materialize_bundle_emits_manifest_and_assets(
    tmp_path: Path,
) -> None:
    target_root = tmp_path / "falcon-target"
    artifact_dir = target_root / "dist" / "browser_split"
    artifact_dir.mkdir(parents=True)
    (artifact_dir / "app.wasm").write_bytes(b"\0asm\x01\x00\x00\x00")
    (artifact_dir / "molt_runtime.wasm").write_bytes(b"\0asm\x01\x00\x00\x00")
    _write_execution_fixture(artifact_dir)
    (target_root / "config.json").write_text('{"dim":2}\n', encoding="utf-8")
    weights_dir = target_root / "weights"
    weights_dir.mkdir()
    (weights_dir / "layer0.safetensors").write_bytes(b"weights")
    (weights_dir / "tokenizer.json").write_text(
        '{"model":{"vocab":{}}}\n', encoding="utf-8"
    )

    deploy = _load_module(DEPLOY_PY, "falcon_driver_deploy_materialize")
    bundle = deploy.materialize_deploy_bundle(
        config_path=DRIVER_DIR / "wrangler.jsonc",
        target_root=target_root,
        weights_base_url="https://weights.example.invalid/falcon",
    )

    bundle_root = Path(bundle["bundle_root"])
    assert (bundle_root / "assets" / "runtime" / "app.wasm").exists()
    assert (bundle_root / "assets" / "runtime" / "molt_runtime.wasm").exists()
    assert (bundle_root / "assets" / "browser.js").exists()
    browser_assets = wasm_loader_asset_closure(
        ROOT / "wasm",
        BROWSER_HOST_ENTRY_ASSETS,
    )
    for asset in browser_assets:
        assert bundle_root.joinpath("assets", "runtime", *Path(asset).parts).exists()
    assert (bundle_root / "assets" / "config.json").exists()
    assert (bundle_root / "assets" / "tokenizer.json").exists()
    materialized_browser_js = (bundle_root / "assets" / "browser.js").read_text(
        encoding="utf-8"
    )
    assert (
        'import { loadMoltWasm } from "./runtime/browser_host.js";'
        in materialized_browser_js
    )
    manifest = json.loads(
        (bundle_root / "assets" / "driver-manifest.base.json").read_text(
            encoding="utf-8"
        )
    )
    assert manifest["target"] == "falcon.browser_webgpu"
    assert (
        manifest["artifacts"]["execution_manifest"]["url"] == "/runtime/manifest.json"
    )
    assert "app_wasm" not in manifest["artifacts"]
    assert "runtime_wasm" not in manifest["artifacts"]
    assert "browser_static_assets" not in manifest
    assert manifest["artifacts"]["config_json"]["url"] == "/config.json"
    assert manifest["artifacts"]["tokenizer_json"]["url"] == "/tokenizer.json"
    assert (bundle_root / "assets/runtime/manifest.json").read_bytes() == (
        artifact_dir / "manifest.json"
    ).read_bytes()
    execution = json.loads((artifact_dir / "manifest.json").read_text(encoding="utf-8"))
    for descriptor in [*execution["modules"].values(), *execution["assets"].values()]:
        name = descriptor["path"]
        emitted = bundle_root.joinpath("assets", "runtime", name).read_bytes()
        assert emitted == (artifact_dir / name).read_bytes()
        assert hashlib.sha256(emitted).hexdigest() == descriptor["sha256"]
    loader_bytes = (bundle_root / "assets/browser.js").read_bytes()
    assert (
        manifest["artifacts"]["browser_loader"]["sha256"]
        == hashlib.sha256(loader_bytes).hexdigest()
    )
    assert manifest["weights"]["base_url"] == "https://weights.example.invalid/falcon"
    assert manifest["weights"]["files"][0]["path"] == "layer0.safetensors"
    wrangler = json.loads((bundle_root / "wrangler.jsonc").read_text(encoding="utf-8"))
    assert wrangler["main"] == "./drivers/falcon/browser_webgpu/worker.ts"
    assert wrangler["assets"]["directory"] == "./assets"
    assert wrangler["assets"]["binding"] == "ASSETS"
    assert wrangler["assets"]["run_worker_first"] == ["/driver-manifest.json"]
    if shutil.which("wrangler") is not None:
        check = _run_test_process(
            ["wrangler", "check", "--config", str(bundle_root / "wrangler.jsonc")],
            cwd=bundle_root,
            text=True,
            capture_output=True,
            check=False,
        )
        assert check.returncode == 0, check.stdout + check.stderr


def test_falcon_driver_materialize_bundle_allows_same_origin_weights(
    tmp_path: Path,
) -> None:
    target_root = tmp_path / "falcon-target"
    artifact_dir = target_root / "dist" / "browser_split"
    artifact_dir.mkdir(parents=True)
    (artifact_dir / "app.wasm").write_bytes(b"\0asm\x01\x00\x00\x00")
    (artifact_dir / "molt_runtime.wasm").write_bytes(b"\0asm\x01\x00\x00\x00")
    _write_execution_fixture(artifact_dir)
    (target_root / "config.json").write_text('{"dim":2}\n', encoding="utf-8")
    weights_dir = target_root / "weights"
    weights_dir.mkdir()
    (weights_dir / "layer0.safetensors").write_bytes(b"weights")
    (weights_dir / "tokenizer.json").write_text(
        '{"model":{"vocab":{}}}\n', encoding="utf-8"
    )

    deploy = _load_module(DEPLOY_PY, "falcon_driver_deploy_require_base_url")
    bundle = deploy.materialize_deploy_bundle(
        config_path=DRIVER_DIR / "wrangler.jsonc",
        target_root=target_root,
        weights_base_url=None,
    )

    manifest = json.loads(
        (
            Path(bundle["bundle_root"]) / "assets" / "driver-manifest.base.json"
        ).read_text(encoding="utf-8")
    )
    assert manifest["weights"]["base_url"] is None
    wrangler = json.loads(
        (Path(bundle["bundle_root"]) / "wrangler.jsonc").read_text(encoding="utf-8")
    )
    assert "WEIGHTS_BASE_URL" not in wrangler.get("vars", {})


def test_falcon_driver_bench_script_help() -> None:
    res = _run_test_process(
        [sys.executable, str(BENCH_PY), "--help"],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    assert res.returncode == 0, res.stderr
    assert "--target-root" in res.stdout


@pytest.mark.parametrize(
    "alternate_weights", [False, True], ids=["default", "alternate"]
)
def test_falcon_driver_verify_wrapper_emits_json(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    alternate_weights: bool,
) -> None:
    target_root = tmp_path / "falcon-target"
    artifact_dir = target_root / "dist" / "browser_split"
    artifact_dir.mkdir(parents=True)
    (artifact_dir / "app.wasm").write_bytes(b"\0asm\x01\x00\x00\x00")
    (artifact_dir / "molt_runtime.wasm").write_bytes(b"\0asm\x01\x00\x00\x00")
    _write_execution_fixture(artifact_dir)
    (target_root / "config.json").write_text('{"dim":2}\n', encoding="utf-8")
    weights_dir = (
        tmp_path / "falcon-weights-f16"
        if alternate_weights
        else target_root / "weights"
    )
    model_name = "model.safetensors" if alternate_weights else "layer0.safetensors"
    model_bytes = b"weights-f16" if alternate_weights else b"weights"
    base_url = "https://weights.example.invalid/" + (
        "falcon-f16" if alternate_weights else "falcon"
    )
    weights_dir.mkdir()
    if alternate_weights:
        (weights_dir / "config.json").write_text('{"dim":2}\n', encoding="utf-8")
    (weights_dir / model_name).write_bytes(model_bytes)
    (weights_dir / "tokenizer.json").write_text(
        '{"model":{"vocab":{}}}\n', encoding="utf-8"
    )

    from drivers.cloudflare.thin_adapter import verify as thin_verify
    from drivers.falcon.browser_webgpu import verify

    commands = []

    def external_success(cmd, *, cwd, env, verbose):
        commands.append((list(cmd), cwd))
        return subprocess.CompletedProcess(cmd, 0, "external success\n", "")

    monkeypatch.setattr(thin_verify, "_run_command", external_success)
    monkeypatch.setattr(verify, "REPO_ROOT", tmp_path / "project")
    artifacts_root = tmp_path / "artifacts"
    monkeypatch.setenv("MOLT_EXT_ROOT", str(artifacts_root))
    monkeypatch.setenv("MOLT_SCRATCH_STORAGE", "disk")
    monkeypatch.setattr(
        sys,
        "argv",
        [
            str(VERIFY_PY),
            "--target-root",
            str(target_root),
            *(["--weights-root", str(weights_dir)] if alternate_weights else []),
            "--weights-base-url",
            base_url,
            "--wrangler",
            "wrangler-fixture",
            "--run-id",
            "falcon-portable",
        ],
    )
    assert verify.main() == 0
    payload = json.loads(capsys.readouterr().out)
    bundle_root = Path(payload["bundle"]["bundle_root"])
    config = str(bundle_root / "wrangler.jsonc")
    assert len(commands) == 2
    assert commands[0] == (
        ["wrangler-fixture", "check", "--config", config],
        bundle_root,
    )
    dry_out = Path(commands[1][0][4])
    assert dry_out == (
        artifacts_root.resolve()
        / "tmp"
        / "drivers"
        / "cloudflare"
        / "falcon-portable"
        / "dry-run"
    )
    assert dry_out.is_dir()
    assert commands[1] == (
        [
            "wrangler-fixture",
            "deploy",
            "--dry-run",
            "--outdir",
            str(dry_out),
            "--config",
            config,
        ],
        bundle_root,
    )
    assert payload["target"] == "falcon.browser_webgpu"
    assert payload["bundle"]["bundle_root"]
    assert payload["bundle_contract"]["manifest_asset"]
    assert payload["wrangler_check"]["returncode"] == 0
    assert payload["wrangler_dry_run"]["returncode"] == 0
    manifest = json.loads(
        Path(payload["bundle"]["manifest_asset"]).read_text(encoding="utf-8")
    )
    assert manifest["weights"]["base_url"] == base_url
    model_record = next(
        record
        for record in manifest["weights"]["files"]
        if record["path"] == model_name
    )
    assert model_record["size_bytes"] == len(model_bytes)
    assert model_record["sha256"] == hashlib.sha256(model_bytes).hexdigest()


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
        **dict(__import__("os").environ),
        "PYTHONPATH": str(ROOT / "src"),
        "MOLT_WASM_LINKED": "0",
    }
    build = _run_test_process(
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
        cwd=ROOT,
        env=build_env,
        text=True,
        capture_output=True,
        check=False,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "app.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    weights_bin = tmp_path / "weights.bin"
    weights_bin.write_bytes(b"weights")
    weights_config = tmp_path / "weights-config.json"
    weights_config.write_text('{"ignored":true}\n', encoding="utf-8")
    config_json = tmp_path / "config.json"
    config_json.write_text('{"dim":2}\n', encoding="utf-8")
    tokenizer_json = tmp_path / "tokenizer.json"
    tokenizer_json.write_text('{"model":{"vocab":{}}}\n', encoding="utf-8")
    manifest_json = tmp_path / "driver-manifest.base.json"

    class _ArtifactHandler(BaseHTTPRequestHandler):
        def log_message(self, fmt: str, *args: object) -> None:
            return None

        def do_GET(self) -> None:  # noqa: N802
            mapping = {
                "/app.wasm": output_wasm,
                "/manifest.json": tmp_path / "manifest.json",
                "/molt_runtime.wasm": runtime_wasm,
                "/weights/falcon-ocr/test-hash/config.json": weights_config,
                "/weights/falcon-ocr/test-hash/model.safetensors": weights_bin,
                "/config.json": config_json,
                "/tokenizer.json": tokenizer_json,
                "/driver-manifest.base.json": manifest_json,
            }
            execution = json.loads(
                (tmp_path / "manifest.json").read_text(encoding="utf-8")
            )
            mapping.update(
                {
                    "/" + row["path"]: tmp_path / row["path"]
                    for row in execution.get("assets", {}).values()
                }
            )
            target = mapping.get(self.path)
            if target is None:
                self.send_response(404)
                self.end_headers()
                return
            payload = target.read_bytes()
            ctype = (
                "application/wasm"
                if target.suffix == ".wasm"
                else "application/octet-stream"
            )
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
        manifest_json.write_text(
            json.dumps(
                {
                    "target": "falcon.browser_webgpu",
                    "artifacts": {
                        "execution_manifest": {"url": "/manifest.json"},
                        "config_json": {"url": "/config.json"},
                        "tokenizer_json": {"url": "/tokenizer.json"},
                    },
                    "weights": {
                        "base_url": f"{base_url}/weights/falcon-ocr/test-hash",
                        "files": [
                            {"path": "config.json", "url": "config.json"},
                            {"path": "model.safetensors", "url": "model.safetensors"},
                        ],
                    },
                    "exports": {
                        "init": "main_molt__init",
                        "ocrTokens": "main_molt__ocr_tokens",
                    },
                }
            )
            + "\n",
            encoding="utf-8",
        )
        script = tmp_path / "driver_roundtrip.mjs"
        script.write_text(
            f"""
import {{ initFalconBrowserWebGpu }} from {BROWSER_JS.as_uri()!r};
const session = await initFalconBrowserWebGpu({{
  manifestUrl: {f"{base_url}/driver-manifest.base.json"!r},
}});
let tokens;
try {{
tokens = await session.ocrTokens({{
  width: 32,
  height: 16,
  rgb: new Uint8Array([1,2,3,4,5,6]),
  promptIds: [257, 258],
  maxNewTokens: 3,
}});
}} finally {{ session.host.dispose(); }}
console.log(JSON.stringify(tokens));
console.error(JSON.stringify({{ tokenizerUrl: session.tokenizerUrl }}));
""".lstrip(),
            encoding="utf-8",
        )
        run = _run_test_process(
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
        assert (
            json.loads(run.stderr.strip())["tokenizerUrl"]
            == f"{base_url}/tokenizer.json"
        )
    finally:
        server.shutdown()


def test_falcon_browser_driver_reuses_cached_weights_on_second_init(
    tmp_path: Path,
) -> None:
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
    build = _run_test_process(
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
        cwd=ROOT,
        env=build_env,
        text=True,
        capture_output=True,
        check=False,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "app.wasm"
    runtime_wasm = tmp_path / "molt_runtime.wasm"
    weights_bin = tmp_path / "weights.bin"
    weights_bin.write_bytes(b"weights")
    config_json = tmp_path / "config.json"
    config_json.write_text('{"dim":2}\n', encoding="utf-8")
    tokenizer_json = tmp_path / "tokenizer.json"
    tokenizer_json.write_text('{"model":{"vocab":{}}}\n', encoding="utf-8")
    manifest_json = tmp_path / "driver-manifest.base.json"
    manifest_json.write_text(
        json.dumps(
            {
                "target": "falcon.browser_webgpu",
                "artifacts": {
                    "execution_manifest": {"url": "/manifest.json"},
                    "config_json": {"url": "/config.json"},
                    "tokenizer_json": {"url": "/tokenizer.json"},
                },
                "weights": {
                    "base_url": "/weights/falcon/test-hash",
                    "files": [
                        {"path": "model.safetensors", "url": "model.safetensors"}
                    ],
                },
                "exports": {
                    "init": "main_molt__init",
                    "ocrTokens": "main_molt__ocr_tokens",
                },
            }
        )
        + "\n",
        encoding="utf-8",
    )

    hits = {
        "/weights/falcon/test-hash/model.safetensors": 0,
    }

    class _ArtifactHandler(BaseHTTPRequestHandler):
        def log_message(self, fmt: str, *args: object) -> None:
            return None

        def do_GET(self) -> None:  # noqa: N802
            mapping = {
                "/app.wasm": output_wasm,
                "/manifest.json": tmp_path / "manifest.json",
                "/molt_runtime.wasm": runtime_wasm,
                "/weights/falcon/test-hash/model.safetensors": weights_bin,
                "/config.json": config_json,
                "/tokenizer.json": tokenizer_json,
                "/driver-manifest.base.json": manifest_json,
            }
            execution = json.loads(
                (tmp_path / "manifest.json").read_text(encoding="utf-8")
            )
            mapping.update(
                {
                    "/" + row["path"]: tmp_path / row["path"]
                    for row in execution.get("assets", {}).values()
                }
            )
            target = mapping.get(self.path)
            if target is None:
                self.send_response(404)
                self.end_headers()
                return
            if self.path in hits:
                hits[self.path] += 1
            payload = target.read_bytes()
            ctype = (
                "application/wasm"
                if target.suffix == ".wasm"
                else "application/octet-stream"
            )
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
        script = tmp_path / "driver_cache_roundtrip.mjs"
        script.write_text(
            f"""
import {{ initFalconBrowserWebGpu }} from {BROWSER_JS.as_uri()!r};
const store = new Map();
globalThis.caches = {{
  open: async () => ({{
    match: async (key) => {{
      const value = store.get(String(key));
      return value ? value.clone() : undefined;
    }},
    put: async (key, response) => {{
      store.set(String(key), response.clone());
    }},
  }}),
}};
const first = await initFalconBrowserWebGpu({{
  manifestUrl: {f"{base_url}/driver-manifest.base.json"!r},
}});
try {{
  const second = await initFalconBrowserWebGpu({{
    manifestUrl: {f"{base_url}/driver-manifest.base.json"!r},
  }});
  try {{ console.log("done"); }} finally {{ second.host.dispose(); }}
}} finally {{ first.host.dispose(); }}
""".lstrip(),
            encoding="utf-8",
        )
        run = _run_test_process(
            ["node", str(script)],
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=False,
        )
        assert run.returncode == 0, run.stderr
        assert hits["/weights/falcon/test-hash/model.safetensors"] == 1
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
    build = _run_test_process(
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
        cwd=ROOT,
        env=build_env,
        text=True,
        capture_output=True,
        check=False,
    )
    assert build.returncode == 0, build.stderr

    output_wasm = tmp_path / "app.wasm"
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
                    "execution_manifest": {"url": "/manifest.json"},
                    "config_json": {"url": "/config.json"},
                    "tokenizer_json": {"url": "/tokenizer.json"},
                },
                "weights": {
                    "base_url": None,
                    "files": [{"path": "weights.bin", "url": "/weights.bin"}],
                },
                "exports": {
                    "init": "main_molt__init",
                    "ocrTokens": "main_molt__ocr_tokens",
                },
            }
        )
        + "\n",
        encoding="utf-8",
    )
    tokenizer_json = tmp_path / "tokenizer.json"
    tokenizer_json.write_text('{"model":{"vocab":{}}}\n', encoding="utf-8")

    class _ArtifactHandler(BaseHTTPRequestHandler):
        def log_message(self, fmt: str, *args: object) -> None:
            return None

        def do_GET(self) -> None:  # noqa: N802
            mapping = {
                "/app.wasm": output_wasm,
                "/manifest.json": tmp_path / "manifest.json",
                "/molt_runtime.wasm": runtime_wasm,
                "/weights.bin": weights_bin,
                "/config.json": config_json,
                "/tokenizer.json": tokenizer_json,
                "/driver-manifest.base.json": manifest_json,
            }
            execution = json.loads(
                (tmp_path / "manifest.json").read_text(encoding="utf-8")
            )
            mapping.update(
                {
                    "/" + row["path"]: tmp_path / row["path"]
                    for row in execution.get("assets", {}).values()
                }
            )
            target = mapping.get(self.path)
            if target is None:
                self.send_response(404)
                self.end_headers()
                return
            payload = target.read_bytes()
            ctype = (
                "application/wasm"
                if target.suffix == ".wasm"
                else "application/octet-stream"
            )
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
let tokens;
try {{
tokens = await session.ocrTokens({{
  width: 32,
  height: 16,
  rgb: new Uint8Array([1,2,3,4,5,6]),
  promptIds: [257, 258],
  maxNewTokens: 3,
  exportName: 'main_molt__ocr_tokens',
}});
}} finally {{ session.host.dispose(); }}
console.log(JSON.stringify(tokens));
""".lstrip(),
            encoding="utf-8",
        )
        run = _run_test_process(
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
                    "execution_manifest": {"url": "/manifest.json"},
                    "config_json": {"url": "/config.json"},
                    "tokenizer_json": {"url": "/tokenizer.json"},
                },
                "weights": {
                    "base_url": "https://weights.example.invalid/falcon",
                    "files": [{"path": "weights.bin", "url": "weights.bin"}],
                },
                "exports": {
                    "init": "main_molt__init",
                    "ocrTokens": "main_molt__ocr_tokens",
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
    run = _run_test_process(
        ["node", str(script)],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    assert run.returncode == 0, run.stderr
    assert "Relative manifestUrl requires" not in run.stdout
    assert "resolved-fetch:" in run.stdout


def test_falcon_browser_driver_requires_manifest_url(tmp_path: Path) -> None:
    script = tmp_path / "missing_manifest_url.mjs"
    script.write_text(
        f"""
import {{ initFalconBrowserWebGpu }} from {BROWSER_JS.as_uri()!r};
try {{
  await initFalconBrowserWebGpu({{}});
}} catch (error) {{
  console.log(String(error));
}}
""".lstrip(),
        encoding="utf-8",
    )
    run = _run_test_process(
        ["node", str(script)],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    assert run.returncode == 0, run.stderr
    assert "requires manifestUrl" in run.stdout


def test_falcon_browser_driver_fetches_manifest_before_gpu_capability_is_needed(
    tmp_path: Path,
) -> None:
    script = tmp_path / "missing_webgpu.mjs"
    script.write_text(
        f"""
import {{ initFalconBrowserWebGpu }} from {BROWSER_JS.as_uri()!r};
globalThis.fetch = async () => {{
  throw new Error("manifest fetch reached");
}};
try {{
  await initFalconBrowserWebGpu({{ manifestUrl: "https://example.invalid/driver-manifest.base.json" }});
}} catch (error) {{
  console.log(String(error));
}}
""".lstrip(),
        encoding="utf-8",
    )
    run = _run_test_process(
        ["node", str(script)],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    assert run.returncode == 0, run.stderr
    assert "manifest fetch reached" in run.stdout
    assert (
        "requires WebGPU support or an injected gpuKernelDispatcher" not in run.stdout
    )


@pytest.mark.parametrize(
    "damage",
    [
        "missing-manifest",
        "wrong-mode",
        "missing-asset",
        "changed-module",
        "escaped-path",
    ],
)
def test_falcon_deploy_refuses_incomplete_execution_closure(
    tmp_path: Path, damage: str
) -> None:
    from drivers.falcon.browser_webgpu.deploy import execution_artifact_records

    (tmp_path / "app.wasm").write_bytes(b"app")
    (tmp_path / "molt_runtime.wasm").write_bytes(b"runtime")
    _write_execution_fixture(tmp_path)
    path = tmp_path / "manifest.json"
    manifest = json.loads(path.read_text(encoding="utf-8"))
    if damage == "missing-manifest":
        path.unlink()
    elif damage == "wrong-mode":
        manifest["mode"] = "unsupported"
        path.write_text(json.dumps(manifest), encoding="utf-8")
    elif damage == "missing-asset":
        (tmp_path / "bundle.tar").unlink()
    elif damage == "changed-module":
        (tmp_path / "app.wasm").write_bytes(b"other app")
    else:
        manifest["modules"]["app"]["path"] = "../app.wasm"
        path.write_text(json.dumps(manifest), encoding="utf-8")
    with pytest.raises((ValueError, OSError)):
        execution_artifact_records(tmp_path)


def test_falcon_deploy_rejects_incompatible_version_before_module_read(
    tmp_path: Path,
) -> None:
    from drivers.falcon.browser_webgpu.deploy import execution_artifact_records

    (tmp_path / "app.wasm").write_bytes(b"app")
    (tmp_path / "molt_runtime.wasm").write_bytes(b"runtime")
    _write_execution_fixture(tmp_path)
    path = tmp_path / "manifest.json"
    original = json.loads(path.read_text(encoding="utf-8"))
    (tmp_path / "app.wasm").unlink()
    for version in ["missing", None, 0, 1, 2, 4, 99, True, False, "3", 3.0]:
        manifest = dict(original)
        if version == "missing":
            del manifest["version"]
        else:
            manifest["version"] = version
        path.write_text(json.dumps(manifest), encoding="utf-8")
        with pytest.raises(ValueError, match="Molt deployment manifest version must be 3"):
            execution_artifact_records(tmp_path)


@pytest.mark.parametrize("mode", ["split-runtime", "linked"])
def test_falcon_bundle_preserves_build_paths_and_manifest_bytes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mode: str
) -> None:
    from drivers.falcon.browser_webgpu import deploy
    from drivers.cloudflare.thin_adapter.verify import validate_bundle_contract

    root = tmp_path / "model"
    artifacts = root / "dist/browser_split"
    artifacts.mkdir(parents=True)
    (artifacts / "app.wasm").write_bytes(b"different app")
    (artifacts / "molt_runtime.wasm").write_bytes(b"different runtime")
    _write_execution_fixture(artifacts)
    manifest_path = artifacts / "manifest.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    (artifacts / "code").mkdir()
    (artifacts / "app.wasm").rename(artifacts / "code/selected.wasm")
    manifest["modules"]["app"]["path"] = "code/selected.wasm"
    if mode == "linked":
        manifest = {
            "version": WASM_EXECUTION_MANIFEST_VERSION,
            "mode": "linked",
            "modules": {"linked": manifest["modules"]["app"]},
            "abi": {"app_exports": {"sentinel": "linked ABI stays untouched"}},
        }
    if mode == "split-runtime":
        pinned_host = (
            artifacts / "browser_host.js"
        ).read_bytes() + b"\n// selected build bytes\n"
        (artifacts / "browser_host.js").write_bytes(pinned_host)
        manifest["assets"]["browser_host.js"].update(
            size=len(pinned_host), sha256=hashlib.sha256(pinned_host).hexdigest()
        )
    manifest_path.write_text(json.dumps(manifest, indent=3) + "\n", encoding="utf-8")
    (root / "config.json").write_text("{}", encoding="utf-8")
    (root / "weights").mkdir()
    (root / "weights/model.safetensors").write_bytes(b"weights")
    bundle = deploy.materialize_deploy_bundle(
        config_path=DRIVER_DIR / "wrangler.jsonc",
        target_root=root,
        weights_base_url=None,
    )
    output = Path(bundle["assets_root"])
    assert (output / "runtime/manifest.json").read_bytes() == manifest_path.read_bytes()
    assert (output / "runtime/code/selected.wasm").read_bytes() == b"different app"
    assert not (output / "runtime/app.wasm").exists()
    if mode == "split-runtime":
        assert (output / "runtime/browser_host.js").read_bytes() == pinned_host
        assert (
            output / "runtime/bundle.tar"
        ).read_bytes() == b"independent bundle fixture"
    else:
        assert not (output / "runtime/bundle.tar").exists()
        driver = json.loads(
            (output / "driver-manifest.base.json").read_text(encoding="utf-8")
        )
        assert "browser_host.js" in {
            item["relative_path"] for item in driver["hosting_assets"]
        }
        for item in driver["hosting_assets"]:
            assert (
                hashlib.sha256(
                    (output / "runtime" / item["relative_path"]).read_bytes()
                ).hexdigest()
                == item["sha256"]
            )
    # Neither selected split assets nor retained linked hosting may be tied to
    # whichever compiler checkout happens to perform later verification.
    monkeypatch.setattr(deploy, "REPO_ROOT", tmp_path / "absent-newer-compiler")
    validate_bundle_contract(
        bundle_root=Path(bundle["bundle_root"]),
        wrangler_config=Path(bundle["wrangler_config"]),
    )
    if mode == "linked":
        host_path = output / "runtime/browser_host.js"
        host_bytes = host_path.read_bytes()
        host_path.write_bytes(b"unrelated host")
        with pytest.raises(RuntimeError, match="hosting asset bytes drifted"):
            validate_bundle_contract(
                bundle_root=Path(bundle["bundle_root"]),
                wrangler_config=Path(bundle["wrangler_config"]),
            )
        host_path.write_bytes(host_bytes)
    (output / "runtime/code/selected.wasm").write_bytes(b"replacement")
    with pytest.raises(ValueError, match="bytes mismatch"):
        validate_bundle_contract(
            bundle_root=Path(bundle["bundle_root"]),
            wrangler_config=Path(bundle["wrangler_config"]),
        )


def test_falcon_parallel_initialization_transfers_or_retires_host(
    tmp_path: Path,
) -> None:
    """Drive all settlement orders without compiling or weakening the host API."""
    if shutil.which("node") is None:
        pytest.skip("Node is required for browser ownership controls")
    # Only the host acquisition is controlled. Use the shipped resolver and
    # combined-error authority; compiled roundtrips above use the complete host.
    original = 'import { loadMoltWasm } from "../../../wasm/browser_host.js";'
    source = BROWSER_JS.read_text(encoding="utf-8")
    assert source.count(original) == 1
    (tmp_path / "driver.mjs").write_text(
        source.replace(
            original, "const loadMoltWasm = options => globalThis.acquireHost(options);"
        ),
        encoding="utf-8",
    )
    script = tmp_path / "ownership.mjs"
    script.write_text(
        "import { resolveMoltWasmUrls } from "
        + json.dumps((ROOT / "wasm/browser_host.js").as_uri())
        + ";\n"
        + r"""
import assert from 'node:assert/strict';
import { initFalconBrowserWebGpu } from './driver.mjs';
const executionUrl = 'https://artifacts.example/build/manifest.json';
const execution = {version: 3, mode: 'split-runtime', modules: {
  app: {path: 'code/selected.wasm'}, runtime: {path: 'rt/selected.wasm'}
}};
assert.deepEqual(resolveMoltWasmUrls({version: 3, mode: 'linked', modules: {
  linked: {path: 'code/linked.wasm'}
}}, executionUrl), {wasmUrl: null, runtimeUrl: null,
  linkedUrl: 'https://artifacts.example/build/code/linked.wasm'});
const metadata = {artifacts: {
  execution_manifest: {url: executionUrl}, config_json: {url: '/config.json'}
}, weights: {base_url: 'https://model.example/', files: [{url: 'model.safetensors'}]}};
const defer = () => { let resolve, reject; const promise = new Promise((a,b) => {resolve=a;reject=b;}); return {promise,resolve,reject}; };
for (const scenario of ['success', 'weights-first', 'config-first', 'host-first', 'load', 'run', 'init', 'cleanup']) {
  const entered = [defer(), defer(), defer()];
  const gates = [defer(), defer(), defer()];
  const primary = new Error(scenario);
  const cleanup = new Error('cleanup failure');
  let retired = 0, ran = 0, calls = 0;
  const host = {
    run() { ran++; if (scenario === 'run') throw primary; },
    async invokeExport() { calls++; if (scenario === 'init') throw primary; return {resultJson: [17]}; },
    dispose() { retired++; if (scenario === 'cleanup') throw cleanup; }
  };
  globalThis.acquireHost = async options => {
    entered[0].resolve();
    assert.equal(options.manifestUrl, executionUrl);
    assert.equal(options.manifest, undefined);
    assert.equal(options.wasmUrl, undefined);
    assert.equal(options.runtimeUrl, undefined);
    assert.deepEqual(resolveMoltWasmUrls(execution, options.manifestUrl), {
      wasmUrl: 'https://artifacts.example/build/code/selected.wasm',
      runtimeUrl: 'https://artifacts.example/build/rt/selected.wasm', linkedUrl: null
    });
    await gates[0].promise;
    return host;
  };
  globalThis.fetch = async url => {
    if (url === 'https://model.example/driver.json') return {ok: true, json: async () => metadata};
    const index = String(url).endsWith('model.safetensors') ? 1 : 2;
    assert.equal(String(url), index === 1 ? 'https://model.example/model.safetensors' : 'https://model.example/config.json');
    entered[index].resolve(); await gates[index].promise;
    return {ok: true, arrayBuffer: async () => new ArrayBuffer(2), text: async () => '{}'};
  };
  const pending = initFalconBrowserWebGpu({manifestUrl: 'https://model.example/driver.json', browserHostOptions: {
    manifestUrl: 'https://wrong.example/default.json', manifest: {version: 3, mode: 'linked'}
  }});
  await Promise.all(entered.map(x => x.promise));
  if (scenario === 'weights-first' || scenario === 'cleanup') {
    gates[1].reject(primary); gates[2].resolve(); await Promise.resolve(); gates[0].resolve();
  } else if (scenario === 'config-first') {
    gates[2].reject(primary); gates[1].resolve(); await Promise.resolve(); gates[0].resolve();
  } else if (scenario === 'host-first') {
    gates[0].resolve(); await Promise.resolve(); gates[1].reject(primary); gates[2].resolve();
  } else if (scenario === 'load') {
    gates[0].reject(primary); gates[1].resolve(); gates[2].resolve();
  } else { gates.forEach(x => x.resolve()); }
  if (scenario === 'success') {
    const session = await pending;
    assert.equal(retired, 0); assert.equal(ran, 1); assert.equal(calls, 1);
    assert.deepEqual(await session.ocrTokens({width: 1,height: 1,rgb: []}), [17]);
    await session.invokeExport('again'); assert.equal(calls, 3);
    assert.equal(session.host, host); session.host.dispose(); assert.equal(retired, 1);
  } else {
    await assert.rejects(pending, error => {
      if (scenario === 'cleanup') {
        assert.equal(error.cause, primary); assert.deepEqual(error.errors, [primary, cleanup]);
      } else assert.equal(error, primary);
      return true;
    });
    assert.equal(retired, scenario === 'load' ? 0 : 1);
    if (!['run', 'init'].includes(scenario)) { assert.equal(ran, 0); assert.equal(calls, 0); }
  }
}
console.log('ownership-ok');
""",
        encoding="utf-8",
    )
    run = _run_test_process(
        ["node", str(script)], cwd=ROOT, text=True, capture_output=True, check=False
    )
    assert run.returncode == 0, run.stderr
    assert run.stdout.strip() == "ownership-ok"


@pytest.mark.parametrize("mode", ["split-runtime", "linked"])
@pytest.mark.parametrize("omitted", ["runtime_lifecycle.js", "nested/leaf.mjs"])
def test_falcon_refuses_omitted_loader_reference_even_without_descriptor(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mode: str, omitted: str
) -> None:
    from drivers.falcon.browser_webgpu import deploy
    from drivers.cloudflare.thin_adapter.verify import validate_bundle_contract

    root = tmp_path / "model"
    artifacts = root / "dist/browser_split"
    artifacts.mkdir(parents=True)
    (artifacts / "app.wasm").write_bytes(b"app")
    (artifacts / "molt_runtime.wasm").write_bytes(b"runtime")
    _write_execution_fixture(artifacts)
    if mode == "linked":
        manifest_path = artifacts / "manifest.json"
        original = json.loads(manifest_path.read_text(encoding="utf-8"))
        manifest_path.write_text(
            json.dumps(
                {
                    "version": WASM_EXECUTION_MANIFEST_VERSION,
                    "mode": "linked",
                    "modules": {"linked": original["modules"]["app"]},
                }
            ),
            encoding="utf-8",
        )
    (root / "config.json").write_text("{}", encoding="utf-8")
    (root / "weights").mkdir()
    (root / "weights/model.safetensors").write_bytes(b"weights")
    bundle = deploy.materialize_deploy_bundle(
        config_path=DRIVER_DIR / "wrangler.jsonc",
        target_root=root,
        weights_base_url=None,
    )
    output = Path(bundle["assets_root"])
    runtime = output / "runtime"
    driver_path = output / "driver-manifest.base.json"
    driver = json.loads(driver_path.read_text(encoding="utf-8"))
    execution_path = runtime / "manifest.json"
    execution = json.loads(execution_path.read_text(encoding="utf-8"))

    def publish_record(name: str, content: bytes) -> None:
        path = runtime / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
        digest = hashlib.sha256(content).hexdigest()
        if mode == "split-runtime":
            execution["assets"][name] = {
                "path": name,
                "size": len(content),
                "sha256": digest,
            }
        else:
            driver["hosting_assets"] = [
                row for row in driver["hosting_assets"] if row["relative_path"] != name
            ] + [{"relative_path": name, "size_bytes": len(content), "sha256": digest}]

    def publish_metadata() -> None:
        payload = (json.dumps(execution) + "\n").encode()
        execution_path.write_bytes(payload)
        driver["artifacts"]["execution_manifest"].update(
            size_bytes=len(payload),
            sha256=hashlib.sha256(payload).hexdigest(),
        )
        driver_path.write_text(json.dumps(driver), encoding="utf-8")

    if omitted == "nested/leaf.mjs":
        publish_record(
            "browser_host.js",
            (runtime / "browser_host.js").read_bytes()
            + b'\nimport "./nested/child.mjs";\n',
        )
        publish_record("nested/child.mjs", b'import "./leaf.mjs";\n')
        publish_record("nested/leaf.mjs", b"export const retained = true;\n")
    publish_metadata()
    monkeypatch.setattr(deploy, "REPO_ROOT", tmp_path / "unrelated-compiler")
    kwargs = dict(
        bundle_root=Path(bundle["bundle_root"]),
        wrangler_config=Path(bundle["wrangler_config"]),
    )
    validate_bundle_contract(**kwargs)
    (runtime / omitted).unlink()
    if mode == "split-runtime":
        execution["assets"] = {
            key: value
            for key, value in execution["assets"].items()
            if value["path"] != omitted
        }
    else:
        driver["hosting_assets"] = [
            row for row in driver["hosting_assets"] if row["relative_path"] != omitted
        ]
    publish_metadata()
    # All remaining records are valid. Only the real retained import proves
    # the omitted dependency is required; neither file nor descriptor remains.
    with pytest.raises(ValueError, match=f"undeclared browser asset {omitted}"):
        validate_bundle_contract(**kwargs)
    if mode == "split-runtime":
        with pytest.raises(ValueError, match=f"undeclared browser asset {omitted}"):
            deploy.execution_artifact_records(runtime)
