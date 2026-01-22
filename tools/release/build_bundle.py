#!/usr/bin/env python3
"""Build release bundles for Molt.

Creates tar.gz (macOS/Linux) or zip (Windows) with a consistent layout.
"""

from __future__ import annotations

import argparse
import shutil
import stat
import tarfile
import tempfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def _write_text(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)


def _make_unix_wrapper(path: Path) -> None:
    script = """#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [ -z "${MOLT_HOME:-}" ]; then
  if [ -w "$ROOT" ]; then
    export MOLT_HOME="$ROOT"
  else
    export MOLT_HOME="$HOME/.molt"
  fi
fi
export MOLT_PROJECT_ROOT="${MOLT_PROJECT_ROOT:-$PWD}"
PYTHON_BIN="${PYTHON:-}"
if [ -z "$PYTHON_BIN" ]; then
  if command -v python3 >/dev/null 2>&1; then
    PYTHON_BIN=python3
  elif command -v python >/dev/null 2>&1; then
    PYTHON_BIN=python
  else
    echo "molt: python3 not found" >&2
    exit 1
  fi
fi
exec "$PYTHON_BIN" "$ROOT/lib/molt/bootstrap.py" "$@"
"""
    _write_text(path, script)
    path.chmod(path.stat().st_mode | stat.S_IEXEC)


def _make_windows_wrapper(root: Path) -> None:
    cmd = (
        "@echo off\r\n"
        "set ROOT=%~dp0..\r\n"
        "if not defined MOLT_HOME set MOLT_HOME=%USERPROFILE%\\.molt\r\n"
        "if not defined MOLT_PROJECT_ROOT set MOLT_PROJECT_ROOT=%CD%\r\n"
        "set BOOT=%ROOT%\\lib\\molt\\bootstrap.py\r\n"
        'if not exist "%BOOT%" (\r\n'
        "  echo molt: bootstrap not found at %BOOT%\r\n"
        "  exit /b 1\r\n"
        ")\r\n"
        'if exist "%SystemRoot%\\py.exe" (\r\n'
        '  py -3.12 "%BOOT%" %*\r\n'
        "  exit /b %ERRORLEVEL%\r\n"
        ")\r\n"
        'python "%BOOT%" %*\r\n'
        "exit /b %ERRORLEVEL%\r\n"
    )
    ps1 = (
        "$root = Split-Path -Parent $MyInvocation.MyCommand.Path\n"
        '$root = Resolve-Path (Join-Path $root "..")\n'
        'if (-not $env:MOLT_HOME) { $env:MOLT_HOME = Join-Path $env:USERPROFILE ".molt" }\n'
        "if (-not $env:MOLT_PROJECT_ROOT) { $env:MOLT_PROJECT_ROOT = (Get-Location).Path }\n"
        '$boot = Join-Path $root "lib" "molt" "bootstrap.py"\n'
        'if (-not (Test-Path $boot)) { throw "molt: bootstrap not found at $boot" }\n'
        "if (Get-Command py -ErrorAction SilentlyContinue) {\n"
        "  py -3.12 $boot @args\n"
        "} else {\n"
        "  python $boot @args\n"
        "}\n"
    )
    _write_text(root / "bin" / "molt.cmd", cmd)
    _write_text(root / "bin" / "molt.ps1", ps1)


def _bundle_root(name: str, version: str) -> Path:
    return Path(f"{name}-{version}")


def _copy_file(src: Path, dst: Path) -> None:
    dst.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(src, dst)


def _bundle_molt(
    root: Path,
    wheel: Path,
    worker_bin: Path | None,
) -> None:
    _copy_file(
        ROOT / "packaging" / "bootstrap.py", root / "lib" / "molt" / "bootstrap.py"
    )
    _copy_file(
        ROOT / "packaging" / "INSTALL.md", root / "share" / "molt" / "INSTALL.md"
    )
    _copy_file(ROOT / "LICENSE", root / "share" / "molt" / "LICENSE")
    wheels_dir = root / "share" / "molt" / "wheels"
    wheels_dir.mkdir(parents=True, exist_ok=True)
    _copy_file(wheel, wheels_dir / wheel.name)
    if worker_bin is not None:
        _copy_file(worker_bin, root / "bin" / worker_bin.name)


def _bundle_worker(root: Path, worker_bin: Path) -> None:
    _copy_file(worker_bin, root / "bin" / worker_bin.name)
    _copy_file(ROOT / "LICENSE", root / "share" / "molt" / "LICENSE")


def _archive_tar(root_dir: Path, out_path: Path) -> None:
    with tarfile.open(out_path, "w:gz") as tar:
        tar.add(root_dir, arcname=root_dir.name)


def _archive_zip(root_dir: Path, out_path: Path) -> None:
    with zipfile.ZipFile(out_path, "w", compression=zipfile.ZIP_DEFLATED) as zf:
        for path in root_dir.rglob("*"):
            zf.write(path, path.relative_to(root_dir.parent))


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--version", required=True)
    parser.add_argument(
        "--platform", choices=["macos", "linux", "windows"], required=True
    )
    parser.add_argument("--arch", required=True)
    parser.add_argument("--wheel", required=False)
    parser.add_argument("--worker", required=False)
    parser.add_argument("--kind", choices=["molt", "molt-worker"], default="molt")
    parser.add_argument("--output", required=True)
    args = parser.parse_args()

    if args.kind == "molt" and not args.wheel:
        raise SystemExit("--wheel is required for molt bundles")
    if args.kind == "molt-worker" and not args.worker:
        raise SystemExit("--worker is required for molt-worker bundles")

    wheel = Path(args.wheel) if args.wheel else None
    worker_bin = Path(args.worker) if args.worker else None

    bundle_name = args.kind
    root_name = _bundle_root(bundle_name, args.version)

    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)
        root_dir = tmp_path / root_name
        root_dir.mkdir(parents=True, exist_ok=True)

        if args.kind == "molt":
            _bundle_molt(root_dir, wheel, worker_bin)
            if args.platform == "windows":
                _make_windows_wrapper(root_dir)
            else:
                _make_unix_wrapper(root_dir / "bin" / "molt")
        else:
            _bundle_worker(root_dir, worker_bin)

        out_path = Path(args.output)
        out_path.parent.mkdir(parents=True, exist_ok=True)

        if args.platform == "windows":
            _archive_zip(root_dir, out_path)
        else:
            _archive_tar(root_dir, out_path)


if __name__ == "__main__":
    main()
