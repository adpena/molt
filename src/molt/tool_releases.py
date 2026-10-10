"""Pinned, digest-bound releases of standalone tool binaries.

``config/tool_releases.toml`` is the one authority for tools that ship as
prebuilt release binaries, including ``wasm-tools`` and ``lune``. CI installs the same pin
(gated by tests/test_ci_workflow_topology.py), the proof plan's toolchain
policy cites it as setup evidence, and the proof queue provisions the host
asset into ``<toolchain root>/toolchains/<name>-<version>/bin`` before a lane that
declares the tool runs. A download is accepted only when its byte size and
SHA-256 match the manifest exactly; the installed executable carries an
attestation that discovery re-verifies by content hash on every use.
"""

from __future__ import annotations

from contextlib import contextmanager
import hashlib
import json
import os
import platform
import re
import shutil
import stat
import sys
import tarfile
import tomllib
import urllib.parse
import urllib.request
import zipfile
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any, TypeVar

from molt.dx import TOOLCHAINS_DIRNAME
from molt.file_locks import (
    _acquire_file_lock,
    _file_lock_owned_operation,
    _release_file_lock,
)
from molt.file_publication import (
    canonical_file_leaf,
    durable_publish_exclusive,
    durable_replace,
)
from molt.portable_paths import portable_relative_path
from molt.temporary_artifacts import OwnedTemporaryDirectory
from molt.source_root import compiler_source_root
from molt.toolchain_identity import (
    StableRegularFileError,
    open_stable_regular_file,
    stable_executable_probe,
    stable_regular_file_handle_identity,
    stable_regular_file_identity,
    stable_regular_file_version,
)

_ToolResult = TypeVar("_ToolResult")

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
    base = compiler_source_root() if root is None else Path(root)
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


def _archive_https_host(url: str) -> str:
    """Checked-in archive URLs and each redirect must remain credential-free HTTPS."""
    if not isinstance(url, str) or any(ord(c) <= 32 or ord(c) == 127 for c in url):
        raise ToolReleaseError(
            "pinned archive URL contains whitespace/control characters"
        )
    try:
        parsed = urllib.parse.urlsplit(url)
        host, port = parsed.hostname, parsed.port
    except ValueError as exc:
        raise ToolReleaseError("pinned archive URL is malformed") from exc
    if (
        parsed.scheme != "https"
        or not host
        or not re.fullmatch(r"[A-Za-z0-9]+(?:[.-][A-Za-z0-9]+)*", host)
        or port not in (None, 443)
        or parsed.username is not None
        or parsed.password is not None
        or parsed.fragment
        or "\\" in url
    ):
        raise ToolReleaseError(
            "pinned archive URL must use credential-free HTTPS on port 443"
        )
    return host.lower()


class _ArchiveRedirects(urllib.request.HTTPRedirectHandler):
    def __init__(self, url: str) -> None:
        self.hosts = {_archive_https_host(url)}
        # GitHub documents this exact host for release-asset downloads. Other
        # source authorities keep same-origin redirects only; no wildcard CDN.
        if _RELEASE_ASSET_URL_RE.fullmatch(url):
            self.hosts.add("release-assets.githubusercontent.com")

    def admit(self, url: str) -> None:
        if _archive_https_host(url) not in self.hosts:
            raise ToolReleaseError(
                "pinned archive redirect leaves its admitted HTTPS origin"
            )

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        self.admit(newurl)
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def provision_archive(*, url: str, size: int, sha256: str, downloads: Path) -> Path:
    """Cache one exact archive through the shared pinned-tool transfer owner.

    This is explicit development provisioning. It never extracts or executes
    payloads. Cache readers still retain their own stable descriptor while
    consuming bytes. Invalid old entries survive any pre-publication failure.
    """
    try:
        redirects = _ArchiveRedirects(url)
        parsed = urllib.parse.urlsplit(url)
        filename = portable_relative_path(parsed.path.rsplit("/", 1)[-1])
        if len(filename.parts) != 1 or parsed.query or "%" in filename.name:
            raise ToolReleaseError("pinned archive URL must name one literal filename")
        if (
            type(size) is not int
            or size <= 0
            or not isinstance(sha256, str)
            or not _SHA256_RE.fullmatch(sha256)
        ):
            raise ToolReleaseError(
                "pinned archive needs a positive size and lowercase SHA-256"
            )
        archive = canonical_file_leaf(
            Path(downloads) / filename.name, create_parent=True
        )
        lock_path = archive.parent / (
            ".molt-archive-"
            + hashlib.sha256(filename.name.encode()).hexdigest()[:16]
            + ".lock"
        )
        busy_message = f"pinned archive cache is busy: {archive}"
        try:
            lock = _acquire_file_lock(
                lock_path, timeout_s=180, timeout_message=busy_message
            )
        except RuntimeError as exc:
            if str(exc) != busy_message:
                raise
            raise ToolReleaseError(busy_message) from exc
        try:
            with _file_lock_owned_operation(lock, expected_lock_path=lock_path):
                previous = (
                    stable_regular_file_version(archive, label="pinned archive cache")
                    if archive.exists()
                    else None
                )
                if previous is not None and previous.size == size:
                    with open_stable_regular_file(
                        archive, label="pinned archive cache", observed=previous
                    ) as opened:
                        identity = stable_regular_file_handle_identity(
                            opened, label="pinned archive cache", max_bytes=size
                        )
                    if identity.sha256 == sha256:
                        return archive
                with OwnedTemporaryDirectory(
                    prefix="molt-archive-", dir=archive.parent
                ) as raw:
                    staged = Path(raw) / filename.name
                    digest = hashlib.sha256()
                    received = 0
                    request = urllib.request.Request(
                        url,
                        headers={
                            "User-Agent": "molt-pinned-archive/1",
                            "Accept-Encoding": "identity",
                        },
                    )
                    opener = urllib.request.build_opener(redirects)
                    with (
                        opener.open(request, timeout=180) as response,
                        staged.open("xb") as stream,
                    ):
                        redirects.admit(response.geturl())
                        if (
                            response.status != 200
                            or response.headers.get("Content-Encoding", "identity")
                            != "identity"
                        ):
                            raise ToolReleaseError(
                                "pinned archive response must be an unencoded HTTP 200 body"
                            )
                        declared = response.headers.get("Content-Length")
                        if declared is not None and (
                            not re.fullmatch(r"[0-9]+", declared)
                            or int(declared) != size
                        ):
                            raise ToolReleaseError(
                                "pinned archive response length disagrees with its pinned identity"
                            )
                        while chunk := response.read(
                            min(1024 * 1024, size - received + 1)
                        ):
                            received += len(chunk)
                            if received > size:
                                raise ToolReleaseError(
                                    "pinned archive download exceeds its pinned identity size"
                                )
                            stream.write(chunk)
                            digest.update(chunk)
                        stream.flush()
                        os.fsync(stream.fileno())
                    if received != size or digest.hexdigest() != sha256:
                        raise ToolReleaseError(
                            f"{url} does not match its pinned identity: got size {received} sha256 {digest.hexdigest()}, expected size {size} sha256 {sha256}"
                        )
                    captured = stable_regular_file_identity(
                        staged, label="staged pinned archive"
                    )
                    if (captured.size, captured.sha256) != (size, sha256):
                        raise ToolReleaseError(
                            "staged archive changed after its pinned transfer"
                        )
                    if previous is None:
                        durable_publish_exclusive(staged, archive)
                    else:
                        # Do not replace a generation that changed while downloading.
                        with open_stable_regular_file(
                            archive, label="pinned archive cache", observed=previous
                        ):
                            pass
                        durable_replace(staged, archive)
                    admitted = stable_regular_file_identity(
                        archive, label="published pinned archive"
                    )
                    if (admitted.size, admitted.sha256) != (size, sha256):
                        raise ToolReleaseError(
                            "published archive changed before admission"
                        )
                    return archive
        finally:
            _release_file_lock(lock)
    except ValueError as exc:
        raise ToolReleaseError("provision_archive refused: " + str(exc)) from exc


@contextmanager
def open_pinned_archive(path: Path, *, size: int, sha256: str):
    """Bind the pin and every payload read to one owned descriptor lifetime."""
    try:
        if not path.is_file():
            raise ToolReleaseError(
                f"pinned archive absent: {path}; expected {size} bytes SHA-256 {sha256}; explicitly provision the input; consumer performs no automatic fetch"
            )
        with open_stable_regular_file(path, label="pinned archive") as opened:
            identity = stable_regular_file_handle_identity(
                opened, label="pinned archive", max_bytes=size
            )
            if (identity.size, identity.sha256) != (size, sha256):
                raise ToolReleaseError(f"pinned archive identity mismatch: {path}")
            yield opened
    except ValueError as exc:
        raise ToolReleaseError("open_pinned_archive refused: " + str(exc)) from exc


def _extract_member(archive: Path, asset: ToolAsset, destination: Path) -> str:
    destination = canonical_file_leaf(destination, create_parent=True)
    with OwnedTemporaryDirectory(prefix="molt-tool-", dir=destination.parent) as raw:
        staged_path = Path(raw) / "executable"
        # The closing source fence must succeed before publishing extracted bytes.
        with open_pinned_archive(
            archive, size=asset.size, sha256=asset.sha256
        ) as opened:
            with staged_path.open("xb") as staged:
                try:
                    if archive.name.endswith(".zip"):
                        with zipfile.ZipFile(opened.stream) as bundle:
                            with bundle.open(asset.archive_member) as source:
                                shutil.copyfileobj(source, staged, 1024 * 1024)
                    else:
                        with tarfile.open(fileobj=opened.stream, mode="r:*") as bundle:
                            entry = bundle.getmember(asset.archive_member)
                            if not entry.isfile():
                                raise ToolReleaseError(
                                    f"{archive.name}: {asset.archive_member} is not a regular file"
                                )
                            source = bundle.extractfile(entry)
                            if source is None:
                                raise ToolReleaseError(
                                    f"{archive.name}: cannot read {asset.archive_member}"
                                )
                            with source:
                                shutil.copyfileobj(source, staged, 1024 * 1024)
                except KeyError as exc:
                    raise ToolReleaseError(
                        f"{archive.name} has no member {asset.archive_member}"
                    ) from exc
        if os.name != "nt":
            staged_path.chmod(
                staged_path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH
            )
        captured = stable_regular_file_identity(
            staged_path, label="extracted pinned tool"
        )
        durable_replace(staged_path, destination)
        published = stable_regular_file_identity(
            destination, label="published pinned tool"
        )
        if (published.size, published.sha256) != (captured.size, captured.sha256):
            raise ToolReleaseError(
                "published tool differs from its pinned archive member"
            )
        return captured.sha256


def provision_tool(
    release: ToolRelease, toolchain_root: Path, *, downloads: Path | None = None
) -> ToolDiscovery:
    """Install ``release`` for this host under the toolchain root, or fail closed.

    Idempotent: a valid attested installation is returned untouched. Otherwise
    the host asset is downloaded (or reused from ``downloads`` when its size and
    digest already match), verified against the manifest, and its executable
    member is published before its attestation. Discovery admits only the exact
    matching pair, so an interrupted publication cannot attest different bytes.
    """
    try:
        existing = discover_tool(release, toolchain_root)
        if existing is not None:
            return existing
        asset = host_asset(release)
        archive = provision_archive(
            url=asset.url,
            size=asset.size,
            sha256=asset.sha256,
            downloads=Path(toolchain_root) / TOOLCHAINS_DIRNAME / DOWNLOADS_DIRNAME
            if downloads is None
            else downloads,
        )
        prefix = tool_prefix(toolchain_root, release)
        executable = tool_executable(prefix, release)
        executable_sha256 = _extract_member(archive, asset, executable)
        payload = _attestation_payload(release, asset, executable_sha256)
        attestation_path = prefix / TOOL_ATTESTATION_FILENAME
        with OwnedTemporaryDirectory(prefix="molt-attestation-", dir=prefix) as raw:
            staged = Path(raw) / TOOL_ATTESTATION_FILENAME
            staged.write_text(
                json.dumps(payload, sort_keys=True, indent=2) + "\n", "utf-8"
            )
            durable_replace(staged, attestation_path)
        discovered = discover_tool(release, toolchain_root)
        if discovered is None:
            raise ToolReleaseError(
                f"{release.name} {release.version} did not attest after provisioning"
            )
        return discovered
    except ValueError as exc:
        raise ToolReleaseError("provision_tool refused: " + str(exc)) from exc


def discover_pinned_tool(
    name: str, repo_root: Path | None = None
) -> ToolDiscovery | None:
    """Read the one manifest-owned installation, without PATH or network fallback."""
    from molt.dx import checkout_custody

    root = compiler_source_root() if repo_root is None else repo_root
    release = load_tool_releases(root).get(name)
    if release is None:
        return None
    return discover_tool(release, checkout_custody(root).toolchain_root)


def pinned_executable(name: str, repo_root: Path) -> Path | None:
    """Return the attested managed executable, if present.

    Optional host consumers such as Node retain their explicit host-selection
    policy. Compiler validation consumers use require_pinned_tool instead.
    """
    discovery = discover_pinned_tool(name, repo_root)
    return None if discovery is None else discovery.executable


def require_pinned_tool(name: str, repo_root: Path | None = None) -> ToolDiscovery:
    """Require the managed release; a stale ambient executable is never a substitute."""
    root = compiler_source_root() if repo_root is None else repo_root
    discovery = discover_pinned_tool(name, root)
    if discovery is None:
        release = tool_release(name, root)
        raise ToolReleaseError(
            f"{name} {release.version} is required from attested toolchain custody; "
            "artifact reuse is disabled until the validator is provisioned. "
            f"Run python -m molt.tool_releases provision {name} --repo-root "
            f'"{root}"'
        )
    return discovery


def run_pinned_tool(
    name: str,
    args: Sequence[str],
    *,
    run: Callable[..., _ToolResult],
    repo_root: Path | None = None,
    **kwargs: Any,
) -> _ToolResult:
    """Bind the caller's existing guarded runner to one attested tool generation."""
    discovery = require_pinned_tool(name, repo_root)
    try:
        with stable_executable_probe(discovery.executable, label=f"pinned {name}") as (
            entrypoint,
            identity,
        ):
            if identity.sha256 != discovery.executable_sha256:
                raise ToolReleaseError(f"{name} changed after attested discovery")
            return run([str(entrypoint), *args], **kwargs)
    except (OSError, StableRegularFileError) as exc:
        raise ToolReleaseError(f"{name} execution identity failed: {exc}") from exc


def main(argv: list[str] | None = None) -> int:
    import argparse

    from molt.dx import checkout_custody

    parser = argparse.ArgumentParser(
        description="Provision or inspect pinned tool releases under custody."
    )
    parser.add_argument("action", choices=("list", "discover", "provision"))
    parser.add_argument("tool", nargs="?")
    parser.add_argument("--repo-root", type=Path, default=Path.cwd())
    parser.add_argument(
        "--github-path",
        type=Path,
        help="append the attested executable directory to GitHub Actions PATH",
    )
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
    if args.github_path is not None:
        with args.github_path.open("a", encoding="utf-8") as path_file:
            path_file.write(str(discovery.executable.parent) + "\n")
    print(
        f"{release.name} {release.version}: {discovery.executable} "
        f"sha256={discovery.executable_sha256}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
