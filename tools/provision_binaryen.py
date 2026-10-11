#!/usr/bin/env python3
"""Provision the exact manifest-owned Binaryen asset for the current host."""

from __future__ import annotations

import argparse
from dataclasses import asdict, dataclass, field
import hashlib
import json
import os
from pathlib import Path
import stat
import tarfile
import tempfile
import urllib.request

from molt.binaryen_identity import (
    INSTALL_RECEIPT_FILENAME,
    INSTALL_RECEIPT_SCHEMA,
    MAX_TREE_BYTES,
    MAX_TREE_ENTRIES,
    BinaryenInstallationIdentity,
    BinaryenTreeEntry,
    BinaryenTreeIdentity,
    binaryen_installation_identity,
    binaryen_tree_identity_from_entries,
    load_binaryen_install_receipt,
    read_binaryen_version,
    validate_binaryen_install_receipt,
)
from molt.binaryen_toolchain import BinaryenHostAsset, binaryen_host_asset
from molt.file_publication import durable_publish_directory_exclusive
from molt.portable_paths import portable_path_identity, portable_relative_path


ROOT = Path(__file__).resolve().parents[1]


@dataclass(frozen=True, slots=True)
class _BinaryenArchiveIdentity:
    tree: BinaryenTreeIdentity
    executable_sha256: str


@dataclass(slots=True)
class _BinaryenArchiveValidator:
    expected_root: str
    executable: str
    member_count: int = 0
    total_size: int = 0
    root_seen: bool = False
    executable_seen: bool = False
    identities: set[str] = field(default_factory=set)
    kinds: dict[str, str] = field(default_factory=dict)

    def admit(self, member: tarfile.TarInfo) -> str:
        self.member_count += 1
        if self.member_count > MAX_TREE_ENTRIES:
            raise ValueError("Binaryen archive member count is invalid")
        member_name = (
            member.name[:-1]
            if member.isdir() and member.name.endswith("/")
            else member.name
        )
        relative = portable_relative_path(member_name)
        if not relative.parts or relative.parts[0] != self.expected_root:
            raise ValueError(
                "Binaryen archive member is outside "
                f"{self.expected_root}: {member.name}"
            )
        if len(relative.parts) == 1:
            if self.root_seen or not member.isdir():
                raise ValueError(
                    f"Binaryen archive root is not one exact directory: {member.name}"
                )
            self.root_seen = True
            return "."

        tree_relative_text = relative.relative_to(self.expected_root).as_posix()
        identity = portable_path_identity(tree_relative_text)
        if identity in self.identities:
            raise ValueError(
                f"Binaryen archive has a portable path collision: {member.name}"
            )
        self.identities.add(identity)
        if not member.isfile() and not member.isdir():
            raise ValueError(
                f"Binaryen archive contains an unsupported node: {member.name}"
            )
        parent = portable_relative_path(tree_relative_text).parent
        while parent.parts:
            if self.kinds.get(parent.as_posix()) != "directory":
                raise ValueError(
                    "Binaryen archive omits or misorders an extracted parent "
                    f"directory: {parent.as_posix()}"
                )
            parent = parent.parent
        if member.isdir():
            kind = "directory"
        else:
            if member.size < 0:
                raise ValueError(
                    f"Binaryen archive member has an invalid size: {member.name}"
                )
            self.total_size += member.size
            if self.total_size > MAX_TREE_BYTES:
                raise ValueError("Binaryen archive exceeds its extracted-size policy")
            kind = "file"
            if tree_relative_text == self.executable:
                self.executable_seen = True
        self.kinds[tree_relative_text] = kind
        return tree_relative_text

    def finalize(self) -> None:
        if self.member_count == 0:
            raise ValueError("Binaryen archive member count is invalid")
        if not self.root_seen:
            raise ValueError(
                f"Binaryen archive is missing root directory {self.expected_root}"
            )
        if not self.executable_seen:
            raise ValueError(
                f"Binaryen archive is missing required executable: {self.executable}"
            )


def _download(url: str, output: Path, *, size: int, sha256: str) -> None:
    digest = hashlib.sha256()
    observed_size = 0
    with (
        output.open("xb") as stream,
        urllib.request.urlopen(url, timeout=120) as response,
    ):
        while chunk := response.read(1024 * 1024):
            stream.write(chunk)
            digest.update(chunk)
            observed_size += len(chunk)
            if observed_size > size:
                raise ValueError("Binaryen download exceeds its manifest size")
    if observed_size != size or digest.hexdigest() != sha256:
        raise ValueError("Binaryen download differs from its manifest identity")


def _validated_archive_members(
    archive: tarfile.TarFile,
    *,
    expected_root: str,
    executable: str,
) -> list[tuple[tarfile.TarInfo, str]]:
    validator = _BinaryenArchiveValidator(
        expected_root=expected_root,
        executable=executable,
    )
    validated: list[tuple[tarfile.TarInfo, str]] = []
    for member in archive.getmembers():
        validated.append((member, validator.admit(member)))
    validator.finalize()
    return validated


def _archive_identity(
    archive: tarfile.TarFile,
    *,
    asset_id: str,
    expected_root: str,
    executable: str,
) -> _BinaryenArchiveIdentity:
    """Compute offline manifest identities from one validated upstream archive."""

    entries: list[BinaryenTreeEntry] = []
    executable_sha256: str | None = None
    for member, relative_text in _validated_archive_members(
        archive,
        expected_root=expected_root,
        executable=executable,
    ):
        mode = 0 if asset_id.startswith("windows-") else stat.S_IMODE(member.mode)
        if member.isdir():
            entry = BinaryenTreeEntry(
                path=relative_text,
                kind="directory",
                size=0,
                mode=mode,
                sha256="",
            )
        else:
            extracted = archive.extractfile(member)
            if extracted is None:
                raise ValueError(
                    f"Binaryen archive member is unreadable: {member.name}"
                )
            with extracted:
                content_sha256 = hashlib.file_digest(extracted, "sha256").hexdigest()
            entry = BinaryenTreeEntry(
                path=relative_text,
                kind="file",
                size=member.size,
                mode=mode,
                sha256=content_sha256,
            )
            if relative_text == executable:
                executable_sha256 = content_sha256
        entries.append(entry)
    if executable_sha256 is None:
        raise ValueError(
            f"Binaryen archive is missing required executable: {executable}"
        )
    try:
        tree = binaryen_tree_identity_from_entries(entries)
    except ValueError as exc:
        raise ValueError(f"Binaryen archive tree identity is invalid: {exc}") from exc
    return _BinaryenArchiveIdentity(
        tree=tree,
        executable_sha256=executable_sha256,
    )


def _validate_archive(
    archive: tarfile.TarFile,
    *,
    asset: BinaryenHostAsset,
) -> None:
    _validated_archive_members(
        archive,
        expected_root=asset.archive_root,
        executable=asset.executable,
    )


def _extract_archive_once(
    archive_path: Path,
    output: Path,
    asset: BinaryenHostAsset,
) -> None:
    """Validate and extract one gzip stream without a duplicate decompression pass."""

    validator = _BinaryenArchiveValidator(
        expected_root=asset.archive_root,
        executable=asset.executable,
    )
    directory_modes: list[tuple[Path, int]] = []
    with tarfile.open(archive_path, "r|gz") as archive:
        for member in archive:
            relative = validator.admit(member)
            if asset.tree_includes_modes and member.isdir():
                # The data filter intentionally removes directory modes. Only
                # restore permissions its safety policy would allow, after
                # extraction so read-only parents cannot block their children.
                if member.mode < 0 or member.mode & ~0o755:
                    raise ValueError("Binaryen archive directory has an unsafe mode")
                directory = output / asset.archive_root
                if relative != ".":
                    directory = directory.joinpath(
                        *portable_relative_path(relative).parts
                    )
                directory_modes.append((directory, member.mode))
            archive.extract(member, output, filter="data")
    validator.finalize()
    # Keep each ancestor accessible until all of its children's permissions
    # have been installed, including the separately admitted archive root.
    for directory, mode in sorted(
        directory_modes, key=lambda entry: len(entry[0].parts), reverse=True
    ):
        directory.chmod(mode)


def _installation_identity(
    root: Path,
    asset: BinaryenHostAsset,
) -> BinaryenInstallationIdentity:
    observed = binaryen_installation_identity(
        root,
        include_modes=asset.tree_includes_modes,
        executable=asset.executable,
    )
    if (
        observed.tree.entries != asset.tree_entries
        or observed.tree.total_bytes != asset.tree_total_bytes
        or observed.tree.sha256 != asset.tree_sha256
    ):
        raise ValueError("Binaryen extracted tree differs from its manifest identity")
    if observed.executable_sha256 != asset.executable_sha256:
        raise ValueError("Binaryen executable differs from its manifest identity")
    return observed


def _read_wasm_opt_version(executable: Path, *, expected_sha256: str) -> str:
    version, record = read_binaryen_version(executable, expected_sha256=expected_sha256)
    # Pinned release assets retain their tagged identity even though external
    # source archive builds can legitimately report the numeric version alone.
    if record != f"wasm-opt version {version} (version_{version})":
        raise ValueError(
            f"provisioned wasm-opt reported an invalid version: {record!r}"
        )
    return version


def _wasm_opt_path(root: Path, asset: BinaryenHostAsset) -> Path:
    relative = portable_relative_path(asset.executable)
    return root.joinpath(*relative.parts)


def _verify_installation(root: Path, asset: BinaryenHostAsset) -> Path:
    try:
        receipt = load_binaryen_install_receipt(root)
        observed = _installation_identity(root, asset)
        observed_tree = observed.tree.as_record()
        root = root.resolve(strict=True)
    except (OSError, ValueError) as exc:
        raise ValueError(f"existing Binaryen installation is invalid: {exc}") from exc
    if receipt["asset"] != asdict(asset):
        raise ValueError(
            "existing Binaryen provision receipt differs from the exact host asset"
        )
    if receipt["tree"] != observed_tree:
        raise ValueError(
            "existing Binaryen filesystem tree differs from its provisioned identity"
        )
    wasm_opt = _wasm_opt_path(root, asset)
    if not wasm_opt.is_file():
        raise ValueError(f"existing Binaryen installation lacks {asset.executable}")
    if not asset.id.startswith("windows-") and not os.access(wasm_opt, os.X_OK):
        raise ValueError(
            f"existing Binaryen executable is not executable: {asset.executable}"
        )
    observed_version = _read_wasm_opt_version(
        wasm_opt, expected_sha256=asset.executable_sha256
    )
    if observed_version != asset.version:
        raise ValueError(
            "existing Binaryen executable identity differs from the exact host asset: "
            f"expected {asset.version!r}, found {observed_version!r}"
        )
    return root


def provision_binaryen(output: Path) -> Path:
    lexical_output = output.expanduser().absolute()
    if lexical_output.exists() or lexical_output.is_symlink():
        return _verify_installation(lexical_output, binaryen_host_asset(ROOT))
    lexical_output.parent.mkdir(parents=True, exist_ok=True)
    output = lexical_output.parent.resolve(strict=True) / lexical_output.name
    if output.exists() or output.is_symlink():
        return _verify_installation(output, binaryen_host_asset(ROOT))

    asset = binaryen_host_asset(ROOT)
    with tempfile.TemporaryDirectory(prefix="molt-binaryen-", dir=output.parent) as raw:
        temporary = Path(raw)
        archive_path = temporary / f"{asset.archive_root}.tar.gz"
        _download(asset.url, archive_path, size=asset.size, sha256=asset.sha256)
        extracted = temporary / "extract"
        extracted.mkdir()
        _extract_archive_once(archive_path, extracted, asset)

        installation = extracted / asset.archive_root
        wasm_opt = _wasm_opt_path(installation, asset)
        if not wasm_opt.is_file():
            raise ValueError(
                f"Binaryen archive is missing required executable: {asset.executable}"
            )
        if not asset.id.startswith("windows-") and not os.access(wasm_opt, os.X_OK):
            raise ValueError(
                f"Binaryen archive executable is not executable: {asset.executable}"
            )
        observed = _installation_identity(installation, asset)
        observed_version = _read_wasm_opt_version(
            wasm_opt, expected_sha256=asset.executable_sha256
        )
        if observed_version != asset.version:
            raise ValueError(
                "Binaryen executable identity differs from the exact host asset: "
                f"expected {asset.version!r}, found {observed_version!r}"
            )

        receipt = validate_binaryen_install_receipt(
            {
                "schema": INSTALL_RECEIPT_SCHEMA,
                "asset": asdict(asset),
                "tree": observed.tree.as_record(),
            }
        )
        (installation / INSTALL_RECEIPT_FILENAME).write_text(
            json.dumps(receipt, allow_nan=False, separators=(",", ":"), sort_keys=True)
            + "\n",
            encoding="utf-8",
            newline="",
        )
        durable_publish_directory_exclusive(installation, output)
    return output


def _write_github_outputs(path: Path, outputs: dict[str, Path]) -> None:
    with path.open("a", encoding="utf-8", newline="\n") as handle:
        for name, value in outputs.items():
            rendered = str(value)
            if "\n" in rendered or "\r" in rendered:
                raise ValueError(f"GitHub output {name!r} contains a newline")
            handle.write(f"{name}={rendered}\n")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--github-output", type=Path)
    args = parser.parse_args()
    root = provision_binaryen(args.output)
    asset = binaryen_host_asset(ROOT)
    wasm_opt = _wasm_opt_path(root, asset)
    if args.github_output is not None:
        _write_github_outputs(
            args.github_output,
            {"root": root, "wasm_opt": wasm_opt},
        )
    print(root)


if __name__ == "__main__":
    main()
