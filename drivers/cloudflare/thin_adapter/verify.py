from __future__ import annotations

import json
import subprocess
import uuid
from pathlib import Path


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
