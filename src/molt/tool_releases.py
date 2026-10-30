"""Pinned, digest-bound releases of standalone tool binaries.

``config/tool_releases.toml`` is the one authority for tools that ship as
prebuilt release binaries, including ``wasm-tools`` and ``lune``. CI installs the same pin
(gated by tests/test_ci_workflow_topology.py), the proof plan's toolchain
policy cites it as setup evidence, and the proof queue provisions the host
asset into ``<toolchain root>/toolchains/<name>-<version>/bin`` before a lane that
declares the tool runs. A download is accepted only when its byte size and
SHA-256 match the manifest exactly; the installed executable carries an
attestation that discovery re-verifies by content hash on every use.
Explicit controller-seeded archives use that same pin and publication boundary
without opening the network; an invalid seed never falls back to a download.

A runtime tool, such as npm, is a package that another pinned tool runs: one
host-independent (``any``) asset whose ``archive_tree`` is extracted whole,
and an ``entrypoint`` that the ``runtime`` tool executes. ``pinned_command``
returns that exact argv (``node <prefix>/package/bin/npm-cli.js``), so no
consumer finds the tool, its runtime or a shim on PATH. Its attestation adds
the digest of every extracted file, which discovery re-verifies.
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
import unicodedata
import urllib.parse
import urllib.request
import zipfile
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any, TypeVar

from molt.dx import TOOLCHAINS_DIRNAME, DxConfigError, canonical_toolchain_root
from molt.file_deletion import delete_path
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
from molt.process_environment import ProcessEnvironment
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
TOOL_RELEASES_SCHEMA_VERSION = 3
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
# The npm registry's version document, whose `dist.integrity` (SHA-512) the
# pin mover checks the tarball against before it records the SHA-256.
PROVENANCE_NPM_REGISTRY = "npm-registry"
_PROVENANCE_KINDS = frozenset(
    {PROVENANCE_GITHUB_RELEASE, PROVENANCE_CHECKSUM_MANIFEST, PROVENANCE_NPM_REGISTRY}
)
_NPM_PACKAGE = r"(?:@[a-z0-9][a-z0-9._-]*/)?[a-z0-9][a-z0-9._-]*"
_NPM_REGISTRY_URL_RE = re.compile(
    rf"^https://registry\.npmjs\.org/(?P<package>{_NPM_PACKAGE})/"
    r"(?P<version>[0-9][A-Za-z0-9.+-]*)$"
)
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
# The one asset of a runtime tool: its bytes are the same on every host.
NOARCH_ASSET_KEY = "any"
# A runtime tool's extracted package, under its prefix.
PACKAGE_DIRNAME = "package"


class ToolReleaseError(RuntimeError):
    pass


@dataclass(frozen=True)
class ToolAsset:
    key: str
    url: str
    size: int
    sha256: str
    # Exactly one is set: the executable file of a native tool, or the
    # package directory of a runtime tool.
    archive_member: str | None = None
    archive_tree: str | None = None

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
    # A runtime tool names the pinned tool that runs its entrypoint, a path
    # inside its package.
    runtime: str | None = None
    entrypoint: str | None = None

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
    # A runtime tool's executable is its entrypoint script.
    executable: Path
    executable_sha256: str
    asset: ToolAsset
    # The digest of every file of a runtime tool's package.
    tree_sha256: str | None = None


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
    if release_id is not None:
        raise ToolReleaseError(f"{where}.provenance: a {kind} record has no release_id")
    if kind == PROVENANCE_NPM_REGISTRY:
        if not _NPM_REGISTRY_URL_RE.fullmatch(url):
            raise ToolReleaseError(
                f"{where}.provenance: url must be an npm registry version "
                f"document: {url}"
            )
        return ToolProvenance(kind=kind, url=url)
    if not _CHECKSUM_MANIFEST_URL_RE.fullmatch(url):
        raise ToolReleaseError(
            f"{where}.provenance: url must be an official SHASUMS256.txt "
            f"checksum manifest: {url}"
        )
    return ToolProvenance(kind=kind, url=url)


def npm_registry_tarball_url(provenance: ToolProvenance) -> str:
    """The tarball the registry serves for the version document ``provenance``."""
    match = _NPM_REGISTRY_URL_RE.fullmatch(provenance.url)
    if provenance.kind != PROVENANCE_NPM_REGISTRY or match is None:
        raise ToolReleaseError(f"not an npm registry record: {provenance.url}")
    package, version = match.group("package"), match.group("version")
    return (
        f"https://registry.npmjs.org/{package}/-/"
        f"{package.rsplit('/', 1)[-1]}-{version}.tgz"
    )


def _require_asset_url(url: str, provenance: ToolProvenance, *, where: str) -> None:
    if provenance.kind == PROVENANCE_GITHUB_RELEASE:
        if not _RELEASE_ASSET_URL_RE.fullmatch(url):
            raise ToolReleaseError(
                f"{where}: url must be a tag-addressed GitHub release asset: {url}"
            )
        return
    if provenance.kind == PROVENANCE_NPM_REGISTRY:
        expected = npm_registry_tarball_url(provenance)
        if url != expected:
            raise ToolReleaseError(
                f"{where}: url must be the registry tarball {expected}: {url}"
            )
        return
    directory = provenance.url.rsplit("/", 1)[0] + "/"
    name = url[len(directory) :]
    if not url.startswith(directory) or not name or "/" in name:
        raise ToolReleaseError(
            f"{where}: url must be an asset in the checksum manifest's own "
            f"version directory {directory}: {url}"
        )


def read_tool_releases_text(root: Path | None = None) -> tuple[Path, str]:
    """The manifest's path and text, for one parse of exactly those bytes."""
    path = tool_releases_path(root)
    try:
        return path, path.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as exc:
        raise ToolReleaseError(f"cannot read tool releases {path}: {exc}") from exc


def load_tool_releases(root: Path | None = None) -> dict[str, ToolRelease]:
    """Load and validate every pinned tool release."""
    return parse_tool_releases(*read_tool_releases_text(root))


def parse_tool_releases(path: Path, text: str) -> dict[str, ToolRelease]:
    """Validate every pinned tool release in ``text``, the manifest at ``path``."""
    try:
        payload = tomllib.loads(text)
    except tomllib.TOMLDecodeError as exc:
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
        runtime = raw.get("runtime")
        entrypoint = raw.get("entrypoint")
        if (runtime is None) != (entrypoint is None):
            raise ToolReleaseError(
                f"{where}: a runtime tool names both runtime and entrypoint"
            )
        if runtime is not None:
            runtime = _require_str(raw, "runtime", where=where)
            entrypoint = _relative_member(
                _require_str(raw, "entrypoint", where=where),
                where=f"{where}.entrypoint",
            )
        raw_assets = raw.get("assets")
        if not isinstance(raw_assets, Mapping) or not raw_assets:
            raise ToolReleaseError(f"{where}: assets must be a non-empty table")
        if runtime is not None and set(raw_assets) != {NOARCH_ASSET_KEY}:
            raise ToolReleaseError(
                f"{where}: a runtime tool has exactly one {NOARCH_ASSET_KEY!r} asset"
            )
        assets: dict[str, ToolAsset] = {}
        for key, raw_asset in raw_assets.items():
            asset_where = f"{where}.assets.{key}"
            if key not in _ASSET_KEYS and not (
                runtime is not None and key == NOARCH_ASSET_KEY
            ):
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
            field = "archive_member" if runtime is None else "archive_tree"
            if ("archive_tree" if runtime is None else "archive_member") in raw_asset:
                raise ToolReleaseError(f"{asset_where}: {field} names this asset")
            member = _relative_member(
                _require_str(raw_asset, field, where=asset_where),
                where=f"{asset_where}.{field}",
            )
            assets[key] = ToolAsset(
                key=key, url=url, size=size, sha256=sha256, **{field: member}
            )
        releases[name] = ToolRelease(
            name=name,
            version=version,
            executable=executable,
            provenance=provenance,
            assets=assets,
            runtime=runtime,
            entrypoint=entrypoint,
        )
    for release in releases.values():
        if release.runtime is None:
            continue
        runtime_release = releases.get(release.runtime)
        if runtime_release is None or runtime_release.runtime is not None:
            raise ToolReleaseError(
                f"{path}: tools.{release.name}: runtime {release.runtime!r} must be "
                "a pinned native tool"
            )
    return releases


def _relative_member(member: str, *, where: str) -> str:
    """An archive path that stays inside its archive."""
    if (
        not member
        or member.startswith(("/", "\\"))
        or "\\" in member
        or any(part in ("", ".", "..") for part in member.split("/"))
    ):
        raise ToolReleaseError(f"{where}: {member!r} escapes archive")
    return member


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
    noarch = release.assets.get(NOARCH_ASSET_KEY)
    if noarch is not None:
        return noarch
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
    if release.entrypoint is not None:
        return Path(prefix) / PACKAGE_DIRNAME / release.entrypoint
    return Path(prefix) / "bin" / release.executable_filename


def _sha256_file(path: Path) -> str:
    with path.open("rb") as handle:
        return hashlib.file_digest(handle, "sha256").hexdigest()


def tree_sha256(root: Path) -> str:
    """One digest of every file under ``root``: its path, mode and bytes.

    A symbolic link or any other entry that is not a file or directory fails
    the digest, as it fails extraction. POSIX hosts bind the owner's
    executable bit; other mode bits, empty directories and hard-link identity
    are outside it, as they are outside what a package's code can read.
    Verifying npm's 1,942 files costs about 0.11 s.
    """
    digest = hashlib.sha256(b"molt.tool-tree.v1\0")
    for directory, dirnames, filenames in os.walk(root):
        dirnames.sort()
        base = Path(directory)
        for name in sorted([*dirnames, *filenames]):
            path = base / name
            info = path.lstat()
            if stat.S_ISDIR(info.st_mode):
                continue
            if not stat.S_ISREG(info.st_mode):
                raise ToolReleaseError(f"{path} is not a regular file")
            executable = os.name != "nt" and bool(info.st_mode & stat.S_IXUSR)
            digest.update(path.relative_to(root).as_posix().encode("utf-8") + b"\0")
            digest.update(b"x\0" if executable else b"-\0")
            digest.update(_sha256_file(path).encode("ascii") + b"\n")
    return digest.hexdigest()


def _attestation_payload(
    release: ToolRelease,
    asset: ToolAsset,
    executable_sha256: str,
    package_sha256: str | None = None,
) -> dict[str, object]:
    payload: dict[str, object] = {
        "schema": TOOL_ATTESTATION_SCHEMA,
        "tool": release.name,
        "version": release.version,
        "asset": asset.key,
        "asset_url": asset.url,
        "asset_sha256": asset.sha256,
        "provenance": release.provenance.payload(),
        "executable": release.entrypoint or release.executable_filename,
        "executable_sha256": executable_sha256,
    }
    if package_sha256 is not None:
        payload["tree_sha256"] = package_sha256
    return payload


def discover_tool(release: ToolRelease, toolchain_root: Path) -> ToolDiscovery | None:
    """Return the attested installation of ``release`` under the root, if valid.

    Anything short of a byte-exact match between the attestation and the
    installed executable is not a discovery: the caller provisions afresh or
    fails closed, never trusts a partially matching install.
    """
    from molt import input_report

    prefix = tool_prefix(toolchain_root, release)
    executable = tool_executable(prefix, release)
    attestation_path = prefix / TOOL_ATTESTATION_FILENAME
    try:
        attestation = json.loads(attestation_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError):
        return None
    if not isinstance(attestation, Mapping):
        return None
    input_report.report_probe(executable)
    if not executable.is_file():
        return None
    asset = release.assets.get(str(attestation.get("asset") or ""))
    if asset is None:
        return None
    executable_sha256 = _sha256_file(executable)
    package_sha256 = None
    if release.entrypoint is not None:
        try:
            package_sha256 = tree_sha256(prefix / PACKAGE_DIRNAME)
        except (OSError, ValueError, ToolReleaseError):  # or an unencodable name
            return None
    if attestation != _attestation_payload(
        release, asset, executable_sha256, package_sha256
    ):
        return None
    return ToolDiscovery(
        release=release,
        prefix=prefix,
        executable=executable,
        executable_sha256=executable_sha256,
        asset=asset,
        tree_sha256=package_sha256,
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


@contextmanager
def _archive_input(
    *,
    url: str,
    size: int,
    sha256: str,
    redirects: _ArchiveRedirects,
    source: Path | None,
):
    """Read one explicit seed or HTTPS body; never retry one through the other."""
    if source is not None:
        with open_pinned_archive(source, size=size, sha256=sha256) as opened:
            yield opened.stream
        return
    request = urllib.request.Request(
        url,
        headers={
            "User-Agent": "molt-pinned-archive/1",
            "Accept-Encoding": "identity",
        },
    )
    opener = urllib.request.build_opener(redirects)
    with opener.open(request, timeout=180) as response:
        redirects.admit(response.geturl())
        if (
            response.status != 200
            or response.headers.get("Content-Encoding", "identity") != "identity"
        ):
            raise ToolReleaseError(
                "pinned archive response must be an unencoded HTTP 200 body"
            )
        declared = response.headers.get("Content-Length")
        if declared is not None and (
            not re.fullmatch(r"[0-9]+", declared) or int(declared) != size
        ):
            raise ToolReleaseError(
                "pinned archive response length disagrees with its pinned identity"
            )
        yield response


def provision_archive(
    *,
    url: str,
    size: int,
    sha256: str,
    downloads: Path,
    source: Path | None = None,
) -> Path:
    """Cache one exact archive through the shared pinned-tool transfer owner.

    This is explicit development provisioning. It never extracts or executes
    payloads. Cache readers still retain their own stable descriptor while
    consuming bytes. Invalid old entries survive any pre-publication failure.
    An explicit local ``source`` is verified and copied through the same pin
    and publication boundary without opening the network. Even a cache hit
    validates an explicit seed; invalid seeds are never silently ignored.
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
                        if source is not None:
                            with open_pinned_archive(source, size=size, sha256=sha256):
                                pass
                        return archive
                with OwnedTemporaryDirectory(
                    prefix="molt-archive-", dir=archive.parent
                ) as raw:
                    staged = Path(raw) / filename.name
                    digest = hashlib.sha256()
                    received = 0
                    with (
                        _archive_input(
                            url=url,
                            size=size,
                            sha256=sha256,
                            redirects=redirects,
                            source=source,
                        ) as response,
                        staged.open("xb") as stream,
                    ):
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
    except OSError as exc:  # urllib's URLError included: offline, refused, reset
        raise ToolReleaseError(f"cannot provision archive {url}: {exc}") from exc


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
    member = asset.archive_member
    if member is None:  # a package tree ships through _extract_tree
        raise ToolReleaseError(f"{asset.filename} names no archive member")
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
                            with bundle.open(member) as source:
                                shutil.copyfileobj(source, staged, 1024 * 1024)
                    else:
                        with tarfile.open(fileobj=opened.stream, mode="r:*") as bundle:
                            entry = bundle.getmember(member)
                            if not entry.isfile():
                                raise ToolReleaseError(
                                    f"{archive.name}: {member} is not a regular file"
                                )
                            source = bundle.extractfile(entry)
                            if source is None:
                                raise ToolReleaseError(
                                    f"{archive.name}: cannot read {member}"
                                )
                            with source:
                                shutil.copyfileobj(source, staged, 1024 * 1024)
                except KeyError as exc:
                    raise ToolReleaseError(
                        f"{archive.name} has no member {member}"
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


def _extract_tree(archive: Path, asset: ToolAsset, destination: Path) -> None:
    """Publish the archive's ``asset.archive_tree`` as the directory ``destination``.

    Only regular files and directories under the tree are admitted; a link,
    a device, a path outside the tree, or two paths that differ only in case
    (one directory on a case-insensitive volume) fail the extraction. The
    tree is staged beside ``destination`` and renamed into place under the
    release's provisioning lock. Replacing a tree no attestation admits takes
    two renames, so a reader can briefly find no tree; it then finds no
    attested installation, never a partial one.
    """
    assert asset.archive_tree is not None
    if archive.name.endswith(".zip"):
        raise ToolReleaseError(f"{archive.name}: a package tree ships as a tarball")
    prefix = asset.archive_tree + "/"
    destination.parent.mkdir(parents=True, exist_ok=True)
    with OwnedTemporaryDirectory(prefix="molt-tool-", dir=destination.parent) as raw:
        staged = Path(raw) / "tree"
        staged.mkdir()
        files = 0
        seen: dict[str, str] = {}
        with open_pinned_archive(
            archive, size=asset.size, sha256=asset.sha256
        ) as opened:
            with tarfile.open(fileobj=opened.stream, mode="r:*") as bundle:
                for entry in bundle:
                    name = entry.name.removeprefix("./")
                    if not name.startswith(prefix):
                        continue
                    relative = _relative_member(
                        name[len(prefix) :].rstrip("/"),
                        where=f"{archive.name}: {entry.name}",
                    )
                    parts = relative.split("/")
                    for depth in range(1, len(parts) + 1):
                        path = "/".join(parts[:depth])
                        folded = unicodedata.normalize("NFC", path).casefold()
                        if seen.setdefault(folded, path) != path:
                            raise ToolReleaseError(
                                f"{archive.name}: {entry.name} differs from "
                                f"{seen[folded]} only in case or normalization"
                            )
                    target = staged / relative
                    if not entry.isdir() and not entry.isfile():
                        raise ToolReleaseError(
                            f"{archive.name}: {entry.name} is not a regular file"
                        )
                    try:
                        if entry.isdir():
                            target.mkdir(parents=True, exist_ok=True)
                            continue
                        source = bundle.extractfile(entry)
                        if source is None:
                            raise ToolReleaseError(
                                f"{archive.name}: cannot read {entry.name}"
                            )
                        target.parent.mkdir(parents=True, exist_ok=True)
                        with source, target.open("xb") as sink:
                            shutil.copyfileobj(source, sink, 1024 * 1024)
                    except OSError as exc:  # a duplicate or a file/directory clash
                        raise ToolReleaseError(
                            f"{archive.name}: cannot extract {entry.name}: {exc}"
                        ) from exc
                    target.chmod(0o755 if entry.mode & stat.S_IXUSR else 0o644)
                    files += 1
        if files == 0:
            raise ToolReleaseError(f"{archive.name} has no files under {prefix}")
        if destination.exists():
            # A tree no attestation admits; the owned directory reclaims it.
            os.replace(destination, Path(raw) / "replaced")
        os.replace(staged, destination)


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
        prefix = tool_prefix(toolchain_root, release)
        prefix.parent.mkdir(parents=True, exist_ok=True)
        # One provisioner per release: a second waits, then finds the first's
        # attested installation instead of replacing files under its readers.
        lock_path = prefix.parent / f".molt-provision-{release.prefix_name}.lock"
        busy = f"{release.name} {release.version} is being provisioned: {lock_path}"
        try:
            lock = _acquire_file_lock(lock_path, timeout_s=600, timeout_message=busy)
        except RuntimeError as exc:
            if str(exc) != busy:
                raise
            raise ToolReleaseError(busy) from exc
        try:
            with _file_lock_owned_operation(lock, expected_lock_path=lock_path):
                return _provision_unlocked(release, toolchain_root, downloads)
        finally:
            _release_file_lock(lock)
    except ValueError as exc:
        raise ToolReleaseError("provision_tool refused: " + str(exc)) from exc
    except OSError as exc:
        raise ToolReleaseError(
            f"cannot provision {release.name} {release.version}: {exc}"
        ) from exc


def _provision_unlocked(
    release: ToolRelease, toolchain_root: Path, downloads: Path | None
) -> ToolDiscovery:
    existing = discover_tool(release, toolchain_root)
    if existing is not None:
        return existing
    prefix = tool_prefix(toolchain_root, release)
    # Under this release's lock, staging another provisioner left behind
    # (killed before its owned directory cleaned up) has no owner: a package
    # tree stages in the prefix, a native executable in its bin directory.
    for orphan in (
        *prefix.glob("molt-tool-*"),
        *(prefix / "bin").glob("molt-tool-*"),
        *prefix.glob("molt-attestation-*"),
    ):
        delete_path(orphan)
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
    package_sha256 = None
    if release.entrypoint is not None:
        package = prefix / PACKAGE_DIRNAME
        _extract_tree(archive, asset, package)
        if not executable.is_file():
            raise ToolReleaseError(
                f"{release.name} {release.version}: {asset.filename} has no "
                f"entrypoint {release.entrypoint}"
            )
        executable_sha256 = _sha256_file(executable)
        package_sha256 = tree_sha256(package)
    else:
        executable_sha256 = _extract_member(archive, asset, executable)
    payload = _attestation_payload(release, asset, executable_sha256, package_sha256)
    attestation_path = prefix / TOOL_ATTESTATION_FILENAME
    with OwnedTemporaryDirectory(prefix="molt-attestation-", dir=prefix) as raw:
        staged = Path(raw) / TOOL_ATTESTATION_FILENAME
        staged.write_text(json.dumps(payload, sort_keys=True, indent=2) + "\n", "utf-8")
        durable_replace(staged, attestation_path)
    discovered = discover_tool(release, toolchain_root)
    if discovered is None:
        raise ToolReleaseError(
            f"{release.name} {release.version} did not attest after provisioning"
        )
    return discovered


def discover_pinned_tool(
    name: str,
    repo_root: Path | None = None,
    *,
    environ: Mapping[str, str] | None = None,
    cwd: Path | None = None,
) -> ToolDiscovery | None:
    """Read the one manifest-owned installation, without PATH or network fallback."""
    root = compiler_source_root() if repo_root is None else repo_root
    release = load_tool_releases(root).get(name)
    if release is None:
        return None
    try:
        toolchain_root = canonical_toolchain_root(
            root, environ, require_exists=False, cwd=cwd
        )
    except DxConfigError as exc:
        raise ToolReleaseError(str(exc)) from exc
    return discover_tool(release, toolchain_root)


def pinned_executable(
    name: str, repo_root: Path, *, environ: Mapping[str, str] | None = None
) -> Path | None:
    """Return the attested managed executable, if present.

    Optional host consumers such as Node retain their explicit host-selection
    policy. Compiler validation consumers use require_pinned_tool instead.
    """
    discovery = discover_pinned_tool(name, repo_root, environ=environ)
    return None if discovery is None else discovery.executable


def require_pinned_tool(
    name: str,
    repo_root: Path | None = None,
    *,
    environ: Mapping[str, str] | None = None,
    cwd: Path | None = None,
) -> ToolDiscovery:
    """Require the managed release; a stale ambient executable is never a substitute."""
    root = compiler_source_root() if repo_root is None else repo_root
    discovery = discover_pinned_tool(name, root, environ=environ, cwd=cwd)
    if discovery is None:
        release = tool_release(name, root)
        raise ToolReleaseError(
            f"{name} {release.version} is required from attested toolchain custody; "
            "artifact reuse is disabled until the validator is provisioned. "
            f"Run python -m molt.tool_releases provision {name} --repo-root "
            f'"{root}"'
        )
    return discovery


@dataclass(frozen=True)
class PinnedCommand:
    """How to run one pinned tool, bound to the installations it runs."""

    tool: ToolDiscovery
    # The pinned tool that executes a runtime tool's entrypoint.
    runtime: ToolDiscovery | None = None

    @property
    def executable(self) -> ToolDiscovery:
        """The installation whose executable the operating system starts."""
        return self.tool if self.runtime is None else self.runtime

    @property
    def argv(self) -> tuple[str, ...]:
        if self.runtime is None:
            return (str(self.tool.executable),)
        return (str(self.runtime.executable), str(self.tool.executable))

    def environment(self, base: Mapping[str, str]) -> ProcessEnvironment:
        """``base`` with a runtime tool's runtime first on PATH.

        npm starts package launchers such as ``#!/usr/bin/env node`` through
        PATH; they must run on the pinned runtime, never another Node. PATH
        never selects the tool itself: ``argv`` names it exactly.
        """
        environment = ProcessEnvironment(base)
        if self.runtime is None:
            return environment
        directory = str(self.runtime.executable.parent)
        rest = [
            entry
            for entry in environment.get("PATH", "").split(os.pathsep)
            if entry and os.path.normcase(entry) != os.path.normcase(directory)
        ]
        return environment.updated({"PATH": os.pathsep.join([directory, *rest])})


def pinned_command(
    name: str,
    repo_root: Path | None = None,
    *,
    environ: Mapping[str, str] | None = None,
    cwd: Path | None = None,
    provision: bool = False,
) -> PinnedCommand:
    """The one way to run pinned tool ``name``: never PATH, never a shim.

    A native tool runs its attested executable; a runtime tool runs its
    entrypoint on its attested runtime (npm: ``node .../bin/npm-cli.js``).
    With ``provision`` an absent or stale installation is provisioned first
    (digest-verified, idempotent); otherwise it must already be attested.
    """
    root = compiler_source_root() if repo_root is None else repo_root
    release = tool_release(name, root)

    def installed(selected: ToolRelease) -> ToolDiscovery:
        if not provision:
            return require_pinned_tool(selected.name, root, environ=environ, cwd=cwd)
        try:
            toolchain_root = canonical_toolchain_root(
                root, environ, require_exists=False, cwd=cwd
            )
        except DxConfigError as exc:
            raise ToolReleaseError(str(exc)) from exc
        return provision_tool(selected, toolchain_root)

    tool = installed(release)
    if release.runtime is None:
        return PinnedCommand(tool)
    return PinnedCommand(tool, installed(tool_release(release.runtime, root)))


def run_command(name: str) -> tuple[str, ...]:
    """The argv prefix that provisions and runs pinned tool ``name``.

    For a step spelled as a command: ``[*run_command("npm"), "ci"]``.
    """
    return (sys.executable, "-m", "molt.tool_releases", "run", name, "--")


def run_pinned_tool(
    name: str,
    args: Sequence[str],
    *,
    run: Callable[..., _ToolResult],
    repo_root: Path | None = None,
    provision: bool = False,
    **kwargs: Any,
) -> _ToolResult:
    """Bind the caller's existing guarded runner to one attested tool generation."""
    command = pinned_command(
        name,
        repo_root,
        environ=kwargs.get("env"),
        cwd=kwargs.get("cwd"),
        provision=provision,
    )
    started = command.executable
    try:
        with stable_executable_probe(started.executable, label=f"pinned {name}") as (
            entrypoint,
            identity,
        ):
            if identity.sha256 != started.executable_sha256:
                raise ToolReleaseError(f"{name} changed after attested discovery")
            if command.runtime is not None:
                kwargs["env"] = command.environment(
                    kwargs.get("env") or os.environ
                ).child_block()
            return run([str(entrypoint), *command.argv[1:], *args], **kwargs)
    except (OSError, StableRegularFileError) as exc:
        raise ToolReleaseError(f"{name} execution identity failed: {exc}") from exc


def main(argv: list[str] | None = None) -> int:
    import argparse
    import subprocess

    parser = argparse.ArgumentParser(
        description="Provision, inspect or run pinned tool releases under custody.",
        epilog="run provisions the tool (and its runtime) first, then runs it "
        "with the arguments after --: python -m molt.tool_releases run npm -- ci",
    )
    parser.add_argument("action", choices=("list", "discover", "provision", "run"))
    parser.add_argument("tool", nargs="?")
    parser.add_argument("--repo-root", type=Path, default=compiler_source_root())
    parser.add_argument(
        "--github-path",
        type=Path,
        help="append the attested executable directory to GitHub Actions PATH",
    )
    argv = sys.argv[1:] if argv is None else list(argv)
    tool_arguments: list[str] = []
    if "--" in argv:
        split = argv.index("--")
        argv, tool_arguments = argv[:split], argv[split + 1 :]
    args = parser.parse_args(argv)
    if tool_arguments and args.action != "run":
        parser.error("only run takes tool arguments")
    releases = load_tool_releases(args.repo_root)
    if args.action == "list":
        for release in releases.values():
            print(f"{release.name} {release.version} assets={sorted(release.assets)}")
        return 0
    if not args.tool:
        parser.error("tool is required")
    release = tool_release(args.tool, args.repo_root)
    if args.action == "run":
        return run_pinned_tool(
            release.name,
            tool_arguments,
            run=subprocess.run,
            repo_root=args.repo_root,
            provision=True,
        ).returncode
    if args.github_path is not None and release.runtime is not None:
        parser.error(
            f"{release.name} runs on {release.runtime}; run it with "
            f"`python -m molt.tool_releases run {release.name} -- ...`, not PATH"
        )
    if args.action == "provision":
        discovery = pinned_command(release.name, args.repo_root, provision=True).tool
    else:
        toolchain_root = canonical_toolchain_root(args.repo_root, require_exists=False)
        discovery = discover_tool(release, toolchain_root)
        if discovery is None:
            print(
                f"{release.name} {release.version}: not provisioned under "
                f"{toolchain_root}"
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
