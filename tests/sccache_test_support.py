"""Hermetic selectors and receipt-backed tool inputs for sccache policy tests."""

from pathlib import Path
import os
import sys

from molt.cargo_execution_policy import CARGO_WRAPPER_ENV_NAMES


def isolate_sccache_environment(monkeypatch, tmp_path: Path) -> None:
    """Select no ambient cache, wrapper, epoch or installed tool generation."""
    for name in tuple(os.environ):
        if name.upper().startswith("SCCACHE_") or name.upper() in {
            *CARGO_WRAPPER_ENV_NAMES,
            "MOLT_USE_SCCACHE",
            "SOURCE_DATE_EPOCH",
        }:
            monkeypatch.delenv(name, raising=False)
    monkeypatch.setenv("MOLT_TARGET_ROOT", str(tmp_path / "unprovisioned-tools"))


def provision_fixture_sccache(monkeypatch, tmp_path: Path) -> tuple[Path, Path]:
    """Provision and discover an attested native image; never execute a server.

    The selection contract needs a provisioned tool, not sccache's compiler or
    server behavior. The fixture pins the running native Python image through
    the real archive, manifest and receipt boundaries.
    """
    from molt import tool_releases
    from tests.test_tool_releases import _sha256, _write_manifest, _zip_archive

    source = tmp_path / "fixture-source"
    source.mkdir()
    downloads = tmp_path / "downloads"
    downloads.mkdir()
    archive = downloads / "sccache-1.2.3.zip"
    executable = "sccache.exe" if os.name == "nt" else "sccache"
    member = f"sccache-1.2.3/{executable}"
    _zip_archive(archive, member, Path(sys.executable).read_bytes())
    data = archive.read_bytes()
    _write_manifest(
        source,
        {
            "url": f"https://github.com/o/r/releases/download/v1.2.3/{archive.name}",
            "size": len(data),
            "sha256": _sha256(data),
            "archive_member": member,
        },
        tool="sccache",
    )
    releases = tool_releases.load_tool_releases(source)
    monkeypatch.setattr(tool_releases, "load_tool_releases", lambda _root: releases)
    root = tmp_path / "provisioned-tools"
    discovery = tool_releases.provision_tool(
        releases["sccache"], root, downloads=downloads
    )
    return root, discovery.executable
