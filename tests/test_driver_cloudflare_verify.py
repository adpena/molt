from __future__ import annotations

import json
import subprocess
from pathlib import Path


def test_thin_adapter_run_wrangler_check_assembles_command(
    tmp_path: Path,
    monkeypatch,
) -> None:
    from drivers.cloudflare.thin_adapter import verify

    bundle_root = tmp_path / "bundle"
    bundle_root.mkdir()
    wrangler_config = bundle_root / "wrangler.jsonc"
    wrangler_config.write_text("{}\n", encoding="utf-8")
    captured: dict[str, object] = {}

    def fake_run(cmd, cwd, env, verbose):
        captured["cmd"] = list(cmd)
        captured["cwd"] = cwd
        return subprocess.CompletedProcess(cmd, 0, "ok\n", "")

    monkeypatch.setattr(verify, "_run_command", fake_run)

    result = verify.run_wrangler_check(
        wrangler="wrangler",
        bundle_root=bundle_root,
        wrangler_config=wrangler_config,
        project_root=tmp_path,
        env={"TMPDIR": str(tmp_path / "tmp")},
        verbose=False,
        run_id="session",
    )

    assert result.returncode == 0
    assert captured["cmd"] == [
        "wrangler",
        "check",
        "--config",
        str(wrangler_config),
    ]
    assert captured["cwd"] == bundle_root


def test_thin_adapter_run_wrangler_dry_run_assembles_command(
    tmp_path: Path,
    monkeypatch,
) -> None:
    from drivers.cloudflare.thin_adapter import verify

    bundle_root = tmp_path / "bundle"
    bundle_root.mkdir()
    wrangler_config = bundle_root / "wrangler.jsonc"
    wrangler_config.write_text("{}\n", encoding="utf-8")
    captured: dict[str, object] = {}

    def fake_run(cmd, cwd, env, verbose):
        captured["cmd"] = list(cmd)
        captured["cwd"] = cwd
        return subprocess.CompletedProcess(cmd, 0, "dry-run ok\n", "")

    monkeypatch.setattr(verify, "_run_command", fake_run)

    result = verify.run_wrangler_dry_run(
        wrangler="wrangler",
        bundle_root=bundle_root,
        wrangler_config=wrangler_config,
        project_root=tmp_path,
        env={"TMPDIR": str(tmp_path / "tmp")},
        verbose=False,
        run_id="session",
    )

    assert result.returncode == 0
    assert captured["cmd"] == [
        "wrangler",
        "deploy",
        "--dry-run",
        "--outdir",
        str(tmp_path / "tmp" / "drivers" / "cloudflare" / "session" / "dry-run"),
        "--config",
        str(wrangler_config),
    ]
    assert captured["cwd"] == bundle_root


def test_thin_adapter_validate_bundle_contract_accepts_materialized_layout(
    tmp_path: Path,
) -> None:
    from drivers.cloudflare.thin_adapter import verify

    bundle_root = tmp_path / "bundle"
    assets_root = bundle_root / "assets"
    assets_root.mkdir(parents=True)
    for name in [
        "app.wasm",
        "molt_runtime.wasm",
        "browser.js",
        "browser_host.js",
        "molt_vfs_browser.js",
        "config.json",
    ]:
        (assets_root / name).write_text("x\n", encoding="utf-8")
    (assets_root / "driver-manifest.base.json").write_text(
        json.dumps(
            {
                "target": "falcon.browser_webgpu",
                "artifacts": {
                    "app_wasm": {"url": "/app.wasm"},
                    "runtime_wasm": {"url": "/molt_runtime.wasm"},
                    "config_json": {"url": "/config.json"},
                    "browser_loader": {"url": "/browser.js"},
                },
            }
        )
        + "\n",
        encoding="utf-8",
    )
    worker_entrypoint = bundle_root / "drivers" / "falcon" / "browser_webgpu" / "worker.ts"
    worker_entrypoint.parent.mkdir(parents=True)
    worker_entrypoint.write_text("export default {};\n", encoding="utf-8")
    wrangler_config = bundle_root / "wrangler.jsonc"
    wrangler_config.write_text(
        json.dumps(
            {
                "assets": {
                    "directory": "./assets",
                    "binding": "ASSETS",
                    "run_worker_first": ["/driver-manifest.json"],
                }
            }
        )
        + "\n",
        encoding="utf-8",
    )

    contract = verify.validate_bundle_contract(
        bundle_root=bundle_root,
        wrangler_config=wrangler_config,
    )

    assert contract["bundle_root"] == str(bundle_root.resolve())


def test_thin_adapter_validate_bundle_contract_rejects_missing_manifest_route(
    tmp_path: Path,
) -> None:
    from drivers.cloudflare.thin_adapter import verify

    bundle_root = tmp_path / "bundle"
    assets_root = bundle_root / "assets"
    assets_root.mkdir(parents=True)
    for name in [
        "app.wasm",
        "molt_runtime.wasm",
        "browser.js",
        "browser_host.js",
        "molt_vfs_browser.js",
        "config.json",
        "driver-manifest.base.json",
    ]:
        (assets_root / name).write_text("x\n", encoding="utf-8")
    worker_entrypoint = bundle_root / "drivers" / "falcon" / "browser_webgpu" / "worker.ts"
    worker_entrypoint.parent.mkdir(parents=True)
    worker_entrypoint.write_text("export default {};\n", encoding="utf-8")
    wrangler_config = bundle_root / "wrangler.jsonc"
    wrangler_config.write_text(
        json.dumps(
            {
                "assets": {
                    "directory": "./assets",
                    "binding": "ASSETS",
                    "run_worker_first": [],
                }
            }
        )
        + "\n",
        encoding="utf-8",
    )

    try:
        verify.validate_bundle_contract(
            bundle_root=bundle_root,
            wrangler_config=wrangler_config,
        )
    except RuntimeError as exc:
        assert "run_worker_first" in str(exc)
    else:
        raise AssertionError("expected validate_bundle_contract to reject missing manifest route")
