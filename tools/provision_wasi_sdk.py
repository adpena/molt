#!/usr/bin/env python3
"""Provision the exact manifest-owned wasi-sdk for this host under custody.

This is the only installer for Molt's WebAssembly tools. It selects the host
asset from ``config/llvm_toolchain_releases.toml``, reuses a cached archive only
when its size and SHA-256 match, admits every archive member before bounded
extraction, and atomically publishes one identity-addressed installation with
its provision receipt. Build and readiness paths only discover that result;
``python -m molt.llvm_toolchain --verify-wasm --wasi-sdk <install>`` verifies it.
"""

from __future__ import annotations

import argparse
from dataclasses import asdict
import hashlib
from pathlib import Path, PurePosixPath, PureWindowsPath
import sys
import tarfile
import tempfile
import urllib.request


ROOT = Path(__file__).resolve().parents[1]
SRC_ROOT = ROOT / "src"
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

from molt import llvm_toolchain  # noqa: E402
from molt.file_publication import (  # noqa: E402
    durable_publish_directory_exclusive,
    durable_replace,
    staged_file_path,
)
from molt.portable_paths import (  # noqa: E402
    portable_path_identity,
    portable_relative_path,
)
from molt.tool_releases import DOWNLOADS_DIRNAME  # noqa: E402
from molt.toolchain_identity import stable_file_sha256  # noqa: E402
from molt.wasi_sdk_identity import (  # noqa: E402
    INSTALL_RECEIPT_FILENAME,
    MAX_TREE_BYTES,
    MAX_TREE_ENTRIES,
    SDK_DIRNAME,
    SDK_TOOL_NAMES,
    executable_filename,
    read_wasi_sdk_version_identity,
    render_wasi_sdk_install_receipt,
    wasi_sdk_tree_identity,
)


DOWNLOAD_CHUNK_BYTES = 1024 * 1024


class WasiSdkProvisionError(ValueError):
    """Raised when the host SDK cannot be provisioned exactly."""


def _download(url: str, output: Path, *, size: int, sha256: str) -> None:
    digest = hashlib.sha256()
    observed_size = 0
    with (
        output.open("xb") as stream,
        urllib.request.urlopen(url, timeout=120) as response,  # noqa: S310
    ):
        while chunk := response.read(DOWNLOAD_CHUNK_BYTES):
            observed_size += len(chunk)
            if observed_size > size:
                raise WasiSdkProvisionError(
                    "WASI SDK download exceeds its manifest size"
                )
            stream.write(chunk)
            digest.update(chunk)
    if observed_size != size or digest.hexdigest() != sha256:
        raise WasiSdkProvisionError(
            "WASI SDK download differs from its manifest identity"
        )


def _archive_matches(archive: Path, asset: llvm_toolchain.WasiSdkHostAsset) -> bool:
    return (
        archive.is_file()
        and not archive.is_symlink()
        and archive.stat().st_size == asset.size
        and stable_file_sha256(archive, label="WASI SDK archive") == asset.sha256
    )


def cached_archive(asset: llvm_toolchain.WasiSdkHostAsset, downloads: Path) -> Path:
    """Return the verified archive for ``asset``, downloading it only on a miss.

    The cache entry is content-addressed by its manifest digest: a mismatched
    file is never reused and is replaced only by a fully verified download.
    """

    llvm_toolchain.reject_poison_toolchain_path(
        downloads, authority="WASI SDK download cache"
    )
    downloads.mkdir(parents=True, exist_ok=True)
    archive = downloads / f"{asset.archive_root}-{asset.sha256}.tar.gz"
    if _archive_matches(archive, asset):
        return archive
    staged = staged_file_path(archive, purpose="download")
    try:
        _download(asset.url, staged, size=asset.size, sha256=asset.sha256)
        durable_replace(staged, archive)
    finally:
        staged.unlink(missing_ok=True)
    return archive


def _validated_link_target(
    member: tarfile.TarInfo,
    relative: PurePosixPath,
    *,
    expected_root: str,
) -> None:
    raw_target = member.linkname
    target = PurePosixPath(raw_target)
    if (
        not raw_target
        or "\\" in raw_target
        or "\x00" in raw_target
        or target.is_absolute()
        or PureWindowsPath(raw_target).drive
    ):
        raise WasiSdkProvisionError(
            f"WASI SDK archive link escapes its root: {member.name}"
        )
    # Symbolic links resolve beside the member; hard links name an archive path.
    base = relative.parent if member.issym() else PurePosixPath()
    resolved_parts: list[str] = []
    for part in (base / target).parts:
        if part == "..":
            if not resolved_parts:
                raise WasiSdkProvisionError(
                    f"WASI SDK archive link escapes its root: {member.name}"
                )
            resolved_parts.pop()
        elif part not in {"", "."}:
            resolved_parts.append(part)
    try:
        resolved = portable_relative_path(PurePosixPath(*resolved_parts).as_posix())
    except ValueError as exc:
        raise WasiSdkProvisionError(
            f"WASI SDK archive link escapes its root: {member.name}"
        ) from exc
    if not resolved.parts or resolved.parts[0] != expected_root:
        raise WasiSdkProvisionError(
            f"WASI SDK archive link escapes its root: {member.name}"
        )


def _validate_archive(archive: tarfile.TarFile, *, expected_root: str) -> None:
    identities: set[str] = set()
    total_size = 0
    for index, member in enumerate(archive, start=1):
        if index > MAX_TREE_ENTRIES:
            raise WasiSdkProvisionError("WASI SDK archive member count is invalid")
        member_name = (
            member.name[:-1]
            if member.isdir() and member.name.endswith("/")
            else member.name
        )
        try:
            relative = portable_relative_path(member_name)
            identity = portable_path_identity(relative.as_posix())
        except ValueError as exc:
            raise WasiSdkProvisionError(
                "WASI SDK archive member is not a portable relative path: "
                f"{member.name}"
            ) from exc
        if relative.parts[0] != expected_root:
            raise WasiSdkProvisionError(
                f"WASI SDK archive member is outside {expected_root}: {member.name}"
            )
        if identity in identities:
            raise WasiSdkProvisionError(
                f"WASI SDK archive has a portable path collision: {member.name}"
            )
        identities.add(identity)
        if not (member.isfile() or member.isdir() or member.issym() or member.islnk()):
            raise WasiSdkProvisionError(
                f"WASI SDK archive contains a special node: {member.name}"
            )
        if member.isfile():
            total_size += member.size
            if member.size < 0 or total_size > MAX_TREE_BYTES:
                raise WasiSdkProvisionError(
                    "WASI SDK archive exceeds its extracted-size policy"
                )
        if member.issym() or member.islnk():
            _validated_link_target(member, relative, expected_root=expected_root)
    if not identities:
        raise WasiSdkProvisionError("WASI SDK archive member count is invalid")


def _stage_installation(
    archive_path: Path,
    asset: llvm_toolchain.WasiSdkHostAsset,
    staged: Path,
) -> None:
    """Build the complete installation layout before anything is published."""

    staged.mkdir()
    extracted = staged / ".extract"
    extracted.mkdir()
    with tarfile.open(archive_path, "r:gz") as archive:
        _validate_archive(archive, expected_root=asset.archive_root)
        archive.extractall(extracted, filter="data")
    sdk = staged / SDK_DIRNAME
    (extracted / asset.archive_root).replace(sdk)
    extracted.rmdir()
    required = (
        sdk / "VERSION",
        *(sdk / "bin" / executable_filename(name, asset.id) for name in SDK_TOOL_NAMES),
        sdk / "share" / "wasi-sysroot" / "include" / "wasm32-wasip1" / "errno.h",
        sdk / "share" / "wasi-sysroot" / "lib" / "wasm32-wasip1" / "libc.a",
    )
    missing = [path for path in required if not path.is_file()]
    if missing:
        raise WasiSdkProvisionError(
            "WASI SDK archive is missing required assets: "
            + ", ".join(path.relative_to(sdk).as_posix() for path in missing)
        )
    version_identity = read_wasi_sdk_version_identity(sdk / "VERSION")
    if version_identity.sdk_version != asset.sdk_version:
        raise WasiSdkProvisionError(
            "WASI SDK VERSION identity differs from the exact host asset: "
            f"expected {asset.sdk_version!r}, found {version_identity.sdk_version!r}"
        )
    if version_identity.llvm_version != asset.llvm_version:
        raise WasiSdkProvisionError(
            "WASI SDK LLVM producer identity differs from the exact host asset: "
            f"expected {asset.llvm_version!r}, found {version_identity.llvm_version!r}"
        )
    tree_identity = wasi_sdk_tree_identity(sdk)
    (staged / INSTALL_RECEIPT_FILENAME).write_text(
        render_wasi_sdk_install_receipt(asdict(asset), tree_identity),
        encoding="utf-8",
        newline="",
    )


def _admit_existing(root: Path, prefix: Path) -> Path:
    try:
        return llvm_toolchain.load_wasi_sdk_installation(
            root, prefix, verify_tree=True
        ).prefix
    except llvm_toolchain.LlvmToolchainConfigError as exc:
        raise WasiSdkProvisionError(
            "existing WASI SDK installation is not its exact provisioned identity: "
            f"{exc}; remove {prefix} explicitly before provisioning again"
        ) from exc


def provision_wasi_sdk(
    toolchain_root: Path,
    *,
    downloads: Path | None = None,
    root: Path = ROOT,
) -> Path:
    """Install this host's exact SDK once and return its install prefix.

    An existing identity-addressed installation is reused only after its whole
    tree matches its receipt; it is never repaired or replaced in place.
    """

    llvm_toolchain.reject_poison_toolchain_path(
        toolchain_root, authority="WASI SDK toolchain root"
    )
    asset = llvm_toolchain.wasi_sdk_host_asset(root)
    prefix = llvm_toolchain.wasi_sdk_install_prefix(
        toolchain_root.expanduser().absolute(), asset
    )
    if prefix.exists() or prefix.is_symlink():
        return _admit_existing(root, prefix)
    archive = cached_archive(
        asset,
        prefix.parent / DOWNLOADS_DIRNAME if downloads is None else downloads,
    )
    prefix.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(
        prefix=".molt-wasi-sdk-", dir=prefix.parent
    ) as raw:
        staged = Path(raw) / prefix.name
        _stage_installation(archive, asset, staged)
        try:
            durable_publish_directory_exclusive(staged, prefix)
        except FileExistsError:
            # A concurrent provisioner published the same identity first.
            return _admit_existing(root, prefix)
    return prefix.resolve(strict=True)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Provision the manifest-owned wasi-sdk for this host."
    )
    parser.add_argument(
        "--toolchain-root",
        type=Path,
        default=None,
        help="Toolchain custody root (default: this checkout's custody root).",
    )
    parser.add_argument(
        "--downloads",
        type=Path,
        default=None,
        help="Verified archive cache (default: <toolchain-root>/toolchains/downloads).",
    )
    parser.add_argument(
        "--github-output",
        type=Path,
        default=None,
        help="Append install/sdk/sysroot paths to a GitHub Actions output file.",
    )
    args = parser.parse_args(argv)
    try:
        toolchain_root = args.toolchain_root
        if toolchain_root is None:
            from molt.dx import checkout_custody

            toolchain_root = checkout_custody(ROOT).toolchain_root
        prefix = provision_wasi_sdk(toolchain_root, downloads=args.downloads)
        installation = llvm_toolchain.load_wasi_sdk_installation(
            ROOT, prefix, verify_tree=False
        )
    except (OSError, RuntimeError, ValueError, tarfile.TarError) as exc:
        print(f"WASI SDK provisioning failed: {exc}", file=sys.stderr)
        return 2
    if args.github_output is not None:
        with args.github_output.open("a", encoding="utf-8", newline="\n") as handle:
            handle.write(f"install={installation.prefix}\n")
            handle.write(f"sdk={installation.sdk}\n")
            handle.write(f"sysroot={installation.sysroot}\n")
    print(installation.prefix)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
