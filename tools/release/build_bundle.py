#!/usr/bin/env python3
"""Build byte-reproducible Molt release bundles."""

from __future__ import annotations

import argparse
import gzip
import os
from pathlib import Path
import shutil
import tarfile
import tempfile

from .archive import ArchivePolicy, write_reproducible_zip
from .compiler_payload import (
    compiler_record,
    materialize_sources,
)
from .git_source_snapshot import GitSourceSnapshot
from molt.compiler_distribution import MAX_SOURCE_FILES


ROOT = Path(__file__).resolve().parents[2]
RELEASE_BUNDLE_ARCHIVE_POLICY = ArchivePolicy(max_members=MAX_SOURCE_FILES * 2)


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
exec "$PYTHON_BIN" -I -B "$ROOT/lib/molt/bootstrap.py" "$@"
"""
    _write_text(path, script)
    path.chmod(0o755)


def _make_windows_wrapper(root: Path) -> None:
    cmd = (
        "@echo off\r\n"
        "setlocal DisableDelayedExpansion\r\n"
        'set "ROOT=%~dp0.."\r\n'
        'if not defined MOLT_HOME set "MOLT_HOME=%USERPROFILE%\\.molt"\r\n'
        'if not defined MOLT_PROJECT_ROOT set "MOLT_PROJECT_ROOT=%CD%"\r\n'
        'set "BOOT=%ROOT%\\lib\\molt\\bootstrap.py"\r\n'
        'if not exist "%BOOT%" (\r\n'
        "  echo molt: bootstrap not found at %BOOT%\r\n"
        "  exit /b 1\r\n"
        ")\r\n"
        "if defined PYTHON goto selected\r\n"
        "where py >nul 2>nul\r\n"
        "if errorlevel 1 goto python\r\n"
        'py -3 -I -B "%BOOT%" %*\r\n'
        "exit /b %ERRORLEVEL%\r\n"
        ":python\r\n"
        'python -I -B "%BOOT%" %*\r\n'
        "exit /b %ERRORLEVEL%\r\n"
        ":selected\r\n"
        '"%PYTHON%" -I -B "%BOOT%" %*\r\n'
        "exit /b %ERRORLEVEL%\r\n"
    )
    ps1 = (
        '$ErrorActionPreference = "Stop"\n'
        "$root = Split-Path -Parent $MyInvocation.MyCommand.Path\n"
        '$root = (Resolve-Path -LiteralPath (Join-Path $root "..")).Path\n'
        'if (-not $env:MOLT_HOME) { $env:MOLT_HOME = Join-Path $env:USERPROFILE ".molt" }\n'
        "if (-not $env:MOLT_PROJECT_ROOT) { $env:MOLT_PROJECT_ROOT = (Get-Location).Path }\n"
        '$boot = Join-Path $root "lib\\molt\\bootstrap.py"\n'
        'if (-not (Test-Path -LiteralPath $boot)) { throw "molt: bootstrap not found at $boot" }\n'
        "if ($env:PYTHON) {\n"
        "  & $env:PYTHON -I -B $boot @args\n"
        "} elseif (Get-Command py -ErrorAction SilentlyContinue) {\n"
        "  py -3 -I -B $boot @args\n"
        "} else {\n"
        "  python -I -B $boot @args\n"
        "}\n"
        "exit $LASTEXITCODE\n"
    )
    _write_text(root / "bin" / "molt.cmd", cmd)
    _write_text(root / "bin" / "molt.ps1", ps1)


def _copy_file(src: Path, dst: Path, *, executable: bool = False) -> None:
    if not src.is_file():
        raise ValueError(f"release input is not a file: {src}")
    dst.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(src, dst)
    dst.chmod(0o755 if executable else 0o644)


def _bundle_molt(
    root: Path,
    wheel: Path,
    worker_bin: Path | None,
    *,
    compiler: Path,
    snapshot: GitSourceSnapshot,
    platform: str,
    arch: str,
) -> None:
    record = compiler_record(compiler, platform=platform, arch=arch)
    source = materialize_sources(
        root, repo_root=ROOT, snapshot=snapshot, compiler=record, wheel=wheel
    )
    _copy_file(compiler, root / record["path"], executable=True)
    _copy_file(
        source / "packaging" / "bootstrap.py", root / "lib" / "molt" / "bootstrap.py"
    )
    _copy_file(
        source / "packaging" / "INSTALL.md", root / "share" / "molt" / "INSTALL.md"
    )
    _copy_file(source / "LICENSE", root / "share" / "molt" / "LICENSE")
    _copy_file(wheel, root / "share" / "molt" / "wheels" / wheel.name)
    if worker_bin is not None:
        _copy_file(worker_bin, root / "bin" / worker_bin.name, executable=True)


def _bundle_worker(root: Path, worker_bin: Path) -> None:
    _copy_file(worker_bin, root / "bin" / worker_bin.name, executable=True)
    _copy_file(ROOT / "LICENSE", root / "share" / "molt" / "LICENSE")


def _normalized_mode(path: Path) -> int:
    return 0o755 if path.is_dir() or path.parent.name == "bin" else 0o644


def _archive_tar(
    root_dir: Path, out_path: Path, epoch: int, source_modes: dict[str, int]
) -> None:
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
                    relative = path.relative_to(root_dir).as_posix()
                    info.mode = (
                        source_modes.get(
                            relative.removeprefix("source/"), _normalized_mode(path)
                        )
                        if relative.startswith("source/")
                        else _normalized_mode(path)
                    )
                    if info.isfile():
                        with path.open("rb") as handle:
                            tar.addfile(info, handle)
                    else:
                        tar.addfile(info)


def build_bundle(
    *,
    version: str,
    platform: str,
    wheel: Path | None,
    worker: Path | None,
    kind: str,
    output: Path,
    source_date_epoch: int,
    arch: str,
    compiler: Path | None = None,
    snapshot: GitSourceSnapshot | None = None,
) -> None:
    if source_date_epoch <= 0:
        raise ValueError("source date epoch must be positive")
    if kind == "molt" and wheel is None:
        raise ValueError("wheel is required for molt bundles")
    if kind == "molt" and (compiler is None or snapshot is None):
        raise ValueError(
            "production compiler and committed source are required for molt bundles"
        )
    if kind == "molt-worker" and worker is None:
        raise ValueError("worker is required for molt-worker bundles")
    if platform not in {"macos", "linux", "windows"}:
        raise ValueError(f"unsupported release platform: {platform}")

    with tempfile.TemporaryDirectory() as temporary:
        root_dir = Path(temporary) / f"{kind}-{version}"
        root_dir.mkdir(parents=True)
        if kind == "molt":
            assert wheel is not None
            assert compiler is not None and snapshot is not None
            _bundle_molt(
                root_dir,
                wheel,
                worker,
                compiler=compiler,
                snapshot=snapshot,
                platform=platform,
                arch=arch,
            )
            if platform == "windows":
                _make_windows_wrapper(root_dir)
            else:
                _make_unix_wrapper(root_dir / "bin" / "molt")
        else:
            assert worker is not None
            _bundle_worker(root_dir, worker)

        output.parent.mkdir(parents=True, exist_ok=True)
        source_modes = snapshot.mode_map if snapshot is not None else {}
        if platform == "windows":
            write_reproducible_zip(
                root_dir,
                output,
                source_date_epoch=source_date_epoch,
                prefix=root_dir.name,
                mode_resolver=lambda relative: (
                    source_modes.get(relative.as_posix().removeprefix("source/"), 0o644)
                    if snapshot is not None and relative.parts[0] == "source"
                    else _normalized_mode(root_dir / relative)
                ),
                policy=RELEASE_BUNDLE_ARCHIVE_POLICY,
            )
        else:
            _archive_tar(root_dir, output, source_date_epoch, source_modes)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--version", required=True)
    parser.add_argument(
        "--platform", choices=["macos", "linux", "windows"], required=True
    )
    parser.add_argument("--arch", required=True)
    parser.add_argument("--wheel", type=Path)
    parser.add_argument("--worker", type=Path)
    parser.add_argument("--compiler", type=Path)
    parser.add_argument("--source-sha")
    parser.add_argument("--kind", choices=["molt", "molt-worker"], default="molt")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--source-date-epoch",
        type=int,
        default=int(os.environ.get("SOURCE_DATE_EPOCH", "0")),
    )
    args = parser.parse_args()
    from .compiler_payload import source_snapshot

    build_bundle(
        version=args.version,
        platform=args.platform,
        wheel=args.wheel,
        worker=args.worker,
        kind=args.kind,
        output=args.output,
        source_date_epoch=args.source_date_epoch,
        arch=args.arch,
        compiler=args.compiler,
        snapshot=source_snapshot(ROOT, args.source_sha) if args.source_sha else None,
    )


if __name__ == "__main__":
    main()
