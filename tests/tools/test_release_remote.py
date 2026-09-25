"""Mocked GitHub custody: no network, no deletion, no overwrite, exact ID pins."""

from __future__ import annotations

import copy
import hashlib
import json
import os
from pathlib import Path
from types import SimpleNamespace
from urllib.parse import parse_qs, urlparse

import pytest

from tools.release import release_remote as remote

VERSION = "0.0.1"
SOURCE = "a" * 40
RELEASE_ID = 101
EVIDENCE_ID = 701
REPOSITORY = "molt-org/molt"
EVIDENCE = remote.release_exit_archive_filename(SOURCE)


class _GitHub:
    def __init__(self) -> None:
        self.release = {"id": RELEASE_ID, "tag_name": f"v{VERSION}", "draft": True}
        self.releases = [self.release]
        self.assets: list[dict[str, object]] = []
        self.content: dict[int, bytes] = {}
        self.commands: list[list[str]] = []
        self.uploads: list[str] = []
        self.downloads: list[int] = []
        self.after_upload = None
        self.after_download = None
        self.after_publish = None
        self.publications = 0
        self.git_source = SOURCE
        self.add(EVIDENCE, b"evidence", identifier=EVIDENCE_ID)

    def add(
        self,
        name: str,
        data: bytes,
        *,
        identifier: int | None = None,
        digest: bool = True,
    ) -> dict[str, object]:
        identifier = (
            identifier
            if identifier is not None
            else max(self.content, default=EVIDENCE_ID) + 1
        )
        item: dict[str, object] = {
            "id": identifier,
            "name": name,
            "size": len(data),
            "state": "uploaded",
            "digest": "sha256:" + hashlib.sha256(data).hexdigest() if digest else None,
            # Malicious server URL must never become command authority.
            "url": "https://attacker.invalid/credentials",
            "browser_download_url": "https://attacker.invalid/payload",
        }
        self.assets.append(item)
        self.content[identifier] = data
        return item

    def run(self, argv, **kwargs):
        argv = list(argv)
        self.commands.append(argv)
        assert "--clobber" not in argv and "DELETE" not in argv
        assert not any("attacker.invalid" in arg for arg in argv)
        if argv[0] == "git":
            assert argv[:4] == [
                "git",
                "ls-remote",
                "--exit-code",
                f"https://github.com/{REPOSITORY}.git",
            ]
            return SimpleNamespace(
                stdout=f"{self.git_source}\trefs/tags/v{VERSION}\n", returncode=0
            )
        assert argv[:4] == ["gh", "api", "--hostname", "github.com"]
        endpoint = argv[4]
        if endpoint == f"repos/{REPOSITORY}/releases?per_page=100":
            result = [self.releases]
        elif endpoint == f"repos/{REPOSITORY}/releases/{RELEASE_ID}":
            if "--method" in argv:
                assert argv[5:] == [
                    "--method",
                    "PATCH",
                    "--field",
                    "draft=false",
                    "--raw-field",
                    "make_latest=true",
                ]
                assert self.release["draft"] is True
                self.publications += 1
                self.release["draft"] = False
                if self.after_publish is not None:
                    self.after_publish()
            result = self.release
        elif (
            endpoint == f"repos/{REPOSITORY}/releases/{RELEASE_ID}/assets?per_page=100"
        ):
            result = [self.assets]
        elif endpoint.startswith(f"repos/{REPOSITORY}/releases/assets/"):
            identifier = int(endpoint.rsplit("/", 1)[1])
            assert "Accept: application/octet-stream" in argv
            self.downloads.append(identifier)
            os.write(kwargs["stdout"], self.content[identifier])
            if self.after_download is not None:
                self.after_download(identifier)
            return SimpleNamespace(returncode=0)
        elif endpoint.startswith("https://uploads.github.com/"):
            parsed = urlparse(endpoint)
            assert parsed.netloc == "uploads.github.com"
            assert parsed.path == f"/repos/{REPOSITORY}/releases/{RELEASE_ID}/assets"
            assert argv[argv.index("--method") + 1] == "POST"
            name = parse_qs(parsed.query)["name"][0]
            assert name not in {item["name"] for item in self.assets}
            source = Path(argv[argv.index("--input") + 1])
            self.uploads.append(name)
            result = self.add(name, source.read_bytes())
            if self.after_upload is not None:
                self.after_upload(result)
        else:
            raise AssertionError(f"unexpected endpoint {endpoint}")
        return SimpleNamespace(stdout=json.dumps(result), returncode=0)


@pytest.fixture
def github(monkeypatch: pytest.MonkeyPatch) -> _GitHub:
    api = _GitHub()
    monkeypatch.setattr(remote, "_COMMANDS", api)
    monkeypatch.setattr(
        remote,
        "load_config",
        lambda: {"repository": {"owner": "molt-org", "name": "molt"}},
    )
    return api


def _local(tmp_path: Path) -> Path:
    root = tmp_path / "local"
    root.mkdir()
    (root / EVIDENCE).write_bytes(b"evidence")
    (root / "payload.bin").write_bytes(b"compiled payload")
    (root / "release_manifest.json").write_text(
        json.dumps({"version": VERSION, "source_sha": SOURCE})
    )
    return root


def _stage(local: Path, *, verifier=None) -> None:
    def verified_snapshot(root: Path, *, source_sha: str) -> dict[str, Path]:
        assert root != local
        assert source_sha == SOURCE
        expected = {path.name: path.read_bytes() for path in local.iterdir()}
        assert {path.name: path.read_bytes() for path in root.iterdir()} == expected
        return {path.name: path for path in root.iterdir()}

    remote.stage_release(
        VERSION,
        SOURCE,
        release_id=RELEASE_ID,
        evidence_asset_id=EVIDENCE_ID,
        local=local,
        verify_local=verifier or verified_snapshot,
    )


@pytest.mark.parametrize(
    "variant", ["missing", "duplicate", "published", "identity", "extra-asset"]
)
def test_draft_admission_requires_one_pinned_exact_draft(
    github: _GitHub, variant: str
) -> None:
    if variant == "missing":
        github.releases = []
    elif variant == "duplicate":
        github.releases.append(copy.deepcopy(github.release))
    elif variant == "published":
        github.release["draft"] = False
    elif variant == "identity":
        github.release["id"] = RELEASE_ID + 1
    else:
        github.add("unexpected", b"extra")
    with pytest.raises(ValueError):
        remote.require_draft(VERSION, SOURCE, release_id=RELEASE_ID, evidence_only=True)
    assert not github.uploads


def test_evidence_pin_is_captured_only_in_evidence_only_state(github: _GitHub) -> None:
    assert remote.require_draft(VERSION, SOURCE, evidence_only=True) == RELEASE_ID
    assert (
        remote.require_evidence_asset_id(VERSION, SOURCE, release_id=RELEASE_ID)
        == EVIDENCE_ID
    )
    github.add("later-asset", b"later")
    with pytest.raises(ValueError, match="exactly its source-named evidence"):
        remote.require_evidence_asset_id(VERSION, SOURCE, release_id=RELEASE_ID)
    assert (
        remote.require_draft(
            VERSION, SOURCE, release_id=RELEASE_ID, evidence_asset_id=EVIDENCE_ID
        )
        == RELEASE_ID
    )


def test_download_evidence_routes_only_by_pinned_asset_id(
    tmp_path: Path, github: _GitHub
) -> None:
    output = tmp_path / "download"
    result = remote.download_evidence(
        VERSION,
        SOURCE,
        release_id=RELEASE_ID,
        evidence_asset_id=EVIDENCE_ID,
        output=output,
    )
    assert result == output / EVIDENCE
    assert result.read_bytes() == b"evidence"
    assert github.downloads == [EVIDENCE_ID]
    assert not github.uploads


def test_stage_preserves_original_evidence_and_uploads_only_missing(
    tmp_path: Path, github: _GitHub
) -> None:
    local = _local(tmp_path)
    original = copy.deepcopy(github.assets[0])
    _stage(local)
    assert github.assets[0] == original
    assert EVIDENCE not in github.uploads
    assert sorted(github.uploads) == ["payload.bin", "release_manifest.json"]
    assert {item["name"] for item in github.assets} == {
        path.name for path in local.iterdir()
    }
    _stage(local)
    assert len(github.uploads) == 2  # Resume is read-only for already matching bytes.


def test_missing_digest_metadata_requires_downloaded_byte_proof(
    tmp_path: Path, github: _GitHub
) -> None:
    github.assets[0]["digest"] = None
    _stage(_local(tmp_path))
    assert github.downloads.count(EVIDENCE_ID) == 2  # Admission and completion.
    assert EVIDENCE not in github.uploads


@pytest.mark.parametrize(
    "variant", ["digest", "bytes", "extra", "missing", "replaced-id", "pending"]
)
def test_stage_fails_closed_without_mutating_conflicting_remote_state(
    tmp_path: Path, github: _GitHub, variant: str
) -> None:
    local = _local(tmp_path)
    if variant == "digest":
        github.assets[0]["digest"] = "sha256:" + "0" * 64
    elif variant == "bytes":
        github.assets[0]["digest"] = None
        github.content[EVIDENCE_ID] = b"tampered"
    elif variant == "extra":
        github.add("foreign", b"foreign")
    elif variant == "missing":
        github.assets = []
    elif variant == "replaced-id":
        github.assets[0]["id"] = EVIDENCE_ID + 10
    else:
        github.assets[0]["state"] = "starter"
    with pytest.raises(ValueError):
        _stage(local)
    assert not github.uploads


def test_stage_runs_crypto_verifier_before_any_remote_mutation(
    tmp_path: Path, github: _GitHub
) -> None:
    def reject(_root: Path, **_kwargs):
        raise ValueError("signature verification failed")

    with pytest.raises(ValueError, match="signature verification failed"):
        _stage(_local(tmp_path), verifier=reject)
    assert github.commands == []


def test_stage_rechecks_upload_response_and_does_not_retry(
    tmp_path: Path, github: _GitHub
) -> None:
    github.after_upload = lambda item: item.__setitem__("digest", "sha256:" + "0" * 64)
    with pytest.raises(ValueError, match="digest differs"):
        _stage(_local(tmp_path))
    assert len(github.uploads) == 1
    assert github.assets[0]["id"] == EVIDENCE_ID


def test_stage_detects_deleted_replaced_evidence_before_next_upload(
    tmp_path: Path, github: _GitHub
) -> None:
    github.after_upload = lambda _item: github.assets[0].__setitem__(
        "id", EVIDENCE_ID + 10
    )
    with pytest.raises(ValueError, match="original evidence asset ID"):
        _stage(_local(tmp_path))
    assert len(github.uploads) == 1


def test_download_release_requires_exact_state_and_exclusive_output(
    tmp_path: Path, github: _GitHub
) -> None:
    github.add("payload.bin", b"payload")
    github.release["draft"] = False
    output = tmp_path / "public"
    with pytest.raises(ValueError, match="publication state"):
        remote.download_release(
            VERSION,
            SOURCE,
            release_id=RELEASE_ID,
            evidence_asset_id=EVIDENCE_ID,
            output=output,
        )
    assert not output.exists()
    files = remote.download_release(
        VERSION,
        SOURCE,
        release_id=RELEASE_ID,
        evidence_asset_id=EVIDENCE_ID,
        output=output,
        published=True,
    )
    assert set(files) == {EVIDENCE, "payload.bin"}
    before = (output / EVIDENCE).read_bytes()
    with pytest.raises(ValueError, match="already exists"):
        remote.download_release(
            VERSION,
            SOURCE,
            release_id=RELEASE_ID,
            evidence_asset_id=EVIDENCE_ID,
            output=output,
            published=True,
        )
    assert (output / EVIDENCE).read_bytes() == before


@pytest.mark.parametrize("value", [True, 0, -1, 1.5, "101"])
def test_release_ids_are_positive_exact_integers(
    tmp_path: Path, github: _GitHub, value: object
) -> None:
    with pytest.raises(ValueError, match="positive integer"):
        remote.download_release(
            VERSION,
            SOURCE,
            release_id=value,
            evidence_asset_id=EVIDENCE_ID,
            output=tmp_path / "out",
        )
    assert github.commands == []


@pytest.mark.parametrize("name", ["../escape", "CON", "sub/file", "bad\\file"])
def test_remote_names_cannot_be_paths(
    tmp_path: Path, github: _GitHub, name: str
) -> None:
    github.add(name, b"bad")
    with pytest.raises(ValueError):
        remote.download_release(
            VERSION,
            SOURCE,
            release_id=RELEASE_ID,
            evidence_asset_id=EVIDENCE_ID,
            output=tmp_path / "out",
        )
    assert not github.downloads


def test_remote_portable_name_collisions_fail_before_download(
    tmp_path: Path, github: _GitHub
) -> None:
    github.add("A", b"one")
    github.add("a", b"two")
    with pytest.raises(ValueError, match="duplicated"):
        remote.download_release(
            VERSION,
            SOURCE,
            release_id=RELEASE_ID,
            evidence_asset_id=EVIDENCE_ID,
            output=tmp_path / "out",
        )
    assert not github.downloads


def test_changed_download_bytes_are_never_published(
    tmp_path: Path, github: _GitHub
) -> None:
    github.content[EVIDENCE_ID] = b"tampered"
    with pytest.raises(ValueError, match="digest/size differs"):
        remote.download_evidence(
            VERSION,
            SOURCE,
            release_id=RELEASE_ID,
            evidence_asset_id=EVIDENCE_ID,
            output=tmp_path / "out",
        )
    assert not (tmp_path / "out").exists()
    assert not list(tmp_path.glob(".release-download-*"))


@pytest.mark.parametrize("identifier", [True, 0, -7, 701.0])
def test_remote_asset_ids_are_positive_exact_integers(
    tmp_path: Path, github: _GitHub, identifier: object
) -> None:
    github.assets[0]["id"] = identifier
    with pytest.raises(ValueError, match="positive integer"):
        remote.download_evidence(
            VERSION,
            SOURCE,
            release_id=RELEASE_ID,
            evidence_asset_id=EVIDENCE_ID,
            output=tmp_path / "out",
        )
    assert not github.downloads


def test_tag_movement_fails_before_asset_transfer(
    tmp_path: Path, github: _GitHub
) -> None:
    github.git_source = "b" * 40
    with pytest.raises(ValueError, match="tag differs from planned source"):
        remote.download_evidence(
            VERSION,
            SOURCE,
            release_id=RELEASE_ID,
            evidence_asset_id=EVIDENCE_ID,
            output=tmp_path / "out",
        )
    assert not github.downloads and not github.uploads


@pytest.mark.parametrize("initial_digest", [True, False])
def test_download_accepts_optional_digest_convergence_with_identical_bytes(
    tmp_path: Path, github: _GitHub, initial_digest: bool
) -> None:
    digest = github.assets[0]["digest"]
    github.assets[0]["digest"] = digest if initial_digest else None
    github.after_download = lambda _id: github.assets[0].__setitem__(
        "digest", None if initial_digest else digest
    )
    result = remote.download_evidence(
        VERSION,
        SOURCE,
        release_id=RELEASE_ID,
        evidence_asset_id=EVIDENCE_ID,
        output=tmp_path / "evidence",
    )
    assert result.read_bytes() == b"evidence"


@pytest.mark.parametrize("initial_digest", [True, False])
def test_stage_accepts_optional_digest_convergence_with_byte_proof(
    tmp_path: Path, github: _GitHub, initial_digest: bool
) -> None:
    digest = github.assets[0]["digest"]
    github.assets[0]["digest"] = digest if initial_digest else None
    github.after_upload = lambda _item: github.assets[0].__setitem__(
        "digest", None if initial_digest else digest
    )
    _stage(_local(tmp_path))
    assert EVIDENCE_ID in github.downloads
    assert len(github.uploads) == 2


def test_optional_digest_convergence_never_accepts_conflicting_nonnull_digest(
    tmp_path: Path, github: _GitHub
) -> None:
    github.after_download = lambda _id: github.assets[0].__setitem__(
        "digest", "sha256:" + "0" * 64
    )
    with pytest.raises(ValueError, match="digest differs"):
        remote.download_evidence(
            VERSION,
            SOURCE,
            release_id=RELEASE_ID,
            evidence_asset_id=EVIDENCE_ID,
            output=tmp_path / "evidence",
        )
    assert not (tmp_path / "evidence").exists()


def test_incomplete_asset_error_identifies_manual_audit_target(
    tmp_path: Path, github: _GitHub
) -> None:
    github.assets[0]["state"] = "starter"
    with pytest.raises(ValueError, match="ID=701.*state='starter'.*manual audit"):
        _stage(_local(tmp_path))
    assert not github.uploads


def _promote(local: Path, *, verifier=None, verify_copy=None) -> None:
    def authenticated(root: Path, *, source_sha: str) -> dict[str, Path]:
        assert root != local and source_sha == SOURCE
        return {path.name: path for path in root.iterdir()}

    def identical(files: dict[str, Path], downloaded: Path) -> None:
        if {name: path.read_bytes() for name, path in files.items()} != {
            path.name: path.read_bytes() for path in downloaded.iterdir()
        }:
            raise ValueError("remote copy differs from authenticated snapshot")

    remote.promote_release(
        VERSION,
        SOURCE,
        release_id=RELEASE_ID,
        evidence_asset_id=EVIDENCE_ID,
        local=local,
        verify_local=verifier or authenticated,
        verify_copy=verify_copy or identical,
    )


def test_promote_authenticates_once_and_verifies_draft_and_public_bytes(
    tmp_path: Path, github: _GitHub
) -> None:
    local = _local(tmp_path)
    authenticated_roots = []

    def authenticate(root: Path, **_kwargs):
        assert github.commands == []
        authenticated_roots.append(root)
        return {path.name: path for path in root.iterdir()}

    _promote(local, verifier=authenticate)
    assert len(authenticated_roots) == 1
    assert github.publications == 1
    assert github.release["draft"] is False
    assert len(github.uploads) == 2
    for item in github.assets:
        assert github.downloads.count(item["id"]) == 2


@pytest.mark.parametrize("failure", ["patch-response", "public-verification"])
def test_promote_resumes_after_publication_without_remote_writes(
    tmp_path: Path, github: _GitHub, failure: str
) -> None:
    local = _local(tmp_path)

    def fail_after_publish():
        raise OSError("publication response lost")

    def check_copy(_files, downloaded):
        if downloaded.name == "public":
            raise OSError("public verification interrupted")

    if failure == "patch-response":
        github.after_publish = fail_after_publish
    with pytest.raises(OSError):
        _promote(local, verify_copy=check_copy)
    assert github.release["draft"] is False
    assert github.publications == 1
    github.after_publish = None
    github.commands.clear()
    github.downloads.clear()
    _promote(local)
    assert not any(
        "POST" in command or "PATCH" in command for command in github.commands
    )
    assert set(github.downloads) == {item["id"] for item in github.assets}
    assert len(github.downloads) == len(github.assets)


@pytest.mark.parametrize("variant", ["extra", "missing", "bytes", "replaced-evidence"])
def test_public_resume_rejects_invalid_asset_set_or_content_without_writes(
    tmp_path: Path, github: _GitHub, variant: str
) -> None:
    local = _local(tmp_path)
    _promote(local)
    if variant == "extra":
        github.add("foreign", b"foreign")
    elif variant == "missing":
        github.assets = [
            item for item in github.assets if item["name"] != "payload.bin"
        ]
    elif variant == "bytes":
        github.content[EVIDENCE_ID] = b"tampered"
    else:
        github.assets[0]["id"] = EVIDENCE_ID + 100
    github.commands.clear()
    with pytest.raises(ValueError):
        _promote(local)
    assert not any(
        "POST" in command or "PATCH" in command for command in github.commands
    )


@pytest.mark.parametrize("variant", ["id", "extra", "digest", "tag"])
def test_promote_rechecks_whole_byte_verified_generation_before_patch(
    tmp_path: Path, github: _GitHub, variant: str
) -> None:
    def mutate_after_verification(_files, downloaded):
        assert downloaded.name == "draft"
        if variant == "extra":
            github.add("foreign", b"foreign")
        elif variant == "tag":
            github.git_source = "b" * 40
        else:
            payload = next(
                item for item in github.assets if item["name"] == "payload.bin"
            )
            payload["id" if variant == "id" else "digest"] = (
                999 if variant == "id" else "sha256:" + "0" * 64
            )

    with pytest.raises(ValueError):
        _promote(_local(tmp_path), verify_copy=mutate_after_verification)
    assert github.publications == 0
    assert github.release["draft"] is True


def test_promotion_reuses_authenticated_snapshot_after_original_local_mutation(
    tmp_path: Path, github: _GitHub
) -> None:
    local = _local(tmp_path)
    original = (local / "payload.bin").read_bytes()

    def authenticate(root: Path, **_kwargs):
        (local / "payload.bin").write_bytes(b"changed after snapshot")
        return {path.name: path for path in root.iterdir()}

    _promote(local, verifier=authenticate)
    payload = next(item for item in github.assets if item["name"] == "payload.bin")
    assert github.content[payload["id"]] == original


@pytest.mark.parametrize("published", [False, True])
def test_promotion_authenticates_before_any_remote_activity(
    tmp_path: Path, github: _GitHub, published: bool
) -> None:
    github.release["draft"] = not published

    def reject(_root, **_kwargs):
        raise ValueError("signature rejected")

    with pytest.raises(ValueError, match="signature rejected"):
        _promote(_local(tmp_path), verifier=reject)
    assert github.commands == []


@pytest.mark.parametrize("published", [False, True])
def test_promotion_rejects_wrong_local_source_before_remote_activity(
    tmp_path: Path, github: _GitHub, published: bool
) -> None:
    github.release["draft"] = not published
    local = _local(tmp_path)
    (local / "release_manifest.json").write_text(
        json.dumps({"version": VERSION, "source_sha": "b" * 40})
    )
    with pytest.raises(ValueError, match="pinned version/source"):
        _promote(local)
    assert github.commands == []


@pytest.mark.parametrize("initial_digest", [True, False])
def test_promote_accepts_digest_metadata_convergence_after_draft_byte_proof(
    tmp_path: Path, github: _GitHub, initial_digest: bool
) -> None:
    digest = github.assets[0]["digest"]
    github.assets[0]["digest"] = digest if initial_digest else None

    def converge(_files, downloaded):
        if downloaded.name == "draft":
            github.assets[0]["digest"] = None if initial_digest else digest

    _promote(_local(tmp_path), verify_copy=converge)
    assert github.publications == 1
