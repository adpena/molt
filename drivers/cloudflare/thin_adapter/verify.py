from __future__ import annotations

import json
import subprocess
import uuid
from pathlib import Path
from typing import Any


def _logs_root(project_root: Path) -> Path:
    return project_root / "logs" / "drivers" / "cloudflare"


def _tmp_root(project_root: Path) -> Path:
    return project_root / "tmp" / "drivers" / "cloudflare"


def _write_text(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def _run_command(
    cmd: list[str],
    *,
    cwd: Path,
    env: dict[str, str],
    verbose: bool,
) -> subprocess.CompletedProcess[str]:
    if verbose:
        print(f"Running: {subprocess.list2cmdline(cmd)}")
    return subprocess.run(
        cmd,
        cwd=cwd,
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )


def validate_bundle_contract(
    *,
    bundle_root: Path,
    wrangler_config: Path,
    manifest_asset_path: str = "assets/driver-manifest.base.json",
    worker_entrypoint_path: str = "drivers/falcon/browser_webgpu/worker.ts",
) -> dict[str, Any]:
    bundle_root = bundle_root.resolve()
    wrangler_config = wrangler_config.resolve()
    if not bundle_root.is_dir():
        raise RuntimeError(f"Cloudflare bundle root not found: {bundle_root}")
    if not wrangler_config.is_file():
        raise RuntimeError(f"Cloudflare wrangler config not found: {wrangler_config}")

    assets_root = bundle_root / "assets"
    manifest_asset = bundle_root / manifest_asset_path
    worker_entrypoint = bundle_root / worker_entrypoint_path
    required_assets = [
        assets_root / "app.wasm",
        assets_root / "molt_runtime.wasm",
        assets_root / "browser.js",
        assets_root / "browser_host.js",
        assets_root / "molt_vfs_browser.js",
        assets_root / "config.json",
        manifest_asset,
        worker_entrypoint,
    ]
    missing = [str(path.relative_to(bundle_root)) for path in required_assets if not path.exists()]
    if missing:
        raise RuntimeError(
            "Cloudflare thin-adapter bundle contract is incomplete: " + ", ".join(sorted(missing))
        )

    config = json.loads(wrangler_config.read_text(encoding="utf-8"))
    assets = config.get("assets")
    if not isinstance(assets, dict):
        raise RuntimeError("Cloudflare wrangler config must include an assets section")
    if assets.get("directory") != "./assets":
        raise RuntimeError("Cloudflare wrangler config assets.directory must be './assets'")
    if assets.get("binding") != "ASSETS":
        raise RuntimeError("Cloudflare wrangler config assets.binding must be 'ASSETS'")
    run_worker_first = assets.get("run_worker_first")
    if not isinstance(run_worker_first, list) or "/driver-manifest.json" not in run_worker_first:
        raise RuntimeError(
            "Cloudflare wrangler config assets.run_worker_first must include '/driver-manifest.json'"
        )

    manifest = json.loads(manifest_asset.read_text(encoding="utf-8"))
    if manifest.get("target") != "falcon.browser_webgpu":
        raise RuntimeError("Cloudflare thin-adapter manifest target must be 'falcon.browser_webgpu'")
    artifacts = manifest.get("artifacts")
    if not isinstance(artifacts, dict):
        raise RuntimeError("Cloudflare thin-adapter manifest is missing artifacts")
    for key, expected_url in {
        "app_wasm": "/app.wasm",
        "runtime_wasm": "/molt_runtime.wasm",
        "config_json": "/config.json",
        "browser_loader": "/browser.js",
    }.items():
        entry = artifacts.get(key)
        if not isinstance(entry, dict) or entry.get("url") != expected_url:
            raise RuntimeError(f"Cloudflare thin-adapter manifest {key} url drifted")

    return {
        "bundle_root": str(bundle_root),
        "wrangler_config": str(wrangler_config),
        "manifest_asset": str(manifest_asset),
        "worker_entrypoint": str(worker_entrypoint),
    }


def run_wrangler_check(
    *,
    wrangler: str,
    bundle_root: Path,
    wrangler_config: Path,
    project_root: Path,
    env: dict[str, str],
    verbose: bool,
    run_id: str | None = None,
) -> subprocess.CompletedProcess[str]:
    session = run_id or uuid.uuid4().hex
    cmd = [wrangler, "check", "--config", str(wrangler_config)]
    result = _run_command(cmd, cwd=bundle_root, env=env, verbose=verbose)
    log_root = _logs_root(project_root) / session
    _write_text(log_root / "wrangler-check.stdout.log", result.stdout or "")
    _write_text(log_root / "wrangler-check.stderr.log", result.stderr or "")
    _write_text(
        log_root / "wrangler-check.json",
        json.dumps(
            {
                "command": cmd,
                "returncode": result.returncode,
                "bundle_root": str(bundle_root),
                "wrangler_config": str(wrangler_config),
            },
            indent=2,
        )
        + "\n",
    )
    return result


def run_wrangler_dry_run(
    *,
    wrangler: str,
    bundle_root: Path,
    wrangler_config: Path,
    project_root: Path,
    env: dict[str, str],
    verbose: bool,
    run_id: str | None = None,
) -> subprocess.CompletedProcess[str]:
    session = run_id or uuid.uuid4().hex
    outdir = _tmp_root(project_root) / session / "dry-run"
    outdir.mkdir(parents=True, exist_ok=True)
    cmd = [
        wrangler,
        "deploy",
        "--dry-run",
        "--outdir",
        str(outdir),
        "--config",
        str(wrangler_config),
    ]
    result = _run_command(cmd, cwd=bundle_root, env=env, verbose=verbose)
    log_root = _logs_root(project_root) / session
    _write_text(log_root / "wrangler-dry-run.stdout.log", result.stdout or "")
    _write_text(log_root / "wrangler-dry-run.stderr.log", result.stderr or "")
    _write_text(
        log_root / "wrangler-dry-run.json",
        json.dumps(
            {
                "command": cmd,
                "returncode": result.returncode,
                "bundle_root": str(bundle_root),
                "wrangler_config": str(wrangler_config),
                "outdir": str(outdir),
            },
            indent=2,
        )
        + "\n",
    )
    return result
