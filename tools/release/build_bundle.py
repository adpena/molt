#!/usr/bin/env python3
"""Build byte-reproducible Molt release bundles."""

from __future__ import annotations

import argparse
import datetime as dt
import gzip
import os
from pathlib import Path
import shutil
import tarfile
import tempfile
import zipfile


ROOT = Path(__file__).resolve().parents[2]
MIN_ZIP_EPOCH = 315532800  # 1980-01-01, the earliest ZIP timestamp.


def _write_text(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8", newline="")


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
    echo "molt: Python 3.12+ not found" >&2
    exit 1
  fi
fi
exec "$PYTHON_BIN" "$ROOT/lib/molt/bootstrap.py" "$@"
"""
    _write_text(path, script)
    path.chmod(0o755)


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
        '  py -3 "%BOOT%" %*\r\n'
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
        "  py -3 $boot @args\n"
        "} else {\n"
        "  python $boot @args\n"
        "}\n"
    )
    _write_text(root / "bin" / "molt.cmd", cmd)
    _write_text(root / "bin" / "molt.ps1", ps1)


def _copy_file(src: Path, dst: Path, *, executable: bool = False) -> None:
    if not src.is_file():
        raise ValueError(f"release input is not a file: {src}")
    dst.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(src, dst)
    dst.chmod(0o755 if executable else 0o644)


def _bundle_molt(root: Path, wheel: Path, worker_bin: Path | None) -> None:
    _copy_file(
        ROOT / "packaging" / "bootstrap.py", root / "lib" / "molt" / "bootstrap.py"
    )
    _copy_file(
        ROOT / "packaging" / "INSTALL.md", root / "share" / "molt" / "INSTALL.md"
    )
    _copy_file(ROOT / "LICENSE", root / "share" / "molt" / "LICENSE")
    _copy_file(wheel, root / "share" / "molt" / "wheels" / wheel.name)
    if worker_bin is not None:
        _copy_file(worker_bin, root / "bin" / worker_bin.name, executable=True)


def _bundle_worker(root: Path, worker_bin: Path) -> None:
    _copy_file(worker_bin, root / "bin" / worker_bin.name, executable=True)
    _copy_file(ROOT / "LICENSE", root / "share" / "molt" / "LICENSE")


def _normalized_mode(path: Path) -> int:
    return 0o755 if path.is_dir() or path.parent.name == "bin" else 0o644


def _archive_tar(root_dir: Path, out_path: Path, epoch: int) -> None:
    with out_path.open("wb") as raw:
        with gzip.GzipFile(
            filename="", mode="wb", fileobj=raw, compresslevel=9, mtime=epoch
        ) as compressed:
            with tarfile.open(
                fileobj=compressed, mode="w", format=tarfile.GNU_FORMAT
            ) as tar:
                for path in (root_dir, *sorted(root_dir.rglob("*"))):
                    arcname = path.relative_to(root_dir.parent).as_posix()
                    info = tar.gettarinfo(str(path), arcname)
                    if not (info.isdir() or info.isfile()):
                        raise ValueError(
                            f"release bundles cannot contain special files: {path}"
                        )
                    info.uid = 0
                    info.gid = 0
                    info.uname = "root"
                    info.gname = "root"
                    info.mtime = epoch
                    info.mode = _normalized_mode(path)
                    if info.isfile():
                        with path.open("rb") as handle:
                            tar.addfile(info, handle)
                    else:
                        tar.addfile(info)


def _archive_zip(root_dir: Path, out_path: Path, epoch: int) -> None:
    timestamp = dt.datetime.fromtimestamp(max(epoch, MIN_ZIP_EPOCH), tz=dt.UTC)
    date_time = (
        timestamp.year,
        timestamp.month,
        timestamp.day,
        timestamp.hour,
        timestamp.minute,
        timestamp.second,
    )
    with zipfile.ZipFile(
        out_path, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9
    ) as archive:
        for path in sorted(root_dir.rglob("*")):
            if path.is_dir():
                continue
            arcname = path.relative_to(root_dir.parent).as_posix()
            info = zipfile.ZipInfo(arcname, date_time=date_time)
            info.create_system = 3
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = (_normalized_mode(path) & 0xFFFF) << 16
            archive.writestr(
                info, path.read_bytes(), compress_type=zipfile.ZIP_DEFLATED
            )


def build_bundle(
    *,
    version: str,
    platform: str,
    wheel: Path | None,
    worker: Path | None,
    kind: str,
    output: Path,
    source_date_epoch: int,
) -> None:
    if source_date_epoch <= 0:
        raise ValueError("source date epoch must be positive")
    if kind == "molt" and wheel is None:
        raise ValueError("wheel is required for molt bundles")
    if kind == "molt-worker" and worker is None:
        raise ValueError("worker is required for molt-worker bundles")
    if platform not in {"macos", "linux", "windows"}:
        raise ValueError(f"unsupported release platform: {platform}")

    with tempfile.TemporaryDirectory() as temporary:
        root_dir = Path(temporary) / f"{kind}-{version}"
        root_dir.mkdir(parents=True)
        if kind == "molt":
            assert wheel is not None
            _bundle_molt(root_dir, wheel, worker)
            if platform == "windows":
                _make_windows_wrapper(root_dir)
            else:
                _make_unix_wrapper(root_dir / "bin" / "molt")
        else:
            assert worker is not None
            _bundle_worker(root_dir, worker)

        output.parent.mkdir(parents=True, exist_ok=True)
        if platform == "windows":
            _archive_zip(root_dir, output, source_date_epoch)
        else:
            _archive_tar(root_dir, output, source_date_epoch)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--version", required=True)
    parser.add_argument(
        "--platform", choices=["macos", "linux", "windows"], required=True
    )
    parser.add_argument("--arch", required=True)
    parser.add_argument("--wheel", type=Path)
    parser.add_argument("--worker", type=Path)
    parser.add_argument("--kind", choices=["molt", "molt-worker"], default="molt")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--source-date-epoch",
        type=int,
        default=int(os.environ.get("SOURCE_DATE_EPOCH", "0")),
    )
    args = parser.parse_args()
    build_bundle(
        version=args.version,
        platform=args.platform,
        wheel=args.wheel,
        worker=args.worker,
        kind=args.kind,
        output=args.output,
        source_date_epoch=args.source_date_epoch,
    )


if __name__ == "__main__":
    main()
