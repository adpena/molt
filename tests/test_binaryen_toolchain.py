from __future__ import annotations

from dataclasses import asdict
import hashlib
import json
from pathlib import Path
import re

import pytest

from molt import binaryen_toolchain
from molt.binaryen_toolchain import BinaryenConfigError


ROOT = Path(__file__).resolve().parents[1]

_EXPECTED_ASSETS = {
    "linux-aarch64": (
        "binaryen-version_130-aarch64-linux.tar.gz",
        101874616,
        "e6ae6e09ac40f4e14bc5be6f687c58e2995c84170013975fa641809dd3b480a0",
        "bin/wasm-opt",
        21,
        292820873,
        "b3ec95ca94f208a7bfd6a80a5bc0f88885e55b2b658b81d196e71f313f2c8693",
        "99a4351f464f16f825afed154f7300896533ebce58b43b20db5542b602fd100e",
    ),
    "linux-x86_64": (
        "binaryen-version_130-x86_64-linux.tar.gz",
        107371282,
        "0a18362361ad05465118cd8eeb72edaeec89de6894bc283576ef4e07aa3babcc",
        "bin/wasm-opt",
        21,
        307435649,
        "41afccdd3788863d452e7aca0e335c07280db3a1dad52dd1abd3c6f0660a8900",
        "052fe3ae03ba9c566f9d0273cc94d943258bc20a92a0d25f148b55c1caf43558",
    ),
    "macos-aarch64": (
        "binaryen-version_130-arm64-macos.tar.gz",
        7362973,
        "79d3ab9f417d9e215f15f598f523d001a7d9ac1e59367e5c869fbdabd1cba72e",
        "bin/wasm-opt",
        21,
        25162001,
        "71825330fa7d956a4f162aaad503fc382150550dcf905dc8aacc612b023260e4",
        "fa49ec32a92b72b368b66f8f19d84e9adffeef002449e6a07cd95a3ed2612b41",
    ),
    "macos-x86_64": (
        "binaryen-version_130-x86_64-macos.tar.gz",
        8705522,
        "d3e2d1235b70c93c54b52eabc1625ea960965152218754f1f4eeb0f873c48e03",
        "bin/wasm-opt",
        21,
        27533785,
        "36bb42e064e15edd2ecc78e835c8f61e45b2523b1375e9f19cd44e9819087fed",
        "bbfb5f25fe43e95a86e45410047c816b036be6dcf363f05b7d768b552fd00606",
    ),
    "windows-aarch64": (
        "binaryen-version_130-arm64-windows.tar.gz",
        105980303,
        "b18c9cbe000562b1ee5d9cb60146616a949aca504903ad63f27fd9fd679898a7",
        "bin/wasm-opt.exe",
        21,
        641724419,
        "8eafa801069d0462efbe34ea7eb49061858a741e4dc0f4036850c32381e04034",
        "37ddf73166f4daffc1344ca60170cd52b7aac1f1df1f2250e300404d13ade219",
    ),
    "windows-x86_64": (
        "binaryen-version_130-x86_64-windows.tar.gz",
        80320428,
        "cc09c874f4332d00aa32ab72745a9b98c9a172f795762f21d03e70638a3f7f4c",
        "bin/wasm-opt.exe",
        21,
        330414607,
        "0ec2b40af8efabe69240bae63600ce73c3f108a51a150ba08a571846ba8f278c",
        "7d1a954d0a1c11dc50ba3e41ec483a27b40e769ae4d577a2de7ed41c6463bbe9",
    ),
}


def test_manifest_pins_the_complete_exact_upstream_v130_matrix() -> None:
    manifest = binaryen_toolchain.load_binaryen_manifest(ROOT)

    assert manifest.schema_version == 1
    assert manifest.release.version == "130"
    assert manifest.release.provenance_url == (
        "https://api.github.com/repos/WebAssembly/binaryen/releases/tags/version_130"
    )
    assert {asset.id for asset in manifest.release.targets} == set(_EXPECTED_ASSETS)
    for asset in manifest.release.targets:
        (
            filename,
            size,
            sha256,
            executable,
            tree_entries,
            tree_total_bytes,
            tree_sha256,
            executable_sha256,
        ) = _EXPECTED_ASSETS[asset.id]
        assert asset.archive_root == "binaryen-version_130"
        assert asset.url == (
            "https://github.com/WebAssembly/binaryen/releases/download/version_130/"
            f"{filename}"
        )
        assert (
            asset.size,
            asset.sha256,
            asset.executable,
            asset.tree_entries,
            asset.tree_total_bytes,
            asset.tree_sha256,
            asset.executable_sha256,
        ) == (
            size,
            sha256,
            executable,
            tree_entries,
            tree_total_bytes,
            tree_sha256,
            executable_sha256,
        )
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
        binaryen_toolchain._load_binaryen_manifest_cached(str(manifest))


@pytest.mark.parametrize(
    "substitution",
    (
        ('version = "130"', 'version = "0130"'),
        ("binaryen-version_130-x86_64-linux", "binaryen-version_130-amd64-linux"),
        ('executable = "bin/wasm-opt"', 'executable = "bin/../wasm-opt"'),
        ("size = 107371282", 'size = "107371282"'),
        ("tree_entries = 21", 'tree_entries = "21"'),
        ("tree_total_bytes = 307435649", "tree_total_bytes = 0"),
        (
            'tree_sha256 = "41afccdd3788863d452e7aca0e335c07280db3a1dad52dd1abd3c6f0660a8900"',
            'tree_sha256 = "not-a-sha256"',
        ),
        (
            'executable_sha256 = "052fe3ae03ba9c566f9d0273cc94d943258bc20a92a0d25f148b55c1caf43558"',
            'executable_sha256 = "not-a-sha256"',
        ),
    ),
)
def test_manifest_rejects_noncanonical_release_and_asset_identities(
    tmp_path: Path,
    substitution: tuple[str, str],
) -> None:
    source = (ROOT / "config/binaryen_releases.toml").read_text(encoding="utf-8")
    old, new = substitution
    invalid = source.replace(old, new, 1)
    assert invalid != source
    manifest = tmp_path / "binaryen_releases.toml"
    manifest.write_text(invalid, encoding="utf-8")

    with pytest.raises(BinaryenConfigError, match="identity|portable"):
        binaryen_toolchain._load_binaryen_manifest_cached(str(manifest))
