from __future__ import annotations

import json
import subprocess
from pathlib import Path

import pytest


def test_extract_live_url_prefers_workers_dev_url() -> None:
    from tools import cloudflare_demo_verify as verify

    stdout = """
    Uploaded worker.
    Deployment complete.
    Live URL: https://molt-python-demo.adpena.workers.dev
    """

    assert (
        verify.extract_live_url(stdout)
        == "https://molt-python-demo.adpena.workers.dev"
    )


def test_validate_bundle_contract_accepts_split_runtime_layout(
    tmp_path: Path,
) -> None:
    from tools import cloudflare_demo_verify as verify

    bundle_root = tmp_path / "bundle"
    bundle_root.mkdir()
    (bundle_root / "worker.js").write_text("export default {};\n")
    (bundle_root / "app.wasm").write_bytes(b"\x00asm\x01\x00\x00\x00")
    (bundle_root / "molt_runtime.wasm").write_bytes(b"\x00asm\x01\x00\x00\x00")
    (bundle_root / "manifest.json").write_text(
        json.dumps(
            {
                "version": 2,
                "mode": "split-runtime",
                "modules": {
                    "app": {"path": "app.wasm", "size": 1},
                    "runtime": {"path": "molt_runtime.wasm", "size": 1},
                },
            }
        )
        + "\n"
    )
    wrangler_config = bundle_root / "wrangler.jsonc"
    wrangler_config.write_text(
        json.dumps(
            {
                "name": "molt-python-demo",
                "main": "worker.js",
                "compatibility_date": "$today",
                "no_bundle": True,
                "find_additional_modules": True,
                "rules": [
                    {"type": "ESModule", "globs": ["**/*.js"], "fallthrough": False},
                    {"type": "CompiledWasm", "globs": ["**/*.wasm"], "fallthrough": False},
                ],
            }
        )
        + "\n"
    )

    contract = verify.validate_bundle_contract(bundle_root, wrangler_config)

    assert contract.bundle_root == bundle_root
    assert contract.wrangler_config == wrangler_config
    assert contract.compatibility_date == "$today"
    assert contract.no_bundle is True
    assert contract.worker_js == bundle_root / "worker.js"
    assert contract.manifest == bundle_root / "manifest.json"


def test_run_wrangler_dry_run_uses_no_bundle_and_outdir(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from tools import cloudflare_demo_verify as verify

    bundle_root = tmp_path / "bundle"
    bundle_root.mkdir()
    wrangler_config = bundle_root / "wrangler.jsonc"
    wrangler_config.write_text("{}\n")
    captured: dict[str, object] = {}

    def fake_run(cmd, cwd, env, verbose):
        captured["cmd"] = list(cmd)
        captured["cwd"] = cwd
        captured["env"] = env
        captured["verbose"] = verbose
        return subprocess.CompletedProcess(cmd, 0, "dry-run ok\n", "")

    monkeypatch.setattr(verify, "_run_command", fake_run)

    result = verify.run_wrangler_dry_run(
        wrangler="wrangler",
        bundle_root=bundle_root,
        wrangler_config=wrangler_config,
        project_root=tmp_path,
        env={"TMPDIR": str(tmp_path / "tmp")},
        json_output=False,
        verbose=False,
        run_id="session",
    )

    assert result.returncode == 0
    assert captured["cmd"] == [
        "wrangler",
        "deploy",
        "--dry-run",
        "--no-bundle",
        "--outdir",
        str(tmp_path / "tmp" / "cloudflare-demo" / "session" / "dry-run"),
        "--config",
        str(wrangler_config),
    ]
    assert captured["cwd"] == bundle_root


def test_run_wrangler_deploy_uses_no_bundle(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from tools import cloudflare_demo_verify as verify

    bundle_root = tmp_path / "bundle"
    bundle_root.mkdir()
    wrangler_config = bundle_root / "wrangler.jsonc"
    wrangler_config.write_text("{}\n")
    captured: dict[str, object] = {}

    def fake_run(cmd, cwd, env, verbose):
        captured["cmd"] = list(cmd)
        captured["cwd"] = cwd
        captured["env"] = env
        captured["verbose"] = verbose
        return subprocess.CompletedProcess(cmd, 0, "https://demo.workers.dev\n", "")

    monkeypatch.setattr(verify, "_run_command", fake_run)

    result = verify.run_wrangler_deploy(
        wrangler="wrangler",
        bundle_root=bundle_root,
        wrangler_config=wrangler_config,
        project_root=tmp_path,
        env={"TMPDIR": str(tmp_path / "tmp")},
        wrangler_args="--minify",
        json_output=False,
        verbose=False,
        run_id="session",
    )

    assert result.returncode == 0
    assert captured["cmd"] == [
        "wrangler",
        "deploy",
        "--no-bundle",
        "--config",
        str(wrangler_config),
        "--minify",
    ]
    assert captured["cwd"] == bundle_root


def test_verify_live_endpoint_writes_report_and_passes(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from tools import cloudflare_demo_verify as verify

    responses = {
        "/": (200, "text/plain; charset=utf-8", "ok root"),
        "/fib/100": (200, "text/plain; charset=utf-8", "ok fib"),
    }

    def fake_probe(base_url: str, path: str, timeout_s: float):
        return responses[path]

    monkeypatch.setattr(verify, "_probe_url", fake_probe)

    result = verify.verify_live_endpoint(
        live_url="https://demo.workers.dev",
        bundle_root=tmp_path / "bundle",
        project_root=tmp_path,
        json_output=False,
        verbose=False,
        run_id="session",
        probes=[
            verify.LiveProbe(path="/"),
            verify.LiveProbe(path="/fib/100"),
        ],
    )

    assert result.returncode == 0
    assert result.report_path.exists()
