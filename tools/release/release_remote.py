"""Pinned GitHub release/asset custody; never delete or replace remote assets."""

from __future__ import annotations

from collections.abc import Callable, Iterator
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
import re
import subprocess
import tempfile
from typing import Any
from urllib.parse import quote

from molt.exact_json import loads_exact, read_exact
from molt.file_publication import (
    durable_publish_directory_exclusive,
    is_link_like,
    resolve_owned_path,
)
from molt.portable_paths import portable_path_component, portable_path_identity
from molt.toolchain_identity import (
    StableRegularFileIdentity,
    snapshot_stable_regular_file,
    stable_regular_file_identity,
    verify_stable_regular_file_identity,
)
from tools.command_execution import CommandExecutor
from tools.git_identity import require_git_object_id

from .release_model import (
    ROOT,
    load_config,
    normalized_version,
    release_exit_archive_filename,
)

_COMMANDS = CommandExecutor.for_file(__file__)


@dataclass(frozen=True, slots=True)
class _Asset:
    id: int
    name: str
    size: int
    sha256: str | None


@dataclass(frozen=True, slots=True)
class _Release:
    id: int
    assets: tuple[_Asset, ...]
    published: bool


@dataclass(frozen=True, slots=True)
class _SignedSnapshot:
    root: Path
    files: dict[str, Path]
    identities: dict[str, StableRegularFileIdentity]


def _positive_id(value: object, *, label: str) -> int:
    if type(value) is not int or value <= 0:
        raise ValueError(f"{label} must be a positive integer")
    return value


def _repository() -> str:
    config = load_config()["repository"]
    parts = (config["owner"], config["name"])
    if any(
        not isinstance(part, str)
        or part in {".", ".."}
        or re.fullmatch(r"[A-Za-z0-9_.-]+", part) is None
        for part in parts
    ):
        raise ValueError("canonical GitHub repository identity is invalid")
    return "/".join(parts)


def _api_json(endpoint: str, *arguments: str) -> Any:
    result = _COMMANDS.run(
        ["gh", "api", "--hostname", "github.com", endpoint, *arguments],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=300,
    )
    return loads_exact(result.stdout)


def _api_list(endpoint: str) -> list[dict[str, Any]]:
    pages = _api_json(endpoint, "--paginate", "--slurp")
    if (
        not isinstance(pages, list)
        or not all(isinstance(page, list) for page in pages)
        or not all(isinstance(item, dict) for page in pages for item in page)
    ):
        raise ValueError("GitHub release listing is not a paginated object list")
    return [item for page in pages for item in page]


def verify_remote_tag(version: str, source_sha: str) -> None:
    """Rebind the remote tag to the planned commit in the configured repository."""
    require_git_object_id(source_sha, label="planned release source")
    ref = f"refs/tags/v{normalized_version(version)}"
    peeled = ref + "^{}"
    result = _COMMANDS.run(
        [
            "git",
            "ls-remote",
            "--exit-code",
            f"https://github.com/{_repository()}.git",
            ref,
            peeled,
        ],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=120,
    )
    refs: dict[str, str] = {}
    for line in result.stdout.splitlines():
        fields = line.split("\t")
        if len(fields) != 2 or fields[1] not in {ref, peeled} or fields[1] in refs:
            raise ValueError("remote release tag query returned invalid refs")
        refs[fields[1]] = require_git_object_id(fields[0], label="remote release tag")
    if refs.get(peeled, refs.get(ref)) != source_sha:
        raise ValueError("remote release tag differs from planned source")


def _asset(value: object) -> _Asset:
    if not isinstance(value, dict):
        raise ValueError("GitHub release asset must be an object")
    identifier = _positive_id(value.get("id"), label="release asset ID")
    name = portable_path_component(value.get("name"))
    size = value.get("size")
    if type(size) is not int or size <= 0 or value.get("state") != "uploaded":
        raise ValueError(
            f"release asset {name!r} ID={identifier} has invalid "
            f"size={size!r}/state={value.get('state')!r}; manual audit required; "
            "no automatic deletion or retry"
        )
    digest = value.get("digest")
    if digest is not None:
        if (
            not isinstance(digest, str)
            or re.fullmatch(r"sha256:[0-9a-f]{64}", digest) is None
        ):
            raise ValueError(f"release asset {name!r} has invalid SHA256 metadata")
        digest = digest.removeprefix("sha256:")
    return _Asset(identifier, name, size, digest)


def _release_snapshot(
    version: str,
    source_sha: str,
    *,
    release_id: int | None,
    published: bool | None = False,
    evidence_only: bool = False,
    evidence_asset_id: int | None = None,
) -> _Release:
    """Pin tag, release ID, state and an exact collision-free asset inventory."""
    if release_id is not None:
        _positive_id(release_id, label="release ID")
    if evidence_asset_id is not None:
        _positive_id(evidence_asset_id, label="original evidence asset ID")
    verify_remote_tag(version, source_sha)
    repository = _repository()
    tag = f"v{normalized_version(version)}"
    releases = _api_list(f"repos/{repository}/releases?per_page=100")
    matches = [item for item in releases if item.get("tag_name") == tag]
    if len(matches) != 1:
        raise ValueError(
            "release admission requires exactly one existing release for its tag"
        )
    selected = matches[0]
    identifier = _positive_id(selected.get("id"), label="release ID")
    if release_id is not None and identifier != release_id:
        raise ValueError("release draft identity changed")
    detail = _api_json(f"repos/{repository}/releases/{identifier}")
    if (
        type(selected.get("draft")) is not bool
        or (published is not None and selected["draft"] is not (not published))
        or not isinstance(detail, dict)
        or type(detail.get("id")) is not int
        or detail["id"] != identifier
        or detail.get("tag_name") != tag
        or detail.get("draft") is not selected["draft"]
    ):
        raise ValueError("pinned release tag, identity or publication state changed")
    assets = tuple(
        sorted(
            (
                _asset(item)
                for item in _api_list(
                    f"repos/{repository}/releases/{identifier}/assets?per_page=100"
                )
            ),
            key=lambda item: item.name,
        )
    )
    if len({item.id for item in assets}) != len(assets) or len(
        {portable_path_identity(item.name) for item in assets}
    ) != len(assets):
        raise ValueError("release asset names or IDs are duplicated")
    if evidence_only and (
        len(assets) != 1 or assets[0].name != release_exit_archive_filename(source_sha)
    ):
        raise ValueError(
            "initial release draft must contain exactly its source-named evidence ZIP"
        )
    if evidence_asset_id is not None and not any(
        asset.name == release_exit_archive_filename(source_sha)
        and asset.id == evidence_asset_id
        for asset in assets
    ):
        raise ValueError("pinned release lost its original evidence asset ID")
    return _Release(identifier, assets, not selected["draft"])


def _reconcile_assets(
    before: tuple[_Asset, ...],
    after: tuple[_Asset, ...],
    identities: dict[str, StableRegularFileIdentity],
    *,
    scratch: Path | None = None,
) -> None:
    """Preserve generations; optional digest metadata is not asset identity.

    Identities must authenticate the corresponding content. Download callers
    already proved remote bytes; staging callers supply scratch to prove any
    optional-metadata transition by downloading that same immutable asset ID.
    """
    previous = {asset.name: asset for asset in before}
    current = {asset.name: asset for asset in after}
    if previous.keys() != current.keys():
        raise ValueError("pinned release asset generation set changed")
    if previous.keys() - identities.keys():
        raise ValueError("pinned release contains assets outside the signed local set")
    for name, old in previous.items():
        new = current[name]
        expected = identities[name]
        if (old.id, old.name, old.size) != (new.id, new.name, new.size):
            raise ValueError(f"pinned release asset generation changed: {name}")
        if old.size != expected.size or any(
            digest is not None and digest != expected.sha256
            for digest in (old.sha256, new.sha256)
        ):
            raise ValueError(f"pinned release asset digest differs: {name}")
        if scratch is not None and old.sha256 != new.sha256:
            with tempfile.TemporaryDirectory(dir=scratch) as temporary:
                downloaded = _download_asset(new, Path(temporary) / name)
                if downloaded.sha256 != expected.sha256:
                    raise ValueError(f"pinned release asset digest differs: {name}")


def require_draft(
    version: str,
    source_sha: str,
    *,
    release_id: int | None = None,
    evidence_only: bool = False,
    evidence_asset_id: int | None = None,
) -> int:
    return _release_snapshot(
        version,
        source_sha,
        release_id=release_id,
        evidence_only=evidence_only,
        evidence_asset_id=evidence_asset_id,
    ).id


def require_evidence_asset_id(
    version: str,
    source_sha: str,
    *,
    release_id: int,
    evidence_asset_id: int | None = None,
) -> int:
    """Capture the original asset generation while the draft is evidence-only."""
    return (
        _release_snapshot(
            version,
            source_sha,
            release_id=release_id,
            evidence_only=True,
            evidence_asset_id=evidence_asset_id,
        )
        .assets[0]
        .id
    )


def _download_asset(asset: _Asset, destination: Path) -> StableRegularFileIdentity:
    """Stream one enumerated asset ID, never a tag address or supplied URL."""
    with destination.open("xb") as stream:
        _COMMANDS.run(
            [
                "gh",
                "api",
                "--hostname",
                "github.com",
                f"repos/{_repository()}/releases/assets/{asset.id}",
                "--header",
                "Accept: application/octet-stream",
            ],
            cwd=ROOT,
            check=True,
            stdout=stream.fileno(),
            stderr=subprocess.PIPE,
            timeout=600,
        )
    identity = stable_regular_file_identity(
        destination, label="downloaded release asset"
    )
    if identity.size != asset.size or (
        asset.sha256 is not None and identity.sha256 != asset.sha256
    ):
        raise ValueError(f"downloaded release asset digest/size differs: {asset.name}")
    return identity


def _download_release(
    version: str,
    source_sha: str,
    *,
    release_id: int,
    output: Path,
    published: bool,
    evidence_only: bool,
    evidence_asset_id: int,
    expected_release: _Release | None = None,
    expected_identities: dict[str, StableRegularFileIdentity] | None = None,
) -> tuple[dict[str, Path], _Release]:
    _positive_id(release_id, label="release ID")
    _positive_id(evidence_asset_id, label="original evidence asset ID")
    output = resolve_owned_path(output)
    if output.exists() or is_link_like(output):
        raise ValueError("release download output already exists")
    release = _release_snapshot(
        version,
        source_sha,
        release_id=release_id,
        published=published,
        evidence_only=evidence_only,
        evidence_asset_id=evidence_asset_id,
    )
    if expected_release is not None:
        if expected_identities is None:
            raise ValueError("expected release requires authenticated identities")
        _reconcile_assets(expected_release.assets, release.assets, expected_identities)
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(
        prefix=".release-download-", dir=output.parent
    ) as temporary:
        stage = Path(temporary) / "assets"
        stage.mkdir()
        identities = {
            asset.name: _download_asset(asset, stage / asset.name)
            for asset in release.assets
        }
        current = _release_snapshot(
            version,
            source_sha,
            release_id=release_id,
            published=published,
            evidence_only=evidence_only,
            evidence_asset_id=evidence_asset_id,
        )
        _reconcile_assets(release.assets, current.assets, identities)
        for identity in identities.values():
            verify_stable_regular_file_identity(
                identity, label="downloaded release asset"
            )
        durable_publish_directory_exclusive(stage, output)
    return {asset.name: output / asset.name for asset in release.assets}, current


def download_evidence(
    version: str,
    source_sha: str,
    *,
    release_id: int,
    evidence_asset_id: int,
    output: Path,
) -> Path:
    files, _ = _download_release(
        version,
        source_sha,
        release_id=release_id,
        output=output,
        published=False,
        evidence_only=True,
        evidence_asset_id=evidence_asset_id,
    )
    return files[release_exit_archive_filename(source_sha)]


def download_release(
    version: str,
    source_sha: str,
    *,
    release_id: int,
    evidence_asset_id: int,
    output: Path,
    published: bool = False,
) -> dict[str, Path]:
    files, _ = _download_release(
        version,
        source_sha,
        release_id=release_id,
        output=output,
        published=published,
        evidence_only=False,
        evidence_asset_id=evidence_asset_id,
    )
    return files


def _matching_asset(
    asset: _Asset,
    expected: StableRegularFileIdentity,
    *,
    scratch: Path,
) -> None:
    if asset.size != expected.size:
        raise ValueError(f"existing release asset size differs: {asset.name}")
    digest = asset.sha256
    if digest is None:
        # Older GH assets omit digest metadata. Establish bytes instead of
        # treating missing metadata as permission to skip or replace an asset.
        destination = scratch / f"asset-{asset.id}"
        identity = _download_asset(asset, destination)
        digest = identity.sha256
    if digest != expected.sha256:
        raise ValueError(f"existing release asset digest differs: {asset.name}")


def _upload_asset(release_id: int, source: StableRegularFileIdentity) -> _Asset:
    verify_stable_regular_file_identity(source, label="release upload snapshot")
    endpoint = (
        f"https://uploads.github.com/repos/{_repository()}/releases/{release_id}/assets"
        f"?name={quote(source.path.name, safe='')}"
    )
    response = _api_json(
        endpoint,
        "--method",
        "POST",
        "--header",
        "Content-Type: application/octet-stream",
        "--input",
        str(source.path),
    )
    verify_stable_regular_file_identity(source, label="release upload snapshot")
    asset = _asset(response)
    if asset.name != source.path.name:
        raise ValueError("release upload response names a different asset")
    return asset


@contextmanager
def _signed_snapshot(
    version: str,
    source_sha: str,
    *,
    local: Path,
    verify_local: Callable[..., dict[str, Path]],
) -> Iterator[_SignedSnapshot]:
    """Authenticate one immutable local tree for the entire remote transaction."""
    require_git_object_id(source_sha, label="planned release source")
    local = resolve_owned_path(local)
    with tempfile.TemporaryDirectory(
        prefix=".release-upload-", dir=local.parent
    ) as temporary:
        root = Path(temporary)
        stage = root / "signed"
        stage.mkdir()
        identities: dict[str, StableRegularFileIdentity] = {}
        for source in sorted(local.iterdir()):
            name = portable_path_component(source.name)
            if is_link_like(source) or not source.is_file():
                raise ValueError("local release contains a non-regular file")
            snapshot = snapshot_stable_regular_file(
                source, stage / name, label="release upload"
            )
            identities[name] = snapshot.snapshot
        verified = verify_local(stage, source_sha=source_sha)
        if verified != {name: stage / name for name in identities}:
            raise ValueError("signed release verifier returned a different asset set")
        manifest = read_exact(
            stage / "release_manifest.json",
            max_bytes=4 * 1024 * 1024,
            label="release manifest",
        )
        if (
            not isinstance(manifest, dict)
            or manifest.get("version") != normalized_version(version)
            or manifest.get("source_sha") != source_sha
        ):
            raise ValueError("signed local release differs from pinned version/source")
        for identity in identities.values():
            verify_stable_regular_file_identity(
                identity, label="signed release snapshot"
            )
        yield _SignedSnapshot(root, verified, identities)


def _verify_snapshot(snapshot: _SignedSnapshot) -> None:
    for identity in snapshot.identities.values():
        verify_stable_regular_file_identity(identity, label="signed release snapshot")


def _stage_snapshot(
    version: str,
    source_sha: str,
    *,
    release_id: int,
    evidence_asset_id: int,
    snapshot: _SignedSnapshot,
) -> _Release:
    root, identities = snapshot.root, snapshot.identities
    release = _release_snapshot(
        version,
        source_sha,
        release_id=release_id,
        evidence_asset_id=evidence_asset_id,
    )
    observed = {asset.name: asset for asset in release.assets}
    evidence_name = release_exit_archive_filename(source_sha)
    if evidence_name not in observed or evidence_name not in identities:
        raise ValueError("pinned draft lost its original source-named evidence")
    if set(observed) - identities.keys():
        raise ValueError("pinned draft contains assets outside the signed local set")
    existing = root / "existing"
    existing.mkdir()
    for asset in release.assets:
        _matching_asset(asset, identities[asset.name], scratch=existing)
    for name in sorted(identities.keys() - observed.keys()):
        # No retry/overwrite if another actor changed this asset generation.
        current = _release_snapshot(
            version,
            source_sha,
            release_id=release_id,
            evidence_asset_id=evidence_asset_id,
        )
        _reconcile_assets(
            tuple(observed.values()), current.assets, identities, scratch=root
        )
        observed = {asset.name: asset for asset in current.assets}
        uploaded = _upload_asset(release_id, identities[name])
        uploaded_scratch = root / f"uploaded-{uploaded.id}"
        uploaded_scratch.mkdir()
        _matching_asset(uploaded, identities[name], scratch=uploaded_scratch)
        if uploaded.id in {asset.id for asset in observed.values()}:
            raise ValueError("release upload response reused an existing asset ID")
        observed[name] = uploaded
    final = _release_snapshot(
        version,
        source_sha,
        release_id=release_id,
        evidence_asset_id=evidence_asset_id,
    )
    _reconcile_assets(tuple(observed.values()), final.assets, identities, scratch=root)
    if set(observed) != set(identities):
        raise ValueError("pinned draft assets changed after upload")
    final_scratch = root / "final"
    final_scratch.mkdir()
    for asset in final.assets:
        _matching_asset(asset, identities[asset.name], scratch=final_scratch)
    _verify_snapshot(snapshot)
    return final


def stage_release(
    version: str,
    source_sha: str,
    *,
    release_id: int,
    evidence_asset_id: int,
    local: Path,
    verify_local: Callable[..., dict[str, Path]],
) -> None:
    """Authenticate once, preserve matching generations, upload only absent names."""
    _positive_id(release_id, label="release ID")
    _positive_id(evidence_asset_id, label="original evidence asset ID")
    with _signed_snapshot(
        version, source_sha, local=local, verify_local=verify_local
    ) as snapshot:
        _stage_snapshot(
            version,
            source_sha,
            release_id=release_id,
            evidence_asset_id=evidence_asset_id,
            snapshot=snapshot,
        )


def promote_release(
    version: str,
    source_sha: str,
    *,
    release_id: int,
    evidence_asset_id: int,
    local: Path,
    verify_local: Callable[..., dict[str, Path]],
    verify_copy: Callable[[dict[str, Path], Path], None],
) -> None:
    """Resume a pinned release from draft or public state without replacement.

    Authenticate the signed snapshot once, verify complete remote bytes before
    and after publication. Public resumption performs no POST/PATCH. A failed
    PATCH is never retried here: a subsequent invocation rediscovers its state.
    GitHub has no conditional PATCH; pre/post generation checks bound, but cannot
    atomically exclude, external writers across the publication request.
    """
    _positive_id(release_id, label="release ID")
    _positive_id(evidence_asset_id, label="original evidence asset ID")
    with _signed_snapshot(
        version, source_sha, local=local, verify_local=verify_local
    ) as snapshot:
        release = _release_snapshot(
            version,
            source_sha,
            release_id=release_id,
            published=None,
            evidence_asset_id=evidence_asset_id,
        )
        if not release.published:
            release = _stage_snapshot(
                version,
                source_sha,
                release_id=release_id,
                evidence_asset_id=evidence_asset_id,
                snapshot=snapshot,
            )
            draft = snapshot.root / "draft"
            _, verified_release = _download_release(
                version,
                source_sha,
                release_id=release_id,
                evidence_asset_id=evidence_asset_id,
                output=draft,
                published=False,
                evidence_only=False,
                expected_release=release,
                expected_identities=snapshot.identities,
            )
            verify_copy(snapshot.files, draft)
            _verify_snapshot(snapshot)
            release = _release_snapshot(
                version,
                source_sha,
                release_id=release_id,
                evidence_asset_id=evidence_asset_id,
            )
            # The entire generation map must still be the one just byte-verified.
            _reconcile_assets(
                verified_release.assets, release.assets, snapshot.identities
            )
            _api_json(
                f"repos/{_repository()}/releases/{release_id}",
                "--method",
                "PATCH",
                "--field",
                "draft=false",
                "--raw-field",
                "make_latest=true",
            )
        public = snapshot.root / "public"
        _download_release(
            version,
            source_sha,
            release_id=release_id,
            evidence_asset_id=evidence_asset_id,
            output=public,
            published=True,
            evidence_only=False,
            expected_release=release,
            expected_identities=snapshot.identities,
        )
        verify_copy(snapshot.files, public)
        _verify_snapshot(snapshot)
