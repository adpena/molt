#!/usr/bin/env python3
"""Plan, assemble, and verify Molt's immutable release candidate set."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import shutil
import tempfile
import tomllib
from typing import Any

from molt.exact_json import read_exact
from molt.file_publication import durable_publish_directory_exclusive
from molt.toolchain_identity import snapshot_stable_regular_file
from tools.command_execution import CommandExecutor
from tools.git_identity import clean_checkout_status_arguments, require_git_object_id

from .build_bundle import build_bundle
from . import release_evidence
from .release_remote import (
    download_evidence,
    download_release,
    promote_release,
    require_draft,
    require_evidence_asset_id,
    stage_release,
    verify_remote_tag,
)
from .release_model import (
    ROOT,
    ATTESTATION_POLICY,
    FILE_FIELDS,
    MANIFEST_SCHEMA,
    PHASE_EXIT_KIND,
    PHASE_ATTESTATION_KIND,
    SPDX_PREDICATE_TYPE,
    file_record,
    load_config,
    normalized_version,
    release_targets,
    release_exit_archive_filename,
    phase_exit_filename,
    phase_exit_attestation_filename,
    stable_release,
    release_subjects,
    sha256_file,
    spdx_document,
    target_by_id,
    write_json,
    validate_artifact_record,
    validate_file_record,
    validate_release_manifest,
)

_COMMANDS = CommandExecutor.for_file(__file__)


CANDIDATE_SCHEMA = "molt.release-candidate.v1"


def _git(*args: str) -> str:
    result = _COMMANDS.run(
        ["git", *args],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    return result.stdout.strip()


def _project_version() -> str:
    with (ROOT / "pyproject.toml").open("rb") as handle:
        return normalized_version(str(tomllib.load(handle)["project"]["version"]))


def _write_github_outputs(path: Path, outputs: dict[str, str]) -> None:
    with path.open("a", encoding="utf-8", newline="\n") as handle:
        for name, value in outputs.items():
            if "\n" in value or "\r" in value:
                raise ValueError(f"GitHub output {name!r} contains a newline")
            handle.write(f"{name}={value}\n")


def resolve_source(requested_version: str, source_sha: str) -> dict[str, str]:
    """Resolve an exact tagged source for evidence retrieval, not admission."""
    require_git_object_id(source_sha, label="workflow release source")
    version = normalized_version(requested_version or _project_version())
    project_version = _project_version()
    if version != project_version:
        raise ValueError(
            f"requested version {version} does not match {project_version}"
        )
    head = _git("rev-parse", "HEAD")
    if source_sha != head:
        raise ValueError(f"workflow source {source_sha} does not match checkout {head}")
    expected_tag = f"v{version}"
    if (
        expected_tag
        not in _git("tag", "--points-at", "HEAD", "--list", expected_tag).splitlines()
    ):
        raise ValueError(f"release checkout is not the exact {expected_tag} tag")
    source_date_epoch = _git("show", "-s", "--format=%ct", "HEAD")
    if _git(*clean_checkout_status_arguments()):
        raise ValueError("release checkout must be clean")
    _git("merge-base", "--is-ancestor", head, "origin/main")
    return {
        "version": version,
        "source_sha": head,
        "source_date_epoch": source_date_epoch,
        "release_exit_archive": release_exit_archive_filename(head),
        "phase_exit_manifest": phase_exit_filename(head)
        if stable_release(version)
        else "",
        "phase_exit_attestation": phase_exit_attestation_filename(head)
        if stable_release(version)
        else "",
    }


def plan_release(
    requested_version: str,
    source_sha: str,
    *,
    release_exit_archive: Path,
) -> dict[str, str]:
    source = resolve_source(requested_version, source_sha)
    identity = file_record(release_exit_archive, kind="release-exit-evidence")
    with tempfile.TemporaryDirectory(prefix=".release-plan-") as temporary:
        manifest = release_evidence.extract_release_exit(
            archive=release_exit_archive,
            source_sha=source["source_sha"],
            source_date_epoch=int(source["source_date_epoch"]),
            output=Path(temporary) / "bundle",
            repo_root=ROOT,
        )
        release_evidence.verify_e3_provenance(manifest, source_sha=source["source_sha"])
    if file_record(release_exit_archive, kind="release-exit-evidence") != identity:
        raise ValueError("release-exit archive changed during planning")
    matrix = {
        "include": [
            {
                "id": target.id,
                "runner": target.runner,
                "platform": target.platform,
                "arch": target.arch,
                "archive": target.archive,
            }
            for target in release_targets()
        ]
    }
    return {
        **source,
        "release_exit_sha256": str(identity["sha256"]),
        "matrix": json.dumps(matrix, separators=(",", ":"), sort_keys=True),
    }


def verify_reproducible(
    primary: Path, secondary: Path, output: Path
) -> dict[str, object]:
    primary_record = file_record(primary, kind="wheel")
    secondary_record = file_record(secondary, kind="wheel")
    if primary_record["sha256"] != secondary_record["sha256"]:
        raise ValueError(
            "release wheel is not reproducible: "
            f"{primary_record['sha256']} != {secondary_record['sha256']}"
        )
    output.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(primary, output)
    return file_record(output, kind="wheel")


def select_one(root: Path, pattern: str) -> Path:
    matches = sorted(path for path in root.glob(pattern) if path.is_file())
    if len(matches) != 1:
        raise ValueError(
            f"expected exactly one release input matching {pattern!r} under {root}, "
            f"found {[path.name for path in matches]}"
        )
    return matches[0]


def assemble_candidate(
    *,
    target_id: str,
    version: str,
    source_sha: str,
    source_date_epoch: int,
    wheel: Path,
    primary_worker: Path,
    secondary_worker: Path,
    output: Path,
) -> dict[str, object]:
    target = target_by_id(target_id)
    worker_primary = file_record(primary_worker, kind="worker-repro-primary")
    worker_secondary = file_record(secondary_worker, kind="worker-repro-secondary")
    if worker_primary["sha256"] != worker_secondary["sha256"]:
        raise ValueError(
            f"{target_id}: molt-worker is not reproducible: "
            f"{worker_primary['sha256']} != {worker_secondary['sha256']}"
        )
    output.mkdir(parents=True, exist_ok=False)
    artifacts: list[dict[str, object]] = []
    with tempfile.TemporaryDirectory() as temporary:
        repeat_root = Path(temporary)
        for kind in ("molt", "molt-worker"):
            filename = target.artifact_filename(kind, version)
            primary_bundle = output / filename
            repeat_bundle = repeat_root / filename
            for worker, destination in (
                (primary_worker, primary_bundle),
                (secondary_worker, repeat_bundle),
            ):
                build_bundle(
                    version=version,
                    platform=target.platform,
                    wheel=wheel if kind == "molt" else None,
                    worker=worker,
                    kind=kind,
                    output=destination,
                    source_date_epoch=source_date_epoch,
                )
            if sha256_file(primary_bundle) != sha256_file(repeat_bundle):
                raise ValueError(f"{target_id}: {kind} bundle is not reproducible")
            record = file_record(primary_bundle, kind=kind)
            record.update(
                {
                    "name": kind,
                    "version": version,
                    "platform": target.platform,
                    "arch": target.arch,
                    "libc": "gnu" if target.platform == "linux" else None,
                }
            )
            artifacts.append(record)

    wheel_record = file_record(wheel, kind="wheel")
    wheel_record.update(
        {
            "name": "molt-wheel",
            "version": version,
            "platform": "any",
            "arch": "any",
            "libc": None,
        }
    )
    payload: dict[str, object] = {
        "schema": CANDIDATE_SCHEMA,
        "version": version,
        "source_sha": source_sha,
        "source_date_epoch": source_date_epoch,
        "target": {
            "id": target.id,
            "platform": target.platform,
            "arch": target.arch,
            "runner": target.runner,
        },
        "wheel": wheel_record,
        "artifacts": sorted(artifacts, key=lambda item: str(item["filename"])),
        "reproducibility": {
            "worker_sha256": worker_primary["sha256"],
            "independent_worker_builds": 2,
            "independent_bundle_assemblies": 2,
            "matched": True,
        },
    }
    write_json(output / "candidate.json", payload)
    return payload


def _load_candidate(path: Path) -> dict[str, Any]:
    """Admit the complete current candidate shape before any nested access."""
    payload = read_exact(path, max_bytes=1024 * 1024, label="release candidate")
    if (
        not isinstance(payload, dict)
        or set(payload)
        != {
            "schema",
            "version",
            "source_sha",
            "source_date_epoch",
            "target",
            "wheel",
            "artifacts",
            "reproducibility",
        }
        or payload.get("schema") != CANDIDATE_SCHEMA
    ):
        raise ValueError(f"invalid release candidate schema: {path}")
    version = payload["version"]
    if not isinstance(version, str) or normalized_version(version) != version:
        raise ValueError(f"release candidate version is invalid: {path}")
    require_git_object_id(payload["source_sha"], label="release candidate source")
    if (
        type(payload["source_date_epoch"]) is not int
        or payload["source_date_epoch"] <= 0
    ):
        raise ValueError(f"release candidate epoch must be a positive integer: {path}")
    target = payload["target"]
    if (
        not isinstance(target, dict)
        or set(target) != {"id", "platform", "arch", "runner"}
        or not all(isinstance(value, str) and value for value in target.values())
    ):
        raise ValueError(f"release candidate target metadata is invalid: {path}")
    artifacts = payload["artifacts"]
    if not isinstance(artifacts, list) or len(artifacts) != 2:
        raise ValueError(f"{target['id']}: expected exactly two release artifacts")
    for record in [payload["wheel"], *artifacts]:
        if (
            not isinstance(record, dict)
            or not all(
                isinstance(record.get(key), str)
                for key in ("name", "version", "platform", "arch")
            )
            or (record.get("libc") is not None and not isinstance(record["libc"], str))
        ):
            raise ValueError(
                f"{target['id']}: release candidate artifact metadata is invalid"
            )
        validate_artifact_record(record, version=version)
    proof = payload["reproducibility"]
    if (
        not isinstance(proof, dict)
        or set(proof)
        != {
            "worker_sha256",
            "independent_worker_builds",
            "independent_bundle_assemblies",
            "matched",
        }
        or any(
            type(proof.get(key)) is not int or proof[key] != 2
            for key in ("independent_worker_builds", "independent_bundle_assemblies")
        )
        or proof.get("matched") is not True
        or not isinstance(proof.get("worker_sha256"), str)
        or re.fullmatch(r"[0-9a-f]{64}", proof["worker_sha256"]) is None
    ):
        raise ValueError(f"{target['id']}: reproducibility proof is incomplete")
    return payload


def _admit_candidate(
    candidate: dict[str, Any],
    candidate_dir: Path,
    *,
    version: str,
    source_sha: str,
    source_date_epoch: int,
    wheel_record: dict[str, object],
) -> list[dict[str, Any]]:
    """Bind a typed candidate and its clean-consumer proof to one release cell."""
    if candidate["version"] != version or candidate["source_sha"] != source_sha:
        raise ValueError(
            "release candidate source identity does not match release plan"
        )
    if candidate["source_date_epoch"] != source_date_epoch:
        raise ValueError("release candidate epoch does not match release plan")
    if candidate["wheel"] != wheel_record:
        raise ValueError("release candidates do not share the one canonical wheel")
    target = target_by_id(candidate["target"]["id"])
    if candidate["target"] != {
        "id": target.id,
        "platform": target.platform,
        "arch": target.arch,
        "runner": target.runner,
    }:
        raise ValueError(f"release candidate target metadata drifted: {target.id}")
    consumer_path = candidate_dir / "consumer-verification.json"
    if not consumer_path.is_file():
        raise ValueError(f"{target.id}: clean-consumer proof is missing")
    consumer = read_exact(
        consumer_path, max_bytes=1024 * 1024, label="release consumer proof"
    )
    if (
        not isinstance(consumer, dict)
        or consumer.get("schema") != "molt.release-consumer-proof.v1"
        or consumer.get("target") != candidate["target"]
        or consumer.get("source_sha") != source_sha
        or any(
            type(consumer.get(key)) is not int or consumer[key] != expected
            for key, expected in {
                "selected": 1,
                "executed": 1,
                "passed": 1,
                "failed": 0,
                "errors": 0,
            }.items()
        )
        or consumer.get("uninstall_verified") is not True
    ):
        raise ValueError(f"{target.id}: clean-consumer proof is invalid")
    artifacts = candidate["artifacts"]
    if {record["name"] for record in artifacts} != {"molt", "molt-worker"}:
        raise ValueError(
            f"{target.id}: expected one Molt and one worker release artifact"
        )
    for record in artifacts:
        if (record["platform"], record["arch"]) != (target.platform, target.arch):
            raise ValueError(f"release candidate artifact target differs: {target.id}")
    return artifacts


def _copy_verified_release_file(
    source: Path, destination: Path, expected: dict[str, Any]
) -> None:
    validate_file_record(expected)
    snapshot = snapshot_stable_regular_file(source, destination, label="release asset")
    if (snapshot.snapshot.sha256, snapshot.snapshot.size) != (
        expected["sha256"],
        expected["size"],
    ):
        snapshot.discard()
        raise ValueError(f"release candidate digest drift: {source}")


def assemble_index(
    *,
    candidate_root: Path,
    wheel: Path,
    version: str,
    source_sha: str,
    source_date_epoch: int,
    output: Path,
    release_exit_archive: Path,
    release_exit_sha256: str,
    phase_exit_manifest: Path | None = None,
) -> dict[str, object]:
    if type(source_date_epoch) is not int or source_date_epoch <= 0:
        raise ValueError("release epoch must be a positive integer")
    source = resolve_source(version, source_sha)
    if int(source["source_date_epoch"]) != source_date_epoch:
        raise ValueError("release epoch differs from tagged source")
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(
        prefix=".release-index-", dir=output.parent
    ) as temporary:
        stage = Path(temporary) / "publish"
        stage.mkdir()
        manifest = _assemble_index_stage(
            candidate_root=candidate_root,
            wheel=wheel,
            version=version,
            source_sha=source_sha,
            source_date_epoch=source_date_epoch,
            output=stage,
            release_exit_archive=release_exit_archive,
            release_exit_sha256=release_exit_sha256,
            phase_exit_manifest=phase_exit_manifest,
        )
        _release_directory_files(stage, require_sigstore_sidecars=False)
        durable_publish_directory_exclusive(stage, output)
    return manifest


def _assemble_index_stage(
    *,
    candidate_root: Path,
    wheel: Path,
    version: str,
    source_sha: str,
    source_date_epoch: int,
    output: Path,
    release_exit_archive: Path,
    release_exit_sha256: str,
    phase_exit_manifest: Path | None,
) -> dict[str, object]:
    evidence_record, phase_record = _stage_release_evidence(
        version=version,
        source_sha=source_sha,
        source_date_epoch=source_date_epoch,
        output=output,
        release_exit_archive=release_exit_archive,
        release_exit_sha256=release_exit_sha256,
        phase_exit_manifest=phase_exit_manifest,
    )
    published = _stage_candidate_assets(
        candidate_root=candidate_root,
        wheel=wheel,
        version=version,
        source_sha=source_sha,
        source_date_epoch=source_date_epoch,
        output=output,
    )
    return _compose_index_metadata(
        version=version,
        source_sha=source_sha,
        source_date_epoch=source_date_epoch,
        output=output,
        wheel=output / wheel.name,
        published=published,
        evidence_record=evidence_record,
        phase_record=phase_record,
    )


def _stage_release_evidence(
    *,
    version: str,
    source_sha: str,
    source_date_epoch: int,
    output: Path,
    release_exit_archive: Path,
    release_exit_sha256: str,
    phase_exit_manifest: Path | None,
) -> tuple[dict[str, object], dict[str, dict[str, object]] | None]:
    """Snapshot and authenticate the planned source-named semantic evidence."""
    evidence_record = file_record(release_exit_archive, kind="release-exit-evidence")
    if (
        evidence_record["filename"] != release_exit_archive_filename(source_sha)
        or evidence_record["sha256"] != release_exit_sha256
    ):
        raise ValueError("release-exit archive differs from admitted plan")
    _copy_verified_release_file(
        release_exit_archive, output / release_exit_archive.name, evidence_record
    )
    phase_record = None
    if phase_exit_manifest is not None:
        if phase_exit_manifest.name != phase_exit_filename(source_sha):
            raise ValueError("H0 phase manifest must have its source-named filename")
        phase_record = {}
        for key, kind, path in (
            ("manifest", PHASE_EXIT_KIND, phase_exit_manifest),
            (
                "attestation",
                PHASE_ATTESTATION_KIND,
                phase_exit_manifest.parent
                / phase_exit_attestation_filename(source_sha),
            ),
        ):
            record = file_record(path, kind=kind)
            _copy_verified_release_file(path, output / path.name, record)
            phase_record[key] = record
    # Admission uses the exact archived/staged bytes, including H0 when required.
    with tempfile.TemporaryDirectory(
        prefix=".release-admission-", dir=output.parent
    ) as temporary:
        release_evidence.extract_release_exit(
            archive=output / str(evidence_record["filename"]),
            source_sha=source_sha,
            source_date_epoch=source_date_epoch,
            output=Path(temporary) / "bundle",
            repo_root=ROOT,
            version=version,
            phase_manifest=output / phase_exit_filename(source_sha)
            if phase_record
            else None,
            authenticate=True,
        )
    return evidence_record, phase_record


def _stage_candidate_assets(
    *,
    candidate_root: Path,
    wheel: Path,
    version: str,
    source_sha: str,
    source_date_epoch: int,
    output: Path,
) -> list[dict[str, object]]:
    """Require the exact admitted target matrix and stage only verified bytes."""
    candidate_paths = sorted(candidate_root.rglob("candidate.json"))
    candidates_by_id: dict[str, tuple[Path, dict[str, Any]]] = {}
    for path in candidate_paths:
        candidate = _load_candidate(path)
        target_id = candidate["target"]["id"]
        if target_id in candidates_by_id:
            raise ValueError(f"duplicate release candidate: {target_id}")
        candidates_by_id[target_id] = (path.parent, candidate)
    expected_ids = {target.id for target in release_targets()}
    actual_ids = set(candidates_by_id)
    if actual_ids != expected_ids:
        raise ValueError(
            f"release candidate matrix mismatch: expected {sorted(expected_ids)}, "
            f"got {sorted(actual_ids)}"
        )
    wheel_record = file_record(wheel, kind="wheel")
    wheel_record.update(
        {
            "name": "molt-wheel",
            "version": version,
            "platform": "any",
            "arch": "any",
            "libc": None,
        }
    )
    published: list[dict[str, object]] = [wheel_record]
    validate_artifact_record(wheel_record, version=version)
    _copy_verified_release_file(wheel, output / wheel.name, wheel_record)
    seen_names = {wheel.name}
    for candidate_dir, candidate in candidates_by_id.values():
        artifacts = _admit_candidate(
            candidate,
            candidate_dir,
            version=version,
            source_sha=source_sha,
            source_date_epoch=source_date_epoch,
            wheel_record=wheel_record,
        )
        for record in artifacts:
            filename = record["filename"]
            if filename in seen_names:
                raise ValueError(f"duplicate release artifact filename: {filename}")
            source = candidate_dir / filename
            _copy_verified_release_file(source, output / filename, record)
            seen_names.add(filename)
            published.append(record)

    published.sort(key=lambda item: str(item["filename"]))
    return published


def _compose_index_metadata(
    *,
    version: str,
    source_sha: str,
    source_date_epoch: int,
    output: Path,
    wheel: Path,
    published: list[dict[str, object]],
    evidence_record: dict[str, object],
    phase_record: dict[str, dict[str, object]] | None,
) -> dict[str, object]:
    """Project the index, checksums and SBOM from the staged release subjects."""
    config = load_config()
    owner = str(config["repository"]["owner"])
    repository = str(config["repository"]["name"])
    for record in published:
        record["url"] = (
            f"https://github.com/{owner}/{repository}/releases/download/"
            f"v{version}/{record['filename']}"
        )
    manifest: dict[str, object] = {
        "schema": MANIFEST_SCHEMA,
        "version": version,
        "source_sha": source_sha,
        "source_date_epoch": source_date_epoch,
        "repo": f"{owner}/{repository}",
        "evidence_archive": evidence_record,
        "phase_exit": phase_record,
        "artifacts": published,
        "attestation": dict(ATTESTATION_POLICY),
    }
    validate_release_manifest(manifest)
    subjects = release_subjects(manifest)
    write_json(output / "release_manifest.json", manifest)
    write_json(
        output / "release.spdx.json",
        spdx_document(
            version=version,
            source_sha=source_sha,
            source_date_epoch=source_date_epoch,
            subjects=subjects,
            wheel=wheel,
        ),
    )
    checksum_lines = [
        f"{record['sha256']}  {record['filename']}" for record in subjects
    ]
    (output / "SHA256SUMS").write_text(
        "\n".join(checksum_lines) + "\n", encoding="utf-8", newline="\n"
    )
    (output / "RELEASE_NOTES.md").write_text(
        f"Molt {version}\n\nSource: `{source_sha}`\n\n"
        "All artifacts passed independent reproducibility and clean-consumer "
        "verification on their target platform; the source-bound E1-E4 evidence "
        "and any required stable H0 phase exit passed their owning gates. "
        "These gates do not expand the advertised verified subset. Verify `SHA256SUMS` and the "
        "published GitHub Sigstore attestations before installation.\n",
        encoding="utf-8",
        newline="\n",
    )
    return manifest


_RELEASE_METADATA = frozenset(
    {"release_manifest.json", "release.spdx.json", "SHA256SUMS", "RELEASE_NOTES.md"}
)
_RELEASE_SIDECARS = frozenset(
    {"release.provenance.sigstore.json", "release.sbom.sigstore.json"}
)


def _release_directory_files(
    root: Path, *, require_sigstore_sidecars: bool
) -> dict[str, Path]:
    files = {path.name: path for path in root.iterdir()}
    if any(
        not path.is_file() or path.is_symlink() or path.is_junction()
        for path in files.values()
    ):
        raise ValueError("release asset set contains non-regular files")
    manifest_path = files.get("release_manifest.json")
    if manifest_path is None:
        raise ValueError("release asset set has no manifest")
    manifest = validate_release_manifest(
        read_exact(manifest_path, max_bytes=4 * 1024 * 1024, label="release manifest")
    )
    subjects = release_subjects(manifest)
    expected = {record["filename"] for record in subjects} | _RELEASE_METADATA
    if require_sigstore_sidecars:
        expected |= _RELEASE_SIDECARS
    if set(files) != expected:
        raise ValueError(
            f"release asset set mismatch: expected={sorted(expected)}, got={sorted(files)}"
        )
    checksums = "".join(
        f"{record['sha256']}  {record['filename']}\n" for record in subjects
    )
    if files["SHA256SUMS"].read_bytes() != checksums.encode("utf-8"):
        raise ValueError("release SHA256SUMS differs from its manifest")
    sbom = read_exact(
        files["release.spdx.json"], max_bytes=16 * 1024 * 1024, label="release SBOM"
    )
    if not isinstance(sbom, dict):
        raise ValueError("release SBOM must be an object")
    expected_sbom_files = {
        f"./{record['filename']}": record["sha256"] for record in subjects
    }
    actual_files = sbom.get("files", [])
    if (
        not isinstance(actual_files, list)
        or len(actual_files) != len(subjects)
        or not all(
            isinstance(record, dict) and isinstance(record.get("fileName"), str)
            for record in actual_files
        )
        or {record.get("fileName"): record.get("checksums") for record in actual_files}
        != {
            name: [{"algorithm": "SHA256", "checksumValue": digest}]
            for name, digest in expected_sbom_files.items()
        }
    ):
        raise ValueError("release SBOM subjects differ from its manifest")
    for record in subjects:
        actual = file_record(files[record["filename"]], kind=record["kind"])
        if actual != {key: record[key] for key in FILE_FIELDS}:
            raise ValueError(f"release asset digest mismatch: {record['filename']}")
    return files


def verify_signed_release(root: Path, *, source_sha: str) -> dict[str, Path]:
    files = _release_directory_files(root, require_sigstore_sidecars=True)
    manifest = validate_release_manifest(
        read_exact(
            files["release_manifest.json"],
            max_bytes=4 * 1024 * 1024,
            label="release manifest",
        )
    )
    source = resolve_source(manifest["version"], source_sha)
    verify_remote_tag(manifest["version"], source_sha)
    if manifest["source_sha"] != source_sha or manifest["source_date_epoch"] != int(
        source["source_date_epoch"]
    ):
        raise ValueError("signed release identity differs from tagged source")
    for name in sorted(set(files) - _RELEASE_SIDECARS):
        release_evidence.verify_provenance(
            files[name],
            source_sha=source_sha,
            workflow="release.yml",
            bundle=files["release.provenance.sigstore.json"],
        )
    for record in release_subjects(manifest):
        release_evidence.verify_provenance(
            files[record["filename"]],
            source_sha=source_sha,
            workflow="release.yml",
            bundle=files["release.sbom.sigstore.json"],
            predicate_type=SPDX_PREDICATE_TYPE,
        )
    return files


def verify_promotion(local: Path, remote: Path, *, source_sha: str) -> None:
    local_files = verify_signed_release(local, source_sha=source_sha)
    _verify_release_copy(local_files, remote)


def _verify_release_copy(local_files: dict[str, Path], remote: Path) -> None:
    """Compare remote bytes with an already authenticated signed local snapshot."""
    remote_files = _release_directory_files(remote, require_sigstore_sidecars=True)
    if set(local_files) != set(remote_files):
        raise ValueError("release asset set mismatch between signed and staged assets")
    for name in sorted(local_files):
        local_digest = sha256_file(local_files[name])
        remote_digest = sha256_file(remote_files[name])
        if local_digest != remote_digest:
            raise ValueError(f"published release asset digest mismatch: {name}")


def main() -> None:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)

    for command in ("source", "plan"):
        plan = subparsers.add_parser(command)
        plan.add_argument("--requested-version", default="")
        plan.add_argument("--source-sha", required=True)
        plan.add_argument("--github-output", type=Path)
        if command == "plan":
            plan.add_argument("--release-exit-archive", type=Path, required=True)

    for command in ("archive-exit", "extract-exit"):
        evidence = subparsers.add_parser(command)
        evidence.add_argument(
            "--manifest" if command == "archive-exit" else "--archive",
            type=Path,
            required=True,
        )
        evidence.add_argument("--source-sha", required=True)
        evidence.add_argument("--source-date-epoch", type=int, required=True)
        evidence.add_argument("--output", type=Path, required=True)

    draft = subparsers.add_parser("require-draft")
    draft.add_argument("--version", required=True)
    draft.add_argument("--source-sha", required=True)
    draft.add_argument("--release-id", type=int)
    draft.add_argument("--evidence-asset-id", type=int)
    draft.add_argument("--evidence-only", action="store_true")
    draft.add_argument("--github-output", type=Path)

    for command in (
        "download-evidence",
        "stage-release",
        "download-release",
        "promote-release",
    ):
        remote = subparsers.add_parser(command)
        remote.add_argument("--version", required=True)
        remote.add_argument("--source-sha", required=True)
        remote.add_argument("--release-id", type=int, required=True)
        remote.add_argument("--evidence-asset-id", type=int, required=True)
        remote.add_argument(
            "--local"
            if command in {"stage-release", "promote-release"}
            else "--output",
            type=Path,
            required=True,
        )
        if command == "download-release":
            remote.add_argument("--published", action="store_true")

    wheel = subparsers.add_parser("verify-wheel")
    wheel.add_argument("--primary", type=Path, required=True)
    wheel.add_argument("--secondary", type=Path, required=True)
    wheel.add_argument("--output", type=Path, required=True)

    select = subparsers.add_parser("select-one")
    select.add_argument("--root", type=Path, required=True)
    select.add_argument("--pattern", required=True)

    candidate = subparsers.add_parser("candidate")
    candidate.add_argument("--target", required=True)
    candidate.add_argument("--version", required=True)
    candidate.add_argument("--source-sha", required=True)
    candidate.add_argument("--source-date-epoch", type=int, required=True)
    candidate.add_argument("--wheel", type=Path, required=True)
    candidate.add_argument("--primary-worker", type=Path, required=True)
    candidate.add_argument("--secondary-worker", type=Path, required=True)
    candidate.add_argument("--output", type=Path, required=True)

    index = subparsers.add_parser("index")
    index.add_argument("--candidate-root", type=Path, required=True)
    index.add_argument("--wheel", type=Path, required=True)
    index.add_argument("--version", required=True)
    index.add_argument("--source-sha", required=True)
    index.add_argument("--source-date-epoch", type=int, required=True)
    index.add_argument("--output", type=Path, required=True)
    index.add_argument("--release-exit-archive", type=Path, required=True)
    index.add_argument("--release-exit-sha256", required=True)
    index.add_argument("--phase-exit-manifest", type=Path)

    verify = subparsers.add_parser("verify-promotion")
    verify.add_argument("--local", type=Path, required=True)
    verify.add_argument("--remote", type=Path, required=True)
    verify.add_argument("--source-sha", required=True)
    signed = subparsers.add_parser("verify-signed")
    signed.add_argument("--root", type=Path, required=True)
    signed.add_argument("--source-sha", required=True)

    subparsers.add_parser("validate")
    args = parser.parse_args()
    if args.command in {"source", "plan"}:
        outputs = (
            resolve_source(args.requested_version, args.source_sha)
            if args.command == "source"
            else plan_release(
                args.requested_version,
                args.source_sha,
                release_exit_archive=args.release_exit_archive,
            )
        )
        if args.github_output:
            _write_github_outputs(args.github_output, outputs)
        print(json.dumps(outputs, sort_keys=True))
    elif args.command in {"archive-exit", "extract-exit"}:
        require_git_object_id(args.source_sha, label="release evidence source")
        if _git("rev-parse", "HEAD") != args.source_sha:
            raise ValueError("release evidence source differs from checkout")
        commit_epoch = int(_git("show", "-s", "--format=%ct", "HEAD"))
        if args.source_date_epoch <= 0 or args.source_date_epoch != commit_epoch:
            raise ValueError("release evidence epoch differs from source commit")
        common = dict(
            source_sha=args.source_sha,
            source_date_epoch=args.source_date_epoch,
            output=args.output,
            repo_root=ROOT,
        )
        if args.command == "archive-exit":
            print(
                json.dumps(
                    release_evidence.archive_release_exit(
                        manifest=args.manifest, **common
                    ),
                    sort_keys=True,
                )
            )
        else:
            print(release_evidence.extract_release_exit(archive=args.archive, **common))
    elif args.command == "require-draft":
        release_id = require_draft(
            args.version,
            args.source_sha,
            release_id=args.release_id,
            evidence_only=args.evidence_only,
            evidence_asset_id=args.evidence_asset_id,
        )
        outputs = {"release_id": str(release_id)}
        if args.evidence_only:
            outputs["evidence_asset_id"] = str(
                require_evidence_asset_id(
                    args.version,
                    args.source_sha,
                    release_id=release_id,
                    evidence_asset_id=args.evidence_asset_id,
                )
            )
        if args.github_output:
            _write_github_outputs(args.github_output, outputs)
        print(release_id)
    elif args.command == "download-evidence":
        print(
            download_evidence(
                args.version,
                args.source_sha,
                release_id=args.release_id,
                output=args.output,
                evidence_asset_id=args.evidence_asset_id,
            )
        )
    elif args.command == "download-release":
        download_release(
            args.version,
            args.source_sha,
            release_id=args.release_id,
            output=args.output,
            published=args.published,
            evidence_asset_id=args.evidence_asset_id,
        )
        print(args.output)
    elif args.command == "stage-release":
        stage_release(
            args.version,
            args.source_sha,
            release_id=args.release_id,
            local=args.local,
            verify_local=verify_signed_release,
            evidence_asset_id=args.evidence_asset_id,
        )
        print(args.release_id)
    elif args.command == "promote-release":
        promote_release(
            args.version,
            args.source_sha,
            release_id=args.release_id,
            local=args.local,
            verify_local=verify_signed_release,
            verify_copy=_verify_release_copy,
            evidence_asset_id=args.evidence_asset_id,
        )
        print(args.release_id)
    elif args.command == "verify-wheel":
        print(
            json.dumps(verify_reproducible(args.primary, args.secondary, args.output))
        )
    elif args.command == "select-one":
        print(select_one(args.root, args.pattern))
    elif args.command == "candidate":
        payload = assemble_candidate(
            target_id=args.target,
            version=normalized_version(args.version),
            source_sha=args.source_sha,
            source_date_epoch=args.source_date_epoch,
            wheel=args.wheel,
            primary_worker=args.primary_worker,
            secondary_worker=args.secondary_worker,
            output=args.output,
        )
        print(json.dumps(payload, sort_keys=True))
    elif args.command == "index":
        manifest = assemble_index(
            candidate_root=args.candidate_root,
            wheel=args.wheel,
            version=normalized_version(args.version),
            source_sha=args.source_sha,
            source_date_epoch=args.source_date_epoch,
            output=args.output,
            release_exit_archive=args.release_exit_archive,
            release_exit_sha256=args.release_exit_sha256,
            phase_exit_manifest=args.phase_exit_manifest,
        )
        print(json.dumps(manifest, sort_keys=True))
    elif args.command == "verify-promotion":
        verify_promotion(args.local, args.remote, source_sha=args.source_sha)
    elif args.command == "verify-signed":
        verify_signed_release(args.root, source_sha=args.source_sha)
    else:
        release_targets()
        load_config()
        print("release supply-chain authority: OK")


if __name__ == "__main__":
    main()
