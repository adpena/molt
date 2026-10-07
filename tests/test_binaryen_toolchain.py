from __future__ import annotations

from dataclasses import asdict
import hashlib
import json
from pathlib import Path
import re

import pytest

from molt import binaryen_toolchain
from molt.binaryen_toolchain import BinaryenConfigError
from molt.source_root import source_file_revision


ROOT = Path(__file__).resolve().parents[1]

# Upstream's per-host archive naming, independent of any release version.
_UPSTREAM_COORDINATES = {
    "linux-aarch64": "aarch64-linux",
    "linux-x86_64": "x86_64-linux",
    "macos-aarch64": "arm64-macos",
    "macos-x86_64": "x86_64-macos",
    "windows-aarch64": "arm64-windows",
    "windows-x86_64": "x86_64-windows",
}
_SHA256 = re.compile(r"[0-9a-f]{64}")


def test_manifest_pins_the_complete_exact_upstream_matrix() -> None:
    manifest = binaryen_toolchain.load_binaryen_manifest(ROOT)
    version = manifest.release.version

    assert manifest.schema_version == 1
    assert re.fullmatch(r"[1-9][0-9]*", version)
    assert manifest.release.provenance_url == (
        "https://api.github.com/repos/WebAssembly/binaryen/releases/tags/"
        f"version_{version}"
    )
    assert {asset.id for asset in manifest.release.targets} == set(
        _UPSTREAM_COORDINATES
    )
    for asset in manifest.release.targets:
        assert asset.archive_root == f"binaryen-version_{version}"
        assert asset.url == (
            "https://github.com/WebAssembly/binaryen/releases/download/"
            f"version_{version}/binaryen-version_{version}-"
            f"{_UPSTREAM_COORDINATES[asset.id]}.tar.gz"
        )
        assert asset.executable == (
            "bin/wasm-opt.exe" if asset.id.startswith("windows-") else "bin/wasm-opt"
        )
        assert asset.size > 0 and asset.tree_entries > 0
        assert asset.tree_total_bytes > 0
        for digest in (asset.sha256, asset.tree_sha256, asset.executable_sha256):
            assert _SHA256.fullmatch(digest), asset.id
        record = asdict(asset)
        record_sha256 = record.pop("record_sha256")
        assert (
            record_sha256
            == hashlib.sha256(
                json.dumps(record, sort_keys=True, separators=(",", ":")).encode(
                    "utf-8"
                )
            ).hexdigest()
        )


@pytest.mark.parametrize(
    ("system", "machine", "asset_id"),
    (
        ("Linux", "x86_64", "linux-x86_64"),
        ("Linux", "aarch64", "linux-aarch64"),
        ("Darwin", "x86_64", "macos-x86_64"),
        ("Darwin", "arm64", "macos-aarch64"),
        ("Windows", "AMD64", "windows-x86_64"),
        ("Windows", "ARM64", "windows-aarch64"),
    ),
)
def test_host_selection_covers_every_upstream_release_coordinate(
    system: str,
    machine: str,
    asset_id: str,
) -> None:
    assert (
        binaryen_toolchain.binaryen_host_asset(
            ROOT,
            system=system,
            machine=machine,
        ).id
        == asset_id
    )


@pytest.mark.parametrize(
    ("system", "machine"),
    (("FreeBSD", "x86_64"), ("Linux", "i686"), ("Linux", "riscv64")),
)
def test_host_selection_fails_closed_outside_the_upstream_matrix(
    system: str,
    machine: str,
) -> None:
    with pytest.raises(BinaryenConfigError, match="unsupported|no host asset"):
        binaryen_toolchain.binaryen_host_asset(
            ROOT,
            system=system,
            machine=machine,
        )


def test_manifest_requires_the_complete_release_matrix(tmp_path: Path) -> None:
    source = (ROOT / "config/binaryen_releases.toml").read_text(encoding="utf-8")
    incomplete, replacements = re.subn(
        r"\n\[targets\.windows-aarch64\]\n.*\Z",
        "\n",
        source,
        count=1,
        flags=re.DOTALL,
    )
    assert replacements == 1
    manifest = tmp_path / "binaryen_releases.toml"
    manifest.write_text(incomplete, encoding="utf-8")

    with pytest.raises(BinaryenConfigError, match="complete shipped release matrix"):
        binaryen_toolchain._load_binaryen_manifest_cached(
            str(manifest), source_file_revision(manifest)
        )


def _linux_x86_64() -> binaryen_toolchain.BinaryenHostAsset:
    release = binaryen_toolchain.load_binaryen_manifest(ROOT).release
    return next(asset for asset in release.targets if asset.id == "linux-x86_64")


# Each substitution corrupts one field of the shipped manifest.
_SUBSTITUTIONS = {
    "zero-padded version": lambda a: (
        f'version = "{a.version}"',
        f'version = "0{a.version}"',
    ),
    "renamed coordinate": lambda a: (
        f"{a.archive_root}-x86_64-linux",
        f"{a.archive_root}-amd64-linux",
    ),
    "escaping executable": lambda a: (
        'executable = "bin/wasm-opt"',
        'executable = "bin/../wasm-opt"',
    ),
    "string size": lambda a: (f"size = {a.size}", f'size = "{a.size}"'),
    "string entries": lambda a: (
        f"tree_entries = {a.tree_entries}",
        f'tree_entries = "{a.tree_entries}"',
    ),
    "empty tree": lambda a: (
        f"tree_total_bytes = {a.tree_total_bytes}",
        "tree_total_bytes = 0",
    ),
    "tree digest": lambda a: (
        f'tree_sha256 = "{a.tree_sha256}"',
        'tree_sha256 = "not-a-sha256"',
    ),
    "executable digest": lambda a: (
        f'executable_sha256 = "{a.executable_sha256}"',
        'executable_sha256 = "not-a-sha256"',
    ),
}


@pytest.mark.parametrize("substitution", sorted(_SUBSTITUTIONS))
def test_manifest_rejects_noncanonical_release_and_asset_identities(
    tmp_path: Path,
    substitution: str,
) -> None:
    source = (ROOT / "config/binaryen_releases.toml").read_text(encoding="utf-8")
    old, new = _SUBSTITUTIONS[substitution](_linux_x86_64())
    invalid = source.replace(old, new, 1)
    assert invalid != source
    manifest = tmp_path / "binaryen_releases.toml"
    manifest.write_text(invalid, encoding="utf-8")

    with pytest.raises(BinaryenConfigError, match="identity|portable"):
        binaryen_toolchain._load_binaryen_manifest_cached(
            str(manifest), source_file_revision(manifest)
        )
