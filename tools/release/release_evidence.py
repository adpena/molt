"""Source-bound semantic admission and portable release-evidence custody.

The semantic predicates remain in release_exit_gate and phase_exit_manifest.
This module binds their verified bytes to the archive that will be published.
"""

from __future__ import annotations

from pathlib import Path
import tempfile

from molt.exact_json import read_exact
from molt.file_publication import (
    durable_publish_directory_exclusive,
    durable_publish_exclusive,
)
from molt.portable_paths import portable_relative_path
from molt.toolchain_identity import (
    stable_regular_file_identity,
    verify_stable_regular_file_identity,
)
from molt.verified_subset import verified_subset_coordinates
from tools.command_execution import CommandExecutor
from tools.git_identity import require_git_object_id
from tools import phase_exit_manifest, release_exit_gate

from .archive import extract_zip_strict, same_regular_file_bytes, write_reproducible_zip
from .release_model import (
    ROOT,
    RELEASE_EXIT_ARCHIVE_KIND,
    file_record,
    load_config,
    release_exit_archive_filename,
    stable_release,
    phase_exit_attestation_filename,
)


_COMMANDS = CommandExecutor.for_file(__file__)


def verify_provenance(
    path: Path,
    *,
    source_sha: str,
    workflow: str,
    bundle: Path | None = None,
    predicate_type: str = "https://slsa.dev/provenance/v1",
) -> None:
    """One cryptographic origin policy for semantic and published subjects."""
    config = load_config()["repository"]
    repository = f"{config['owner']}/{config['name']}"
    identity = stable_regular_file_identity(path, label="attestation subject")
    bundle_identity = (
        stable_regular_file_identity(bundle, label="Sigstore bundle")
        if bundle
        else None
    )
    _COMMANDS.run(
        [
            "gh",
            "attestation",
            "verify",
            str(path),
            "--repo",
            repository,
            "--signer-workflow",
            f"{repository}/.github/workflows/{workflow}",
            "--source-digest",
            source_sha,
            "--deny-self-hosted-runners",
            "--predicate-type",
            predicate_type,
            *(["--bundle", str(bundle)] if bundle else []),
        ],
        cwd=ROOT,
        check=True,
        timeout=120,
    )
    verify_stable_regular_file_identity(identity, label="verified attestation subject")
    if bundle_identity is not None:
        verify_stable_regular_file_identity(
            bundle_identity, label="verified Sigstore bundle"
        )


def verify_release_exit_manifest(
    manifest: Path, *, source_sha: str, repo_root: Path = ROOT
) -> release_exit_gate.ReleaseGateReport:
    require_git_object_id(source_sha, label="release source SHA")
    report = release_exit_gate.verify_release_bundle(manifest, repo_root=repo_root)
    if report.source_sha != source_sha:
        raise ValueError(
            f"release-exit bundle source differs from release plan: expected {source_sha}, got {report.source_sha}"
        )
    if not report.passed or report.problems:
        raise ValueError(
            "release-exit gate did not pass: " + "; ".join(report.problems)
        )
    return report


def verify_e3_provenance(manifest: Path, *, source_sha: str) -> None:
    """Authenticate the exact E3 receipt set, never an optional filesystem glob."""
    payload = read_exact(
        manifest, max_bytes=16 * 1024 * 1024, label="release-exit manifest"
    )
    roles = {
        release_exit_gate.verified_subset_evidence_role(cell.id)
        for cell in verified_subset_coordinates()
    }
    rows = [row for row in payload["evidence"] if row["role"] in roles]
    if len(rows) != len(roles) or {row["role"] for row in rows} != roles:
        raise ValueError("release-exit provenance requires every exact E3 receipt")
    for row in sorted(rows, key=lambda row: row["role"]):
        receipt = manifest.parent / portable_relative_path(row["path"])
        identity = stable_regular_file_identity(receipt, label="E3 provenance subject")
        if (identity.sha256, identity.size) != (row["sha256"], row["size"]):
            raise ValueError(f"E3 provenance subject changed: {row['role']}")
        verify_provenance(
            receipt, source_sha=source_sha, workflow="verified-subset.yml"
        )
        verify_stable_regular_file_identity(
            identity, label="verified E3 provenance subject"
        )


def verify_evidence(
    manifest: Path,
    *,
    source_sha: str,
    version: str,
    phase_manifest: Path | None = None,
    repo_root: Path = ROOT,
    authenticate: bool = False,
) -> None:
    verify_release_exit_manifest(manifest, source_sha=source_sha, repo_root=repo_root)
    if stable_release(version):
        if phase_manifest is None:
            raise ValueError(
                f"release {version} requires a green H0 phase-exit manifest for {source_sha}"
            )
        report = phase_exit_manifest.verify_phase_manifest(
            phase_manifest,
            release_commit=source_sha,
            bundle_manifest=manifest,
            root=repo_root,
        )
        if (
            report.phase != "H0"
            or report.commit != source_sha
            or not report.green
            or report.problems
        ):
            raise ValueError(
                "H0 phase exit is not green: " + "; ".join(report.problems)
            )
        payload = read_exact(
            phase_manifest, max_bytes=16 * 1024 * 1024, label="H0 phase manifest"
        )
        if payload["signed_attestation"]["path"] != phase_exit_attestation_filename(
            source_sha
        ):
            raise ValueError("H0 attestation must use its source-named release asset")
        if authenticate:
            with tempfile.TemporaryDirectory(
                prefix=".phase-subject-", dir=phase_manifest.parent
            ) as temporary:
                subject = Path(temporary) / "subject.json"
                subject.write_bytes(phase_exit_manifest.signing_subject_bytes(payload))
                verify_provenance(
                    subject,
                    source_sha=source_sha,
                    workflow="release.yml",
                    bundle=phase_manifest.parent
                    / payload["signed_attestation"]["path"],
                )
    elif phase_manifest is not None:
        raise ValueError("pre-stable releases must not claim an H0 phase exit")
    if authenticate:
        verify_e3_provenance(manifest, source_sha=source_sha)


def archive_release_exit(
    *,
    manifest: Path,
    source_sha: str,
    source_date_epoch: int,
    output: Path,
    repo_root: Path = ROOT,
) -> dict[str, object]:
    """Publish only after the archived bytes themselves pass the semantic gate."""
    expected_name = release_exit_archive_filename(source_sha)
    if output.name != expected_name or manifest.name != "release-exit.json":
        raise ValueError(
            f"release-exit archive must be named {expected_name}; manifest must be release-exit.json"
        )
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(
        prefix=".release-evidence-", dir=output.parent
    ) as temporary:
        stage = Path(temporary)
        archive = stage / expected_name
        write_reproducible_zip(
            manifest.parent,
            archive,
            source_date_epoch=source_date_epoch,
            mode_resolver=lambda _path: 0o644,
        )
        extracted = stage / "verified"
        extract_zip_strict(archive, extracted)
        verify_release_exit_manifest(
            extracted / "release-exit.json", source_sha=source_sha, repo_root=repo_root
        )
        identity = stable_regular_file_identity(
            archive, label="verified evidence archive"
        )
        durable_publish_exclusive(archive, output)
        result = file_record(output, kind=RELEASE_EXIT_ARCHIVE_KIND)
        if (result["sha256"], result["size"]) != (identity.sha256, identity.size):
            raise ValueError("published evidence archive differs from verified bytes")
        return result


def extract_release_exit(
    *,
    archive: Path,
    source_sha: str,
    source_date_epoch: int,
    output: Path,
    repo_root: Path = ROOT,
    version: str | None = None,
    phase_manifest: Path | None = None,
    authenticate: bool = False,
) -> Path:
    """Verify and canonically re-encode in private staging before publication."""
    expected_name = release_exit_archive_filename(source_sha)
    if archive.name != expected_name:
        raise ValueError(f"release-exit archive must be named {expected_name}")
    identity = stable_regular_file_identity(archive, label="staged release evidence")
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(
        prefix=".release-extract-", dir=output.parent
    ) as temporary:
        stage = Path(temporary)
        extracted = stage / "bundle"
        extract_zip_strict(archive, extracted)
        if version is None:
            verify_release_exit_manifest(
                extracted / "release-exit.json",
                source_sha=source_sha,
                repo_root=repo_root,
            )
        else:
            verify_evidence(
                extracted / "release-exit.json",
                source_sha=source_sha,
                version=version,
                phase_manifest=phase_manifest,
                repo_root=repo_root,
                authenticate=authenticate,
            )
        canonical = stage / expected_name
        write_reproducible_zip(
            extracted,
            canonical,
            source_date_epoch=source_date_epoch,
            mode_resolver=lambda _path: 0o644,
        )
        if not same_regular_file_bytes(canonical, archive):
            raise ValueError(
                "release-exit archive bytes are not the canonical reproducible ZIP"
            )
        verify_stable_regular_file_identity(identity, label="staged release evidence")
        durable_publish_directory_exclusive(extracted, output)
    return output / "release-exit.json"
