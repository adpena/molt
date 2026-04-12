from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DRIVER_DIR = ROOT / "drivers" / "falcon" / "browser_webgpu"
DEPLOY_PY = DRIVER_DIR / "deploy.py"
BENCH_PY = DRIVER_DIR / "bench_hostfed.py"


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
    (target_root / "weights").mkdir()

    deploy = _load_module(DEPLOY_PY, "falcon_driver_deploy")
    surface = deploy.build_deploy_surface(
        config_path=DRIVER_DIR / "wrangler.jsonc",
        target_root=target_root,
    )

    assert surface["target"] == "falcon.browser_webgpu"
    assert surface["target_root"] == str(target_root)
    assert surface["artifacts"]["app_wasm"] == str(artifact_dir / "app.wasm")
    assert surface["artifacts"]["runtime_wasm"] == str(artifact_dir / "molt_runtime.wasm")


def test_falcon_driver_deploy_script_emits_json(tmp_path: Path) -> None:
    target_root = tmp_path / "falcon-target"
    artifact_dir = target_root / "dist" / "browser_split"
    artifact_dir.mkdir(parents=True)
    (artifact_dir / "app.wasm").write_bytes(b"\0asm\x01\x00\x00\x00")
    (artifact_dir / "molt_runtime.wasm").write_bytes(b"\0asm\x01\x00\x00\x00")
    (target_root / "weights").mkdir()

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
