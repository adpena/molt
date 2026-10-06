#!/usr/bin/env python3
"""Build byte-reproducible Molt release bundles."""

from __future__ import annotations

from molt.temporary_artifacts import OwnedTemporaryDirectory

import argparse
import gzip
import json
import os
from pathlib import Path
import shutil
import tarfile
from pathlib import PurePosixPath
from typing import Any, Callable

from .archive import ArchivePolicy, write_reproducible_zip
from .compiler_payload import (
    compiler_record,
    launcher_record,
    materialize_sources,
)
from .binary_compatibility import (
    WheelCompatibilityError,
    audit_wheel,
    derive_bundle_wheel_compatibility,
)
from .git_source_snapshot import GitSourceSnapshot
from .platform_wheel import write_platform_wheel
from .runtime_cells import read_runtime_inventory
from molt.compiler_distribution import (
    COMPILER_BUNDLE_DIRECTORIES,
    MAX_SOURCE_FILES,
    RUNTIME_ROOT,
    verify_runtime_tree,
)


ROOT = Path(__file__).resolve().parents[2]
RELEASE_BUNDLE_ARCHIVE_POLICY = ArchivePolicy(max_members=MAX_SOURCE_FILES * 2)


def _copy_file(src: Path, dst: Path, *, executable: bool = False) -> None:
    if not src.is_file():
        raise ValueError(f"release input is not a file: {src}")
    dst.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(src, dst)
    dst.chmod(0o755 if executable else 0o644)


def _verify_runtime_cell_semantics(
    runtime: dict[str, Any], runtime_cells: Path, source: Path
) -> None:
    """Admit every cell's canonical receipts and bind them to this bundle source.

    Receipts pass the installed admission rules (content, parsers, key
    agreement), and each identity's ``compile.sources`` must equal the runtime
    source tree identity of the bundle's own source, hashed once for all cells.
    """
    from molt.cli.installed_runtime import admit_runtime_cell_receipts
    from molt.cli.runtime_build_identity import verify_runtime_source_identities

    verify_runtime_source_identities(
        source,
        [
            identity
            for cell in runtime["cells"]
            for identity in admit_runtime_cell_receipts(
                cell, runtime_cells / cell["id"]
            )
        ],
    )


def _project_platform_wheel(
    root_dir: Path,
    pure_wheel: Path,
    output_dir: Path,
    *,
    platform: str,
    arch: str,
    source_date_epoch: int,
    bundle_mode: Callable[[PurePosixPath], int],
) -> Path:
    """Tag from the bundle's own binaries, write, and re-audit the written wheel.

    An unsatisfied policy leaves the already-written bundle archive in place
    and records the audit tool's evidence beside it for repair.
    """
    try:
        derived = derive_bundle_wheel_compatibility(
            root_dir, platform=platform, arch=arch
        )
        wheel = write_platform_wheel(
            root_dir,
            pure_wheel,
            output_dir,
            platform=platform,
            arch=arch,
            platform_tag=derived.tag,
            source_date_epoch=source_date_epoch,
            bundle_mode=bundle_mode,
            policy=RELEASE_BUNDLE_ARCHIVE_POLICY,
        )
        written = audit_wheel(wheel, platform=platform, arch=arch)
        if written.tag != derived.tag:
            raise WheelCompatibilityError(
                "the written platform wheel audits differently from its bundle",
                {"bundle": dict(derived.evidence), "wheel": dict(written.evidence)},
            )
        return wheel
    except WheelCompatibilityError as exc:
        output_dir.mkdir(parents=True, exist_ok=True)
        (output_dir / "wheel-compatibility-failure.json").write_text(
            json.dumps(
                {
                    "platform": platform,
                    "arch": arch,
                    "error": str(exc),
                    "evidence": exc.evidence,
                },
                indent=2,
                sort_keys=True,
                default=str,
            )
            + "\n",
            encoding="utf-8",
        )
        raise


def _bundle_molt(
    root: Path,
    *,
    compiler: Path,
    launcher: Path,
    snapshot: GitSourceSnapshot,
    runtime_cells: Path,
    platform: str,
    arch: str,
) -> None:
    record = compiler_record(compiler, platform=platform, arch=arch)
    entry = launcher_record(launcher, platform=platform, arch=arch)
    runtime = read_runtime_inventory(runtime_cells, platform=platform, arch=arch)
    if runtime["source"] != {
        "object_format": snapshot.object_format,
        "commit": snapshot.source_sha,
        "tree": snapshot.tree_sha,
    }:
        raise ValueError("runtime cells were not produced from the bundle source")
    source = materialize_sources(
        root,
        repo_root=ROOT,
        snapshot=snapshot,
        compiler=record,
        launcher=entry,
        runtime=runtime,
    )
    _copy_file(compiler, root / record["path"], executable=True)
    _copy_file(launcher, root / entry["path"], executable=True)
    for cell in runtime["cells"]:
        for member in cell["files"]:
            _copy_file(
                runtime_cells / cell["id"] / member["name"],
                root / RUNTIME_ROOT / cell["id"] / member["name"],
            )
    verify_runtime_tree(root / RUNTIME_ROOT, runtime)
    _verify_runtime_cell_semantics(runtime, root / RUNTIME_ROOT, source)
    _copy_file(
        source / "packaging" / "INSTALL.md", root / "share" / "molt" / "INSTALL.md"
    )
    _copy_file(source / "LICENSE", root / "share" / "molt" / "LICENSE")
    if {entry.name for entry in root.iterdir()} != set(
        COMPILER_BUNDLE_DIRECTORIES
    ) or not all((root / name).is_dir() for name in COMPILER_BUNDLE_DIRECTORIES):
        raise ValueError("Compiler bundle directory projection is not exact")


def _bundle_worker(root: Path, worker_bin: Path) -> None:
    _copy_file(worker_bin, root / "bin" / worker_bin.name, executable=True)
    _copy_file(ROOT / "LICENSE", root / "share" / "molt-worker" / "LICENSE")


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
    worker: Path | None,
    kind: str,
    output: Path,
    source_date_epoch: int,
    arch: str,
    compiler: Path | None = None,
    launcher: Path | None = None,
    snapshot: GitSourceSnapshot | None = None,
    runtime_cells: Path | None = None,
    pure_wheel: Path | None = None,
    wheel_output_dir: Path | None = None,
) -> Path | None:
    """Write one bundle archive; a Molt bundle may also project its pip wheel.

    The platform wheel is written after the archive, from the same assembled
    tree and mode authority, so pip and archive installs carry one
    distribution. Returns that wheel's path when requested.
    """
    if source_date_epoch <= 0:
        raise ValueError("source date epoch must be positive")
    if kind == "molt" and (
        compiler is None
        or launcher is None
        or snapshot is None
        or runtime_cells is None
    ):
        raise ValueError(
            "production compiler, launcher, runtime cells and committed source "
            "are required for molt bundles"
        )
    if (pure_wheel is None) != (wheel_output_dir is None) or (
        pure_wheel is not None and kind != "molt"
    ):
        raise ValueError("a platform wheel needs a molt bundle, pure wheel and output")
    if kind == "molt-worker" and worker is None:
        raise ValueError("worker is required for molt-worker bundles")
    if platform not in {"macos", "linux", "windows"}:
        raise ValueError(f"unsupported release platform: {platform}")

    with OwnedTemporaryDirectory() as temporary:
        root_dir = Path(temporary) / f"{kind}-{version}"
        root_dir.mkdir(parents=True)
        if kind == "molt":
            assert (
                compiler is not None
                and launcher is not None
                and snapshot is not None
                and runtime_cells is not None
            )
            _bundle_molt(
                root_dir,
                compiler=compiler,
                launcher=launcher,
                snapshot=snapshot,
                runtime_cells=runtime_cells,
                platform=platform,
                arch=arch,
            )
        else:
            assert worker is not None
            _bundle_worker(root_dir, worker)

        output.parent.mkdir(parents=True, exist_ok=True)
        source_modes = snapshot.mode_map if snapshot is not None else {}

        def bundle_mode(relative: PurePosixPath) -> int:
            return (
                source_modes.get(relative.as_posix().removeprefix("source/"), 0o644)
                if snapshot is not None and relative.parts[0] == "source"
                else _normalized_mode(root_dir / relative)
            )

        if platform == "windows":
            write_reproducible_zip(
                root_dir,
                output,
                source_date_epoch=source_date_epoch,
                prefix=root_dir.name,
                mode_resolver=bundle_mode,
                policy=RELEASE_BUNDLE_ARCHIVE_POLICY,
            )
        else:
            _archive_tar(root_dir, output, source_date_epoch, source_modes)
        if pure_wheel is None:
            return None
        assert wheel_output_dir is not None
        return _project_platform_wheel(
            root_dir,
            pure_wheel,
            wheel_output_dir,
            platform=platform,
            arch=arch,
            source_date_epoch=source_date_epoch,
            bundle_mode=bundle_mode,
        )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--version", required=True)
    parser.add_argument(
        "--platform", choices=["macos", "linux", "windows"], required=True
    )
    parser.add_argument("--arch", required=True)
    parser.add_argument("--worker", type=Path)
    parser.add_argument("--compiler", type=Path)
    parser.add_argument("--launcher", type=Path)
    parser.add_argument("--runtime-cells", type=Path)
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
        worker=args.worker,
        kind=args.kind,
        output=args.output,
        source_date_epoch=args.source_date_epoch,
        arch=args.arch,
        compiler=args.compiler,
        launcher=args.launcher,
        runtime_cells=args.runtime_cells,
        snapshot=source_snapshot(ROOT, args.source_sha) if args.source_sha else None,
    )


if __name__ == "__main__":
    main()
