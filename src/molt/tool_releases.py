"""Pinned, digest-bound releases of standalone tool binaries.

``config/tool_releases.toml`` is the one authority for tools that ship as
prebuilt release binaries (today: ``wasm-tools``). CI installs the same pin
(gated by tests/test_ci_workflow_topology.py), the proof plan's toolchain
policy cites it as setup evidence, and the proof queue provisions the host
asset into ``<toolchain root>/toolchains/<name>-<version>/bin`` before a lane that
declares the tool runs. A download is accepted only when its byte size and
SHA-256 match the manifest exactly; the installed executable carries an
attestation that discovery re-verifies by content hash on every use.
"""

from __future__ import annotations

import hashlib
import json
import os
import platform
import re
import shutil
import stat
import sys
import tarfile
import tempfile
import tomllib
import urllib.request
import zipfile
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path

from molt.dx import TOOLCHAINS_DIRNAME

TOOL_RELEASES_PATH = "config/tool_releases.toml"
TOOL_RELEASES_SCHEMA_VERSION = 2
TOOL_ATTESTATION_FILENAME = ".molt-tool-release.json"
TOOL_ATTESTATION_SCHEMA = "molt.tool-release.v2"
DOWNLOADS_DIRNAME = "downloads"

# A release's provenance names the authority its asset digests were read from.
# The digest pin is what makes each asset immutable; the URL shapes keep that
# provenance legible: a GitHub release's assets are tag-addressed downloads of
# that release, an official checksum manifest's assets live in the manifest's
# own version directory.
PROVENANCE_GITHUB_RELEASE = "github-release"
PROVENANCE_CHECKSUM_MANIFEST = "checksum-manifest"
_PROVENANCE_KINDS = frozenset({PROVENANCE_GITHUB_RELEASE, PROVENANCE_CHECKSUM_MANIFEST})
_GITHUB_RELEASE_URL_RE = re.compile(
    r"^https://api\.github\.com/repos/[A-Za-z0-9_.\-]+/[A-Za-z0-9_.\-]+/releases/"
    r"tags/[A-Za-z0-9_.\-]+$"
)
_RELEASE_ASSET_URL_RE = re.compile(
    r"^https://github\.com/[A-Za-z0-9_.\-]+/[A-Za-z0-9_.\-]+/releases/download/"
    r"[A-Za-z0-9_.\-]+/[A-Za-z0-9_.\-]+$"
)
_CHECKSUM_MANIFEST_URL_RE = re.compile(
    r"^https://[A-Za-z0-9.\-]+(?:/[A-Za-z0-9_.\-]+)+/SHASUMS256\.txt$"
)
_SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
_VERSION_RE = re.compile(r"^[0-9]+(?:\.[0-9]+)*(?:[-+][A-Za-z0-9.]+)?$")
_ASSET_KEYS = frozenset(
    {
        "x86_64-windows",
        "aarch64-windows",
        "x86_64-linux",
        "aarch64-linux",
        "x86_64-macos",
        "aarch64-macos",
    }
)


class ToolReleaseError(RuntimeError):
    pass


@dataclass(frozen=True)
class ToolAsset:
    key: str
    url: str
    size: int
    sha256: str
    archive_member: str

    @property
    def filename(self) -> str:
        return self.url.rsplit("/", 1)[-1]


@dataclass(frozen=True)
class ToolProvenance:
    kind: str
    url: str
    release_id: int | None = None

    def payload(self) -> dict[str, object]:
        record: dict[str, object] = {"kind": self.kind, "url": self.url}
        if self.release_id is not None:
            record["release_id"] = self.release_id
        return record


@dataclass(frozen=True)
class ToolRelease:
    name: str
    version: str
    executable: str
    provenance: ToolProvenance
    assets: Mapping[str, ToolAsset]

    @property
    def prefix_name(self) -> str:
        return f"{self.name}-{self.version}"

    @property
    def executable_filename(self) -> str:
        return f"{self.executable}.exe" if os.name == "nt" else self.executable


@dataclass(frozen=True)
class ToolDiscovery:
    release: ToolRelease
    prefix: Path
    executable: Path
    executable_sha256: str
    asset: ToolAsset


def tool_releases_path(root: Path | None = None) -> Path:
    base = Path(__file__).resolve().parents[2] if root is None else Path(root)
    return base / TOOL_RELEASES_PATH


def _require_str(table: Mapping[str, object], key: str, *, where: str) -> str:
    value = table.get(key)
    if not isinstance(value, str) or not value:
        raise ToolReleaseError(f"{where}: {key} must be a non-empty string")
    return value


def _load_provenance(raw: Mapping[str, object], *, where: str) -> ToolProvenance:
    table = raw.get("provenance")
    if not isinstance(table, Mapping):
        raise ToolReleaseError(f"{where}: provenance must be a table")
    kind = _require_str(table, "kind", where=f"{where}.provenance")
    if kind not in _PROVENANCE_KINDS:
        raise ToolReleaseError(
            f"{where}.provenance: kind must be one of {sorted(_PROVENANCE_KINDS)}"
        )
    url = _require_str(table, "url", where=f"{where}.provenance")
    release_id = table.get("release_id")
    if kind == PROVENANCE_GITHUB_RELEASE:
        if not _GITHUB_RELEASE_URL_RE.fullmatch(url):
            raise ToolReleaseError(
                f"{where}.provenance: url must be a tag-addressed GitHub release "
                f"record: {url}"
            )
        if not isinstance(release_id, int) or isinstance(release_id, bool):
            raise ToolReleaseError(f"{where}.provenance: release_id must be an integer")
        return ToolProvenance(kind=kind, url=url, release_id=release_id)
    if not _CHECKSUM_MANIFEST_URL_RE.fullmatch(url):
        raise ToolReleaseError(
            f"{where}.provenance: url must be an official SHASUMS256.txt "
            f"checksum manifest: {url}"
        )
    if release_id is not None:
        raise ToolReleaseError(
            f"{where}.provenance: a checksum manifest has no release_id"
        )
    return ToolProvenance(kind=kind, url=url)


def _require_asset_url(url: str, provenance: ToolProvenance, *, where: str) -> None:
    if provenance.kind == PROVENANCE_GITHUB_RELEASE:
        if not _RELEASE_ASSET_URL_RE.fullmatch(url):
            raise ToolReleaseError(
                f"{where}: url must be a tag-addressed GitHub release asset: {url}"
            )
        return
    directory = provenance.url.rsplit("/", 1)[0] + "/"
    name = url[len(directory) :]
    if not url.startswith(directory) or not name or "/" in name:
        raise ToolReleaseError(
            f"{where}: url must be an asset in the checksum manifest's own "
            f"version directory {directory}: {url}"
        )


def load_tool_releases(root: Path | None = None) -> dict[str, ToolRelease]:
    """Load and validate every pinned tool release."""
    path = tool_releases_path(root)
    try:
        payload = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, tomllib.TOMLDecodeError) as exc:
        raise ToolReleaseError(f"cannot read tool releases {path}: {exc}") from exc
    if payload.get("schema_version") != TOOL_RELEASES_SCHEMA_VERSION:
        raise ToolReleaseError(
            f"{path}: schema_version must be {TOOL_RELEASES_SCHEMA_VERSION}"
        )
    tools = payload.get("tools")
    if not isinstance(tools, Mapping) or not tools:
        raise ToolReleaseError(f"{path}: tools must be a non-empty table")
    releases: dict[str, ToolRelease] = {}
    for name, raw in tools.items():
        where = f"{path}: tools.{name}"
        if not isinstance(raw, Mapping):
            raise ToolReleaseError(f"{where} must be a table")
        version = _require_str(raw, "version", where=where)
        if not _VERSION_RE.fullmatch(version):
            raise ToolReleaseError(f"{where}: version {version!r} is not a release")
        executable = _require_str(raw, "executable", where=where)
        if executable != name:
            raise ToolReleaseError(
                f"{where}: executable {executable!r} must equal the tool name"
            )
        provenance = _load_provenance(raw, where=where)
        raw_assets = raw.get("assets")
        if not isinstance(raw_assets, Mapping) or not raw_assets:
            raise ToolReleaseError(f"{where}: assets must be a non-empty table")
        assets: dict[str, ToolAsset] = {}
        for key, raw_asset in raw_assets.items():
            asset_where = f"{where}.assets.{key}"
            if key not in _ASSET_KEYS:
                raise ToolReleaseError(f"{asset_where}: unknown host asset key")
            if not isinstance(raw_asset, Mapping):
                raise ToolReleaseError(f"{asset_where} must be a table")
            url = _require_str(raw_asset, "url", where=asset_where)
            _require_asset_url(url, provenance, where=asset_where)
            size = raw_asset.get("size")
            if not isinstance(size, int) or isinstance(size, bool) or size <= 0:
                raise ToolReleaseError(
                    f"{asset_where}: size must be a positive integer"
                )
            sha256 = _require_str(raw_asset, "sha256", where=asset_where)
            if not _SHA256_RE.fullmatch(sha256):
                raise ToolReleaseError(
                    f"{asset_where}: sha256 must be 64 lowercase hex"
                )
            member = _require_str(raw_asset, "archive_member", where=asset_where)
            if member.startswith(("/", "\\")) or ".." in member.split("/"):
                raise ToolReleaseError(f"{asset_where}: archive_member escapes archive")
            assets[key] = ToolAsset(
                key=key, url=url, size=size, sha256=sha256, archive_member=member
            )
        releases[name] = ToolRelease(
            name=name,
            version=version,
            executable=executable,
            provenance=provenance,
            assets=assets,
        )
    return releases


def tool_release(name: str, root: Path | None = None) -> ToolRelease:
    releases = load_tool_releases(root)
    if name not in releases:
        raise ToolReleaseError(
            f"{name!r} is not a pinned tool release; known: {sorted(releases)}"
        )
    return releases[name]


def host_asset_key() -> str:
    machine = platform.machine().lower()
    if machine in {"amd64", "x86_64", "x64"}:
        arch = "x86_64"
    elif machine in {"arm64", "aarch64"}:
        arch = "aarch64"
    else:
        raise ToolReleaseError(f"no pinned tool assets for host machine {machine!r}")
    if os.name == "nt":
        system = "windows"
    elif sys.platform == "darwin":
        system = "macos"
    elif sys.platform.startswith("linux"):
        system = "linux"
    else:
        raise ToolReleaseError(f"no pinned tool assets for platform {sys.platform!r}")
    return f"{arch}-{system}"


def host_asset(release: ToolRelease) -> ToolAsset:
    key = host_asset_key()
    asset = release.assets.get(key)
    if asset is None:
        raise ToolReleaseError(
            f"{release.name} {release.version} has no asset for host {key}"
        )
    return asset


def tool_prefix(toolchain_root: Path, release: ToolRelease) -> Path:
    return Path(toolchain_root) / TOOLCHAINS_DIRNAME / release.prefix_name


def tool_executable(prefix: Path, release: ToolRelease) -> Path:
    return Path(prefix) / "bin" / release.executable_filename


def _sha256_file(path: Path) -> str:
    with path.open("rb") as handle:
        return hashlib.file_digest(handle, "sha256").hexdigest()


def _attestation_payload(
    release: ToolRelease, asset: ToolAsset, executable_sha256: str
) -> dict[str, object]:
    return {
        "schema": TOOL_ATTESTATION_SCHEMA,
        "tool": release.name,
        "version": release.version,
        "asset": asset.key,
        "asset_url": asset.url,
        "asset_sha256": asset.sha256,
        "provenance": release.provenance.payload(),
        "executable": release.executable_filename,
        "executable_sha256": executable_sha256,
    }


def discover_tool(release: ToolRelease, toolchain_root: Path) -> ToolDiscovery | None:
    """Return the attested installation of ``release`` under the root, if valid.

    Anything short of a byte-exact match between the attestation and the
    installed executable is not a discovery: the caller provisions afresh or
    fails closed, never trusts a partially matching install.
    """
    prefix = tool_prefix(toolchain_root, release)
    executable = tool_executable(prefix, release)
    attestation_path = prefix / TOOL_ATTESTATION_FILENAME
    try:
        attestation = json.loads(attestation_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError):
        return None
    if not isinstance(attestation, Mapping) or not executable.is_file():
        return None
    asset = release.assets.get(str(attestation.get("asset") or ""))
    if asset is None:
        return None
    executable_sha256 = _sha256_file(executable)
    if attestation != _attestation_payload(release, asset, executable_sha256):
        return None
    return ToolDiscovery(
        release=release,
        prefix=prefix,
        executable=executable,
        executable_sha256=executable_sha256,
        asset=asset,
    )


def _download_asset(asset: ToolAsset, downloads: Path) -> Path:
    downloads.mkdir(parents=True, exist_ok=True)
    archive = downloads / asset.filename
    if archive.is_file() and archive.stat().st_size == asset.size:
        if _sha256_file(archive) == asset.sha256:
            return archive
    partial = archive.with_name(archive.name + ".partial")
    with urllib.request.urlopen(asset.url, timeout=180) as response:  # noqa: S310
        with partial.open("wb") as out:
            shutil.copyfileobj(response, out)
    size = partial.stat().st_size
    digest = _sha256_file(partial)
    if size != asset.size or digest != asset.sha256:
        partial.unlink()
        raise ToolReleaseError(
            f"{asset.url} does not match its pinned identity: got size {size} "
            f"sha256 {digest}, expected size {asset.size} sha256 {asset.sha256}"
        )
    os.replace(partial, archive)
    return archive


def _extract_member(archive: Path, member: str, destination: Path) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        dir=destination.parent, prefix=destination.name + ".", delete=False
    ) as staged:
        staged_path = Path(staged.name)
        try:
            if archive.name.endswith(".zip"):
                with zipfile.ZipFile(archive) as bundle:
                    with bundle.open(member) as source:
                        shutil.copyfileobj(source, staged)
            else:
                with tarfile.open(archive) as bundle:
                    entry = bundle.getmember(member)
                    if not entry.isfile():
                        raise ToolReleaseError(
                            f"{archive.name}: {member} is not a regular file"
                        )
                    source = bundle.extractfile(entry)
                    if source is None:
                        raise ToolReleaseError(f"{archive.name}: cannot read {member}")
                    with source:
                        shutil.copyfileobj(source, staged)
        except KeyError as exc:
            staged_path.unlink(missing_ok=True)
            raise ToolReleaseError(f"{archive.name} has no member {member}") from exc
        except BaseException:
            staged_path.unlink(missing_ok=True)
            raise
    if os.name != "nt":
        staged_path.chmod(
            staged_path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH
        )
    os.replace(staged_path, destination)


def provision_tool(
    release: ToolRelease, toolchain_root: Path, *, downloads: Path | None = None
) -> ToolDiscovery:
    """Install ``release`` for this host under the toolchain root, or fail closed.

    Idempotent: a valid attested installation is returned untouched. Otherwise
    the host asset is downloaded (or reused from ``downloads`` when its size and
    digest already match), verified against the manifest, and its executable
    member is installed atomically together with the attestation.
    """
    existing = discover_tool(release, toolchain_root)
    if existing is not None:
        return existing
    asset = host_asset(release)
    archive = _download_asset(
        asset,
        Path(toolchain_root) / TOOLCHAINS_DIRNAME / DOWNLOADS_DIRNAME
        if downloads is None
        else downloads,
    )
    prefix = tool_prefix(toolchain_root, release)
    executable = tool_executable(prefix, release)
    _extract_member(archive, asset.archive_member, executable)
    payload = _attestation_payload(release, asset, _sha256_file(executable))
    attestation_path = prefix / TOOL_ATTESTATION_FILENAME
    staged = attestation_path.with_name(attestation_path.name + ".partial")
    staged.write_text(json.dumps(payload, sort_keys=True, indent=2) + "\n", "utf-8")
    os.replace(staged, attestation_path)
    discovered = discover_tool(release, toolchain_root)
    if discovered is None:
        raise ToolReleaseError(
            f"{release.name} {release.version} did not attest after provisioning"
        )
    return discovered


def pinned_executable(name: str, repo_root: Path) -> Path | None:
    """The provisioned pinned release of ``name`` under this checkout's custody.

    Tools that run WASM artifacts (Node) or inspect them prefer the release the
    manifest pins over whatever the host PATH carries, so a direct run and a
    proof-queue lane execute the same binary. ``None`` when the manifest pins
    no such tool or the release is not provisioned (and attested) under the
    checkout custody toolchain root; callers then fall back to their host
    discovery, never to a partially matching install.
    """
    from molt.dx import checkout_custody

    release = load_tool_releases(repo_root).get(name)
    if release is None:
        return None
    toolchain_root = checkout_custody(repo_root).toolchain_root
    discovery = discover_tool(release, toolchain_root)
    return None if discovery is None else discovery.executable


def main(argv: list[str] | None = None) -> int:
    import argparse

    from molt.dx import checkout_custody

    parser = argparse.ArgumentParser(
        description="Provision or inspect pinned tool releases under custody."
    )
    parser.add_argument("action", choices=("list", "discover", "provision"))
    parser.add_argument("tool", nargs="?")
    parser.add_argument("--repo-root", type=Path, default=Path.cwd())
    args = parser.parse_args(argv)
    releases = load_tool_releases(args.repo_root)
    if args.action == "list":
        for release in releases.values():
            print(f"{release.name} {release.version} assets={sorted(release.assets)}")
        return 0
    if not args.tool:
        parser.error("tool is required")
    release = tool_release(args.tool, args.repo_root)
    toolchain_root = checkout_custody(args.repo_root).toolchain_root
    discovery = (
        provision_tool(release, toolchain_root)
        if args.action == "provision"
        else discover_tool(release, toolchain_root)
    )
    if discovery is None:
        print(
            f"{release.name} {release.version}: not provisioned under {toolchain_root}"
        )
        return 1
    print(
        f"{release.name} {release.version}: {discovery.executable} "
        f"sha256={discovery.executable_sha256}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
