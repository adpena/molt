from __future__ import annotations

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
