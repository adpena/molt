#!/usr/bin/env python3
"""Plan, assemble, and verify Molt's immutable release candidate set."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import tomllib
from typing import Any

from .build_bundle import build_bundle
from .release_model import (
    ROOT,
    file_record,
    load_config,
    normalized_version,
    release_targets,
    sha256_file,
    spdx_document,
    target_by_id,
    write_json,
)


CANDIDATE_SCHEMA = "molt.release-candidate.v1"
MANIFEST_SCHEMA = "molt.release-manifest.v2"


def _git(*args: str) -> str:
    result = subprocess.run(
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


def plan_release(requested_version: str, source_sha: str) -> dict[str, str]:
    version = normalized_version(requested_version or _project_version())
    project_version = _project_version()
    if version != project_version:
        raise ValueError(
            f"requested version {version} does not match {project_version}"
        )
    head = _git("rev-parse", "HEAD")
    if source_sha and source_sha != head:
        raise ValueError(f"workflow source {source_sha} does not match checkout {head}")
    expected_tag = f"v{version}"
    if (
        expected_tag
        not in _git("tag", "--points-at", "HEAD", "--list", expected_tag).splitlines()
    ):
        raise ValueError(f"release checkout is not the exact {expected_tag} tag")
    source_date_epoch = _git("show", "-s", "--format=%ct", "HEAD")
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
        "version": version,
        "source_sha": head,
        "source_date_epoch": source_date_epoch,
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
    payload = json.loads(path.read_text(encoding="utf-8"))
    if payload.get("schema") != CANDIDATE_SCHEMA:
        raise ValueError(f"invalid release candidate schema: {path}")
    return payload


def assemble_index(
    *,
    candidate_root: Path,
    wheel: Path,
    version: str,
    source_sha: str,
    source_date_epoch: int,
    output: Path,
) -> dict[str, object]:
    candidate_paths = sorted(candidate_root.rglob("candidate.json"))
    candidates_by_id: dict[str, tuple[Path, dict[str, Any]]] = {}
    for path in candidate_paths:
        candidate = _load_candidate(path)
        target_id = str(candidate["target"]["id"])
        if target_id in candidates_by_id:
            raise ValueError(f"duplicate release candidate: {target_id}")
        candidates_by_id[target_id] = (path.parent, candidate)
    candidates = [candidate for _, candidate in candidates_by_id.values()]
    expected_ids = {target.id for target in release_targets()}
    actual_ids = {str(candidate["target"]["id"]) for candidate in candidates}
    if actual_ids != expected_ids or len(candidates) != len(expected_ids):
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
    output.mkdir(parents=True, exist_ok=False)
    published: list[dict[str, object]] = [wheel_record]
    shutil.copyfile(wheel, output / wheel.name)
    seen_names = {wheel.name}
    for candidate in candidates:
        if candidate["version"] != version or candidate["source_sha"] != source_sha:
            raise ValueError(
                "release candidate source identity does not match release plan"
            )
        if candidate["source_date_epoch"] != source_date_epoch:
            raise ValueError("release candidate epoch does not match release plan")
        if candidate["wheel"] != wheel_record:
            raise ValueError("release candidates do not share the one canonical wheel")
        target = target_by_id(str(candidate["target"]["id"]))
        if (
            candidate["target"]["platform"] != target.platform
            or candidate["target"]["arch"] != target.arch
        ):
            raise ValueError(f"release candidate target metadata drifted: {target.id}")
        candidate_dir = candidates_by_id[target.id][0]
        reproducibility = candidate.get("reproducibility", {})
        if reproducibility != {
            "worker_sha256": reproducibility.get("worker_sha256"),
            "independent_worker_builds": 2,
            "independent_bundle_assemblies": 2,
            "matched": True,
        } or not isinstance(reproducibility.get("worker_sha256"), str):
            raise ValueError(f"{target.id}: reproducibility proof is incomplete")
        consumer_path = candidate_dir / "consumer-verification.json"
        if not consumer_path.is_file():
            raise ValueError(f"{target.id}: clean-consumer proof is missing")
        consumer = json.loads(consumer_path.read_text(encoding="utf-8"))
        if (
            consumer.get("schema") != "molt.release-consumer-proof.v1"
            or consumer.get("target") != candidate["target"]
            or consumer.get("source_sha") != source_sha
            or consumer.get("selected") != 1
            or consumer.get("executed") != 1
            or consumer.get("passed") != 1
            or consumer.get("failed") != 0
            or consumer.get("errors") != 0
            or consumer.get("uninstall_verified") is not True
        ):
            raise ValueError(f"{target.id}: clean-consumer proof is invalid")
        artifacts = candidate.get("artifacts", [])
        if not isinstance(artifacts, list) or len(artifacts) != 2:
            raise ValueError(f"{target.id}: expected exactly two release artifacts")
        for record in artifacts:
            filename = str(record["filename"])
            if filename in seen_names:
                raise ValueError(f"duplicate release artifact filename: {filename}")
            source = candidate_dir / filename
            actual = file_record(source, kind=str(record["kind"]))
            if any(actual[key] != record[key] for key in actual):
                raise ValueError(f"release candidate digest drift: {source}")
            shutil.copyfile(source, output / filename)
            seen_names.add(filename)
            published.append(record)

    published.sort(key=lambda item: str(item["filename"]))
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
        "artifacts": published,
        "attestation": {
            "provenance": "SLSA v1 signed by GitHub artifact attestations",
            "sbom": "SPDX 2.3 signed by GitHub artifact attestations",
            "signature": "Sigstore keyless OIDC certificate",
        },
    }
    write_json(output / "release_manifest.json", manifest)
    write_json(
        output / "release.spdx.json",
        spdx_document(
            version=version,
            source_sha=source_sha,
            source_date_epoch=source_date_epoch,
            subjects=published,
            wheel=wheel,
        ),
    )
    checksum_lines = [
        f"{record['sha256']}  {record['filename']}" for record in published
    ]
    (output / "SHA256SUMS").write_text(
        "\n".join(checksum_lines) + "\n", encoding="utf-8", newline="\n"
    )
    (output / "RELEASE_NOTES.md").write_text(
        f"Molt {version}\n\nSource: `{source_sha}`\n\n"
        "All artifacts passed independent reproducibility and clean-consumer "
        "verification on their target platform. Verify `SHA256SUMS` and the "
        "published GitHub Sigstore attestations before installation.\n",
        encoding="utf-8",
        newline="\n",
    )
    return manifest


def verify_promotion(local: Path, remote: Path) -> None:
    local_files = {path.name: path for path in local.iterdir() if path.is_file()}
    remote_files = {path.name: path for path in remote.iterdir() if path.is_file()}
    if set(local_files) != set(remote_files):
        raise ValueError(
            f"release asset set mismatch: local={sorted(local_files)}, "
            f"remote={sorted(remote_files)}"
        )
    for name in sorted(local_files):
        local_digest = sha256_file(local_files[name])
        remote_digest = sha256_file(remote_files[name])
        if local_digest != remote_digest:
            raise ValueError(f"published release asset digest mismatch: {name}")


def main() -> None:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)

    plan = subparsers.add_parser("plan")
    plan.add_argument("--requested-version", default="")
    plan.add_argument("--source-sha", default="")
    plan.add_argument("--github-output", type=Path)

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

    verify = subparsers.add_parser("verify-promotion")
    verify.add_argument("--local", type=Path, required=True)
    verify.add_argument("--remote", type=Path, required=True)

    subparsers.add_parser("validate")
    args = parser.parse_args()
    if args.command == "plan":
        outputs = plan_release(args.requested_version, args.source_sha)
        if args.github_output:
            _write_github_outputs(args.github_output, outputs)
        print(json.dumps(outputs, sort_keys=True))
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
        )
        print(json.dumps(manifest, sort_keys=True))
    elif args.command == "verify-promotion":
        verify_promotion(args.local, args.remote)
    else:
        release_targets()
        load_config()
        print("release supply-chain authority: OK")


if __name__ == "__main__":
    main()
