"""Pinned tool releases: one manifest, digest-verified provisioning, attested discovery."""

from __future__ import annotations

import hashlib
import json
import os
import zipfile
from pathlib import Path

import pytest

from molt import tool_releases
from molt.dx import TOOLCHAINS_DIRNAME

ROOT = Path(__file__).resolve().parents[1]


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def test_repository_manifest_pins_every_host_asset_of_wasm_tools() -> None:
    releases = tool_releases.load_tool_releases(ROOT)
    release = releases["wasm-tools"]
    assert release.executable == "wasm-tools"
    assert set(release.assets) >= {
        "x86_64-windows",
        "x86_64-linux",
        "aarch64-linux",
        "x86_64-macos",
        "aarch64-macos",
    }
    for asset in release.assets.values():
        assert asset.url.startswith(
            "https://github.com/bytecodealliance/wasm-tools/releases/download/"
            f"v{release.version}/"
        )
        assert asset.archive_member.endswith(("wasm-tools", "wasm-tools.exe"))
    assert release.prefix_name == f"wasm-tools-{release.version}"
    assert release.provenance.kind == tool_releases.PROVENANCE_GITHUB_RELEASE


def test_repository_manifest_pins_node_from_the_official_distribution() -> None:
    release = tool_releases.load_tool_releases(ROOT)["node"]
    assert release.provenance.kind == tool_releases.PROVENANCE_CHECKSUM_MANIFEST
    assert release.provenance.url == (
        f"https://nodejs.org/dist/v{release.version}/SHASUMS256.txt"
    )
    assert release.provenance.release_id is None
    assert set(release.assets) >= {
        "x86_64-windows",
        "x86_64-linux",
        "aarch64-linux",
        "x86_64-macos",
        "aarch64-macos",
    }
    for asset in release.assets.values():
        assert asset.url.startswith(f"https://nodejs.org/dist/v{release.version}/")
        assert asset.archive_member.endswith(("/bin/node", "/node.exe"))


def test_checksum_manifest_assets_must_live_in_the_manifest_directory(
    tmp_path: Path,
) -> None:
    (tmp_path / "config").mkdir(parents=True)
    manifest = tmp_path / "config" / "tool_releases.toml"

    def write(asset_url: str, provenance_url: str) -> None:
        manifest.write_text(
            "\n".join(
                [
                    "schema_version = 2",
                    "[tools.demo]",
                    'version = "1.2.3"',
                    'executable = "demo"',
                    "[tools.demo.provenance]",
                    'kind = "checksum-manifest"',
                    f'url = "{provenance_url}"',
                    f"[tools.demo.assets.{tool_releases.host_asset_key()}]",
                    f'url = "{asset_url}"',
                    "size = 1",
                    f'sha256 = "{"a" * 64}"',
                    'archive_member = "demo-1.2.3/demo"',
                ]
            )
            + "\n",
            encoding="utf-8",
        )

    write(
        "https://dist.example/v1.2.3/demo-1.2.3.tar.gz",
        "https://dist.example/v1.2.3/SHASUMS256.txt",
    )
    assert tool_releases.load_tool_releases(tmp_path)["demo"].provenance.kind == (
        tool_releases.PROVENANCE_CHECKSUM_MANIFEST
    )
    write(
        "https://dist.example/v1.2.4/demo-1.2.3.tar.gz",
        "https://dist.example/v1.2.3/SHASUMS256.txt",
    )
    with pytest.raises(tool_releases.ToolReleaseError, match="own\s+version directory"):
        tool_releases.load_tool_releases(tmp_path)
    write(
        "https://dist.example/v1.2.3/demo-1.2.3.tar.gz",
        "https://dist.example/v1.2.3/checksums.txt",
    )
    with pytest.raises(tool_releases.ToolReleaseError, match="SHASUMS256"):
        tool_releases.load_tool_releases(tmp_path)


def _write_manifest(
    root: Path, asset: dict[str, object], *, tool: str = "demo"
) -> None:
    (root / "config").mkdir(parents=True, exist_ok=True)
    lines = [
        "schema_version = 2",
        f"[tools.{tool}]",
        'version = "1.2.3"',
        f'executable = "{tool}"',
        f"[tools.{tool}.provenance]",
        'kind = "github-release"',
        'url = "https://api.github.com/repos/o/r/releases/tags/v1.2.3"',
        "release_id = 7",
        f"[tools.{tool}.assets.{tool_releases.host_asset_key()}]",
        *(f"{key} = {json.dumps(value)}" for key, value in asset.items()),
    ]
    (root / "config" / "tool_releases.toml").write_text(
        "\n".join(lines) + "\n", encoding="utf-8"
    )


def _zip_archive(path: Path, member: str, payload: bytes) -> None:
    with zipfile.ZipFile(path, "w") as bundle:
        bundle.writestr(member, payload)


def test_manifest_refuses_unpinned_or_escaping_assets(tmp_path: Path) -> None:
    good = {
        "url": "https://github.com/o/r/releases/download/v1.2.3/demo-1.2.3.zip",
        "size": 10,
        "sha256": "a" * 64,
        "archive_member": "demo-1.2.3/demo",
    }
    _write_manifest(tmp_path, good)
    assert "demo" in tool_releases.load_tool_releases(tmp_path)
    for defect, message in (
        ({"url": "https://example.com/demo.zip"}, "tag-addressed GitHub release"),
        ({"sha256": "A" * 64}, "64 lowercase hex"),
        ({"size": 0}, "positive integer"),
        ({"archive_member": "../demo"}, "escapes archive"),
    ):
        _write_manifest(tmp_path, {**good, **defect})
        with pytest.raises(tool_releases.ToolReleaseError, match=message):
            tool_releases.load_tool_releases(tmp_path)


def _pinned_release(
    tmp_path: Path, payload: bytes
) -> tuple[tool_releases.ToolRelease, Path]:
    exe = "demo.exe" if os.name == "nt" else "demo"
    archive = tmp_path / "demo-1.2.3.zip"
    _zip_archive(archive, f"demo-1.2.3/{exe}", payload)
    data = archive.read_bytes()
    _write_manifest(
        tmp_path,
        {
            "url": "https://github.com/o/r/releases/download/v1.2.3/demo-1.2.3.zip",
            "size": len(data),
            "sha256": _sha256(data),
            "archive_member": f"demo-1.2.3/{exe}",
        },
    )
    return tool_releases.tool_release("demo", tmp_path), archive


def test_provisioning_installs_and_attests_only_a_byte_exact_download(
    tmp_path: Path,
) -> None:
    release, archive = _pinned_release(tmp_path, b"demo binary")
    downloads = tmp_path / "downloads"
    downloads.mkdir()
    (downloads / archive.name).write_bytes(archive.read_bytes())
    toolchain_root = tmp_path / "target-root"

    discovery = tool_releases.provision_tool(
        release, toolchain_root, downloads=downloads
    )
    assert discovery.prefix == toolchain_root / TOOLCHAINS_DIRNAME / "demo-1.2.3"
    assert discovery.executable.read_bytes() == b"demo binary"
    assert discovery.executable_sha256 == _sha256(b"demo binary")
    attestation = json.loads(
        (discovery.prefix / tool_releases.TOOL_ATTESTATION_FILENAME).read_text(
            encoding="utf-8"
        )
    )
    assert attestation["schema"] == tool_releases.TOOL_ATTESTATION_SCHEMA
    assert attestation["executable_sha256"] == discovery.executable_sha256
    assert tool_releases.discover_tool(release, toolchain_root) == discovery
    assert (
        tool_releases.provision_tool(release, toolchain_root, downloads=downloads)
        == discovery
    )


def test_tampered_or_stale_installs_are_not_discoveries(tmp_path: Path) -> None:
    release, archive = _pinned_release(tmp_path, b"demo binary")
    downloads = tmp_path / "downloads"
    downloads.mkdir()
    (downloads / archive.name).write_bytes(archive.read_bytes())
    toolchain_root = tmp_path / "target-root"
    discovery = tool_releases.provision_tool(
        release, toolchain_root, downloads=downloads
    )

    discovery.executable.write_bytes(b"replaced")
    assert tool_releases.discover_tool(release, toolchain_root) is None
    reprovisioned = tool_releases.provision_tool(
        release, toolchain_root, downloads=downloads
    )
    assert reprovisioned.executable.read_bytes() == b"demo binary"

    attestation = discovery.prefix / tool_releases.TOOL_ATTESTATION_FILENAME
    attestation.write_text("{}", encoding="utf-8")
    assert tool_releases.discover_tool(release, toolchain_root) is None
    attestation.unlink()
    assert tool_releases.discover_tool(release, toolchain_root) is None


def test_download_that_disagrees_with_the_pin_is_refused(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    release, archive = _pinned_release(tmp_path, b"demo binary")
    forged = tmp_path / "forged.zip"
    _zip_archive(forged, "demo-1.2.3/demo", b"something else")

    class Response:
        def __init__(self) -> None:
            self._data = forged.read_bytes()

        def __enter__(self) -> Response:
            return self

        def __exit__(self, *_exc: object) -> None:
            return None

        def read(self, size: int = -1) -> bytes:
            data, self._data = (
                self._data[:size] if size > 0 else self._data,
                (self._data[size:] if size > 0 else b""),
            )
            return data

    monkeypatch.setattr(
        tool_releases.urllib.request, "urlopen", lambda url, timeout: Response()
    )
    downloads = tmp_path / "downloads"
    with pytest.raises(tool_releases.ToolReleaseError, match="pinned identity"):
        tool_releases.provision_tool(
            release, tmp_path / "target-root", downloads=downloads
        )
    assert not (downloads / archive.name).exists()
    assert tool_releases.discover_tool(release, tmp_path / "target-root") is None


def test_pinned_executable_prefers_a_provisioned_release(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # No release pinned for the name, or pinned but not provisioned: None, so
    # callers fall back to host discovery rather than a partial install.
    assert tool_releases.pinned_executable("no-such-tool", ROOT) is None
    release = tool_releases.tool_release("node", ROOT)
    toolchain_root = tmp_path / "toolchains"

    class Custody:
        pass

    Custody.toolchain_root = toolchain_root
    monkeypatch.setattr("molt.dx.checkout_custody", lambda root, *a, **k: Custody)
    assert tool_releases.pinned_executable("node", ROOT) is None

    prefix = tool_releases.tool_prefix(toolchain_root, release)
    executable = tool_releases.tool_executable(prefix, release)
    executable.parent.mkdir(parents=True)
    executable.write_bytes(b"pinned")
    asset = tool_releases.host_asset(release)
    (prefix / tool_releases.TOOL_ATTESTATION_FILENAME).write_text(
        json.dumps(
            tool_releases._attestation_payload(
                release, asset, hashlib.sha256(b"pinned").hexdigest()
            )
        ),
        encoding="utf-8",
    )
    assert tool_releases.pinned_executable("node", ROOT) == executable
