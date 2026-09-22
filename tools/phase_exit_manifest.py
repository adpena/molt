#!/usr/bin/env python3
"""Assemble and verify century-plan phase-exit manifests.

docs/design/CENTURY_SYSTEMS_PLAN.md §5 fixes the phase predicate:

    phase_green := schema_valid
      and manifest.commit == release_commit
      and manifest.matrix_digest == generated_matrix.digest
      and every(required_requirement has exactly one current passing evidence row)
      and every(required_matrix_cell is covered by that row)
      and open_obligations == []
      and legacy_count == 0
      and signatures_and_hashes_verify

Missing, stale, duplicate, waived, unevaluated, or indirectly inferred evidence
evaluates false. This module implements exactly that predicate over a manifest
whose evidence rows are projected from the typed release-exit bundle
(tools/release_exit_gate.py), the exact verified-subset matrix
(tools/verified_subset.py matrix), the legacy inventory
(tools/legacy_inventory.py), and a Sigstore attestation bundle whose in-toto
subject binds the manifest bytes.

`assemble` never fabricates a fact: when an authority does not carry a required
field (for example a Pact acceptance receipt that records no command), the row
carries `null` and `verify` names the requirement and field that make the phase
false. Those nulls are the work list, not a defect of the validator.
"""

from __future__ import annotations

import argparse
import base64
import datetime as dt
import json
import sys
import tomllib
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from molt.exact_json import (
    ExactJsonError,
    canonical_json_sha256,
    loads_exact,
    write_exact,
)
from molt.toolchain_identity import stable_file_sha256
from tools import legacy_inventory
from tools import release_exit_gate as reg
from tools import verified_subset as vs
from tools.git_identity import is_git_object_id

ROOT = Path(__file__).resolve().parents[1]
REQUIREMENTS_PATH = ROOT / "config" / "phase_exit_requirements.toml"
REQUIREMENTS_SCHEMA = "molt.phase-exit-requirements.v1"
MANIFEST_SCHEMA = "molt.phase-exit-manifest.v1"
STATUS_PASS = "PASS"
STATUS_FAIL = "FAIL"
E3_ROLE_PREFIX = "e3_"
E3_WILDCARD_ROLE = "e3_*"
VERIFIED_SUBSET_CELL_PREFIX = "verified-subset:"
_MANIFEST_KEYS = frozenset(
    {
        "schema",
        "phase",
        "commit",
        "matrix_digest",
        "evidence",
        "open_obligations",
        "legacy_count",
        "signed_attestation",
    }
)
_EVIDENCE_KEYS = frozenset(
    {
        "requirement_id",
        "authority",
        "command",
        "artifact_sha256",
        "matrix_cells",
        "status",
        "toolchain_digest",
        "observed_at",
    }
)
_ATTESTATION_KEYS = frozenset({"kind", "path", "sha256", "subject_sha256"})
ATTESTATION_KIND = "sigstore-bundle"


@dataclass(frozen=True)
class Requirement:
    id: str
    authority: str
    evidence_role: str
    matrix_cells: tuple[str, ...]


@dataclass(frozen=True)
class Obligation:
    id: str
    description: str
    closed_by: str


@dataclass(frozen=True)
class Phase:
    id: str
    title: str
    matrix_authority: str
    requirements: tuple[Requirement, ...]
    obligations: tuple[Obligation, ...]


@dataclass(frozen=True)
class PhaseReport:
    phase: str | None
    commit: str | None
    green: bool
    problems: tuple[str, ...]


# --- requirements registry ---------------------------------------------------


def load_phases(path: Path = REQUIREMENTS_PATH) -> dict[str, Phase]:
    with path.open("rb") as handle:
        document = tomllib.load(handle)
    if document.get("schema") != REQUIREMENTS_SCHEMA:
        raise ValueError(f"{path}: unsupported phase-exit requirements schema")
    phases: dict[str, Phase] = {}
    for raw_phase in document.get("phase", []):
        if not isinstance(raw_phase, dict):
            raise ValueError(f"{path}: phase rows must be tables")
        phase_id = raw_phase.get("id")
        if not isinstance(phase_id, str) or phase_id in phases:
            raise ValueError(f"{path}: phase id must be a unique string")
        requirements: list[Requirement] = []
        seen_ids: set[str] = set()
        seen_roles: set[str] = set()
        for raw in raw_phase.get("requirement", []):
            if not isinstance(raw, dict) or set(raw) != {
                "id",
                "authority",
                "evidence_role",
                "matrix_cells",
            }:
                raise ValueError(f"{path}: {phase_id} requirement rows need exact keys")
            cells = raw["matrix_cells"]
            if (
                not isinstance(raw["id"], str)
                or not isinstance(raw["authority"], str)
                or not isinstance(raw["evidence_role"], str)
                or not isinstance(cells, list)
                or not cells
                or not all(isinstance(cell, str) and cell for cell in cells)
            ):
                raise ValueError(f"{path}: {phase_id} requirement fields are mistyped")
            if raw["id"] in seen_ids or raw["evidence_role"] in seen_roles:
                raise ValueError(
                    f"{path}: {phase_id} requirement ids and evidence roles must be unique"
                )
            seen_ids.add(raw["id"])
            seen_roles.add(raw["evidence_role"])
            requirements.append(
                Requirement(
                    raw["id"], raw["authority"], raw["evidence_role"], tuple(cells)
                )
            )
        obligations: list[Obligation] = []
        seen_obligations: set[str] = set()
        for raw in raw_phase.get("obligation", []):
            if not isinstance(raw, dict) or set(raw) != {
                "id",
                "description",
                "closed_by",
            }:
                raise ValueError(f"{path}: {phase_id} obligation rows need exact keys")
            if not all(isinstance(raw[key], str) and raw[key] for key in raw):
                raise ValueError(f"{path}: {phase_id} obligation fields are mistyped")
            if raw["id"] in seen_obligations:
                raise ValueError(f"{path}: {phase_id} obligation ids must be unique")
            if raw["closed_by"] not in seen_ids:
                raise ValueError(
                    f"{path}: {phase_id} obligation {raw['id']} closes on unknown "
                    f"requirement {raw['closed_by']}"
                )
            seen_obligations.add(raw["id"])
            obligations.append(
                Obligation(raw["id"], raw["description"], raw["closed_by"])
            )
        title = raw_phase.get("title")
        matrix_authority = raw_phase.get("matrix_authority")
        if not isinstance(title, str) or not isinstance(matrix_authority, str):
            raise ValueError(f"{path}: {phase_id} needs title and matrix_authority")
        if not requirements:
            raise ValueError(f"{path}: {phase_id} declares no requirements")
        phases[phase_id] = Phase(
            phase_id, title, matrix_authority, tuple(requirements), tuple(obligations)
        )
    if not phases:
        raise ValueError(f"{path}: no phases declared")
    return phases


# --- generated matrix ---------------------------------------------------------


def generated_matrix() -> dict[str, Any]:
    """The exact verified-subset proof matrix, as `tools/verified_subset.py matrix` emits it."""
    payload = vs.matrix_payload()
    include = payload.get("include")
    if not isinstance(include, list) or not include:
        raise ValueError("verified-subset matrix is empty")
    records: list[dict[str, Any]] = []
    for record in include:
        if not isinstance(record, Mapping):
            raise ValueError("verified-subset matrix records must be objects")
        records.append({str(key): value for key, value in sorted(record.items())})
    return {"include": records}


def generated_matrix_digest() -> str:
    return canonical_json_sha256(generated_matrix())


def expand_requirements(
    phase: Phase, matrix: Mapping[str, Any]
) -> tuple[Requirement, ...]:
    """Expand `e3_*` rows into one requirement per exact matrix coordinate."""
    expanded: list[Requirement] = []
    for requirement in phase.requirements:
        if requirement.evidence_role != E3_WILDCARD_ROLE:
            expanded.append(requirement)
            continue
        for record in matrix["include"]:
            coordinate_id = str(record["id"])
            expanded.append(
                Requirement(
                    id=f"{requirement.id}.{coordinate_id}",
                    authority=requirement.authority,
                    evidence_role=f"{E3_ROLE_PREFIX}{coordinate_id}",
                    matrix_cells=(f"{VERIFIED_SUBSET_CELL_PREFIX}{coordinate_id}",),
                )
            )
    return tuple(expanded)


# --- evidence projection ------------------------------------------------------


def _load_json(path: Path, *, label: str) -> Mapping[str, Any]:
    try:
        payload = loads_exact(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError, ExactJsonError) as exc:
        raise ValueError(f"{label} is not valid exact JSON: {path}: {exc}") from exc
    if not isinstance(payload, Mapping):
        raise ValueError(f"{label} must contain an object: {path}")
    return payload


def _receipt_command(payload: Mapping[str, Any]) -> str | None:
    producer = payload.get("producer")
    if isinstance(producer, Mapping):
        argv = producer.get("argv")
        if (
            isinstance(argv, list)
            and argv
            and all(isinstance(item, str) for item in argv)
        ):
            return " ".join(argv)
    command = payload.get("command")
    if isinstance(command, str) and command:
        return command
    return None


def _receipt_toolchain_digest(payload: Mapping[str, Any]) -> str | None:
    for key in ("toolchain", "toolchains", "toolchain_digest"):
        value = payload.get(key)
        if isinstance(value, str) and len(value) == 64:
            return value
        if isinstance(value, (Mapping, list)) and value:
            return canonical_json_sha256(value)
    return None


def _receipt_observed_at(payload: Mapping[str, Any]) -> str | None:
    for key in ("generated_at", "observed_at"):
        value = payload.get(key)
        if isinstance(value, str) and value:
            return value
    return None


def _receipt_cells(role: str, payload: Mapping[str, Any]) -> tuple[str, ...]:
    if role.startswith("e1_"):
        variant = payload.get("variant")
        target = payload.get("target")
        if isinstance(variant, Mapping) and isinstance(target, str):
            triple = variant.get("target_triple")
            cpython = variant.get("cpython")
            tier = variant.get("abi_tier")
            if target == "native":
                triple = "native"
            if all(isinstance(item, str) for item in (triple, cpython, tier)):
                tag = str(cpython).replace(".", "")
                return (f"pact-witness:{triple}:py{tag}:{tier}",)
        return ()
    if role == "e2_scoreboard":
        return ("perf:native:release-fast",)
    if role.startswith(E3_ROLE_PREFIX):
        return (f"{VERIFIED_SUBSET_CELL_PREFIX}{role[len(E3_ROLE_PREFIX) :]}",)
    if role.startswith("e4_"):
        return ("repository",)
    return ()


def _receipt_status(role: str, payload: Mapping[str, Any]) -> str:
    if role == "e2_scoreboard":
        cells = payload.get("cells")
        if isinstance(cells, list) and cells:
            green = reg.pa.perf_schema.VERDICT_GREEN
            if all(
                isinstance(cell, Mapping) and cell.get("verdict") == green
                for cell in cells
            ):
                return STATUS_PASS
        return STATUS_FAIL
    return STATUS_PASS if payload.get("status") == STATUS_PASS else STATUS_FAIL


def project_evidence(
    bundle_manifest: Path, requirements: Sequence[Requirement]
) -> list[dict[str, Any]]:
    """Project release-exit bundle evidence records into phase evidence rows."""
    payload = _load_json(bundle_manifest, label="release-exit manifest")
    records = payload.get("evidence")
    if not isinstance(records, list):
        raise ValueError("release-exit manifest has no evidence list")
    by_role: dict[str, Requirement] = {r.evidence_role: r for r in requirements}
    rows: list[dict[str, Any]] = []
    for record in records:
        if not isinstance(record, Mapping):
            raise ValueError("release-exit evidence records must be objects")
        role = record.get("role")
        rel_path = record.get("path")
        if not isinstance(role, str) or not isinstance(rel_path, str):
            raise ValueError("release-exit evidence records need role and path")
        requirement = by_role.get(role)
        if requirement is None:
            continue
        artifact = bundle_manifest.parent / Path(rel_path)
        artifact_payload = _load_json(artifact, label=f"{role} evidence")
        rows.append(
            {
                "requirement_id": requirement.id,
                "authority": requirement.authority,
                "command": _receipt_command(artifact_payload),
                "artifact_sha256": stable_file_sha256(
                    artifact, label=f"{role} evidence"
                ),
                "matrix_cells": list(_receipt_cells(role, artifact_payload)),
                "status": _receipt_status(role, artifact_payload),
                "toolchain_digest": _receipt_toolchain_digest(artifact_payload),
                "observed_at": _receipt_observed_at(artifact_payload),
            }
        )
    rows.sort(key=lambda row: row["requirement_id"])
    return rows


# --- attestation --------------------------------------------------------------


def attestation_subject_sha256(bundle_path: Path) -> str:
    """Return the in-toto subject digest a Sigstore bundle attests."""
    bundle = _load_json(bundle_path, label="Sigstore bundle")
    envelope = bundle.get("dsseEnvelope")
    if not isinstance(envelope, Mapping):
        raise ValueError(f"Sigstore bundle has no dsseEnvelope: {bundle_path}")
    if envelope.get("payloadType") != "application/vnd.in-toto+json":
        raise ValueError(
            f"Sigstore bundle payload is not an in-toto statement: {bundle_path}"
        )
    signatures = envelope.get("signatures")
    if not isinstance(signatures, list) or not signatures:
        raise ValueError(f"Sigstore bundle carries no signatures: {bundle_path}")
    raw_payload = envelope.get("payload")
    if not isinstance(raw_payload, str):
        raise ValueError(f"Sigstore bundle payload must be base64 text: {bundle_path}")
    try:
        statement = json.loads(base64.b64decode(raw_payload, validate=True))
    except (ValueError, json.JSONDecodeError) as exc:
        raise ValueError(
            f"Sigstore bundle payload is not a JSON statement: {exc}"
        ) from exc
    subjects = statement.get("subject") if isinstance(statement, Mapping) else None
    if not isinstance(subjects, list) or len(subjects) != 1:
        raise ValueError("phase attestation must bind exactly one subject")
    digest = subjects[0].get("digest") if isinstance(subjects[0], Mapping) else None
    sha = digest.get("sha256") if isinstance(digest, Mapping) else None
    if not isinstance(sha, str) or len(sha) != 64:
        raise ValueError("phase attestation subject has no sha256 digest")
    return sha


def _manifest_bytes_sha256(manifest: Mapping[str, Any]) -> str:
    """Digest of the manifest with its attestation slot empty (what gets signed)."""
    unsigned = dict(manifest)
    unsigned["signed_attestation"] = None
    return canonical_json_sha256(unsigned)


# --- assemble -----------------------------------------------------------------


def assemble_phase_manifest(
    *,
    phase_id: str,
    commit: str,
    bundle_manifest: Path,
    output: Path,
    attestation: Path | None = None,
    root: Path = ROOT,
    now: dt.datetime | None = None,
) -> tuple[Path, PhaseReport]:
    phases = load_phases(root / "config" / "phase_exit_requirements.toml")
    if phase_id not in phases:
        raise ValueError(f"unknown phase {phase_id!r}; declared: {sorted(phases)}")
    if not is_git_object_id(commit):
        raise ValueError("commit must be a full Git object id")
    phase = phases[phase_id]
    matrix = generated_matrix()
    requirements = expand_requirements(phase, matrix)
    evidence = project_evidence(bundle_manifest.resolve(strict=True), requirements)
    passing = {
        row["requirement_id"] for row in evidence if row["status"] == STATUS_PASS
    }
    closed_requirements = {
        requirement.id
        for requirement in phase.requirements
        if (requirement.evidence_role != E3_WILDCARD_ROLE and requirement.id in passing)
        or (
            requirement.evidence_role == E3_WILDCARD_ROLE
            and all(
                expanded.id in passing
                for expanded in requirements
                if expanded.id.startswith(requirement.id + ".")
            )
        )
    }
    open_obligations = sorted(
        obligation.id
        for obligation in phase.obligations
        if obligation.closed_by not in closed_requirements
    )
    legacy = legacy_inventory.inventory(root)
    manifest: dict[str, Any] = {
        "schema": MANIFEST_SCHEMA,
        "phase": phase_id,
        "commit": commit,
        "matrix_digest": canonical_json_sha256(matrix),
        "evidence": evidence,
        "open_obligations": open_obligations,
        "legacy_count": legacy.legacy_count,
        "signed_attestation": None,
    }
    if attestation is not None:
        resolved = attestation.resolve(strict=True)
        manifest["signed_attestation"] = {
            "kind": ATTESTATION_KIND,
            "path": resolved.name,
            "sha256": stable_file_sha256(resolved, label="phase attestation"),
            "subject_sha256": attestation_subject_sha256(resolved),
        }
    write_exact(output, manifest)
    report = verify_phase_manifest(
        output,
        release_commit=commit,
        bundle_manifest=bundle_manifest,
        root=root,
        now=now,
    )
    return output, report


# --- verify -------------------------------------------------------------------


def verify_phase_manifest(
    manifest_path: Path,
    *,
    release_commit: str,
    bundle_manifest: Path,
    root: Path = ROOT,
    now: dt.datetime | None = None,
) -> PhaseReport:
    """Evaluate the fixed §5 predicate. Every clause is checked; none is waived."""
    problems: list[str] = []
    try:
        manifest = _load_json(manifest_path, label="phase-exit manifest")
    except ValueError as exc:
        return PhaseReport(None, None, False, (str(exc),))
    phase_id = manifest.get("phase") if isinstance(manifest.get("phase"), str) else None
    commit = manifest.get("commit") if isinstance(manifest.get("commit"), str) else None

    # schema_valid
    if manifest.get("schema") != MANIFEST_SCHEMA:
        problems.append("schema: manifest schema is not molt.phase-exit-manifest.v1")
    if set(manifest) != _MANIFEST_KEYS:
        problems.append(
            "schema: manifest keys must be exactly " + ", ".join(sorted(_MANIFEST_KEYS))
        )
    evidence = manifest.get("evidence")
    if not isinstance(evidence, list):
        problems.append("schema: evidence must be a list")
        evidence = []
    for index, row in enumerate(evidence):
        if not isinstance(row, Mapping) or set(row) != _EVIDENCE_KEYS:
            problems.append(
                f"schema: evidence[{index}] keys must be exactly the §5 row fields"
            )
            continue
        for field in (
            "requirement_id",
            "authority",
            "command",
            "artifact_sha256",
            "status",
            "toolchain_digest",
            "observed_at",
        ):
            value = row[field]
            if not isinstance(value, str) or not value:
                problems.append(
                    f"schema: evidence[{index}] ({row.get('requirement_id')}) field {field} is missing"
                )
        cells = row["matrix_cells"]
        if (
            not isinstance(cells, list)
            or not cells
            or not all(isinstance(c, str) and c for c in cells)
        ):
            problems.append(
                f"schema: evidence[{index}] matrix_cells must be a non-empty string list"
            )
    if not isinstance(manifest.get("open_obligations"), list):
        problems.append("schema: open_obligations must be a list")
    if not isinstance(manifest.get("legacy_count"), int) or isinstance(
        manifest.get("legacy_count"), bool
    ):
        problems.append("schema: legacy_count must be an integer")

    phases = load_phases(root / "config" / "phase_exit_requirements.toml")
    phase = phases.get(phase_id) if phase_id is not None else None
    if phase is None:
        problems.append(f"schema: unknown phase {phase_id!r}")
        return PhaseReport(phase_id, commit, False, tuple(problems))

    # manifest.commit == release_commit
    if commit != release_commit or not is_git_object_id(commit):
        problems.append(
            f"commit: manifest commit {commit!r} is not the release commit {release_commit!r}"
        )

    # matrix_digest == generated_matrix.digest
    matrix = generated_matrix()
    expected_digest = canonical_json_sha256(matrix)
    if manifest.get("matrix_digest") != expected_digest:
        problems.append(
            "matrix: manifest matrix_digest does not match the generated verified-subset matrix"
        )

    # every required requirement has exactly one current passing row covering its cells
    requirements = expand_requirements(phase, matrix)
    rows_by_requirement: dict[str, list[Mapping[str, Any]]] = {}
    for row in evidence:
        if isinstance(row, Mapping) and isinstance(row.get("requirement_id"), str):
            rows_by_requirement.setdefault(row["requirement_id"], []).append(row)
    known_ids = {requirement.id for requirement in requirements}
    for requirement_id in sorted(set(rows_by_requirement) - known_ids):
        problems.append(f"evidence: row for undeclared requirement {requirement_id}")
    for requirement in requirements:
        rows = rows_by_requirement.get(requirement.id, [])
        if len(rows) != 1:
            problems.append(
                f"evidence: {requirement.id} needs exactly one evidence row, found {len(rows)}"
            )
            continue
        row = rows[0]
        if row.get("status") != STATUS_PASS:
            problems.append(
                f"evidence: {requirement.id} status is {row.get('status')!r}, not PASS"
            )
        if row.get("authority") != requirement.authority:
            problems.append(
                f"evidence: {requirement.id} authority {row.get('authority')!r} is not {requirement.authority!r}"
            )
        covered = set(row.get("matrix_cells") or [])
        required = set(requirement.matrix_cells)
        if requirement.evidence_role == E3_WILDCARD_ROLE:
            required = set()
        if not required <= covered:
            problems.append(
                f"evidence: {requirement.id} does not cover matrix cells "
                + ", ".join(sorted(required - covered))
            )

    # open_obligations == []
    open_obligations = manifest.get("open_obligations")
    if isinstance(open_obligations, list) and open_obligations:
        problems.append(
            "obligations: open obligations remain: "
            + ", ".join(map(str, open_obligations))
        )
    declared = {obligation.id for obligation in phase.obligations}
    passing_ids = {
        rid
        for rid, rows in rows_by_requirement.items()
        if len(rows) == 1 and rows[0].get("status") == STATUS_PASS
    }
    for obligation in phase.obligations:
        closer = obligation.closed_by
        closed = closer in passing_ids or (
            any(
                r.evidence_role == E3_WILDCARD_ROLE and r.id == closer
                for r in phase.requirements
            )
            and all(
                expanded.id in passing_ids
                for expanded in requirements
                if expanded.id.startswith(closer + ".")
            )
        )
        listed = (
            isinstance(open_obligations, list) and obligation.id in open_obligations
        )
        if not closed and not listed:
            problems.append(
                f"obligations: {obligation.id} is unmet but not listed as open"
            )
        if closed and listed:
            problems.append(
                f"obligations: {obligation.id} is listed open but its requirement passes"
            )
    if isinstance(open_obligations, list):
        for unknown in sorted(set(map(str, open_obligations)) - declared):
            problems.append(f"obligations: {unknown} is not declared for {phase.id}")

    # legacy_count == 0, and it must equal the live inventory (no stale count)
    live_legacy = legacy_inventory.inventory(root).legacy_count
    legacy_count = manifest.get("legacy_count")
    if legacy_count != live_legacy:
        problems.append(
            f"legacy: manifest legacy_count {legacy_count!r} is stale; inventory reports {live_legacy}"
        )
    if live_legacy != 0:
        problems.append(f"legacy: legacy_count is {live_legacy}, not 0")

    # signatures_and_hashes_verify
    bundle_report = reg.verify_release_bundle(bundle_manifest, repo_root=root, now=now)
    if not bundle_report.passed:
        problems.append(
            "hashes: release-exit bundle does not verify: "
            + "; ".join(bundle_report.problems)
        )
    if bundle_report.source_sha not in (None, release_commit):
        problems.append(
            "hashes: release-exit bundle source_sha is not the release commit"
        )
    try:
        projected = project_evidence(bundle_manifest.resolve(strict=True), requirements)
    except (OSError, ValueError) as exc:
        problems.append(f"hashes: cannot re-project bundle evidence: {exc}")
        projected = []
    projected_hashes = {
        row["requirement_id"]: row["artifact_sha256"] for row in projected
    }
    for row in evidence:
        if not isinstance(row, Mapping):
            continue
        rid = row.get("requirement_id")
        if isinstance(rid, str) and projected_hashes.get(rid) != row.get(
            "artifact_sha256"
        ):
            problems.append(
                f"hashes: {rid} artifact_sha256 does not match the bundle evidence bytes"
            )
    attestation = manifest.get("signed_attestation")
    if not isinstance(attestation, Mapping) or set(attestation) != _ATTESTATION_KEYS:
        problems.append("signature: manifest carries no signed attestation")
    else:
        attestation_path = manifest_path.parent / str(attestation.get("path"))
        if attestation.get("kind") != ATTESTATION_KIND:
            problems.append("signature: attestation kind must be sigstore-bundle")
        try:
            actual_sha = stable_file_sha256(attestation_path, label="phase attestation")
            subject = attestation_subject_sha256(attestation_path)
        except (OSError, ValueError) as exc:
            problems.append(f"signature: attestation is unreadable: {exc}")
        else:
            if actual_sha != attestation.get("sha256"):
                problems.append(
                    "signature: attestation bytes do not match their recorded sha256"
                )
            if subject != attestation.get("subject_sha256"):
                problems.append(
                    "signature: recorded subject digest does not match the bundle"
                )
            if subject != _manifest_bytes_sha256(manifest):
                problems.append(
                    "signature: attestation subject does not bind these manifest bytes"
                )

    return PhaseReport(phase_id, commit, not problems, tuple(problems))


# --- CLI ----------------------------------------------------------------------


def _print_report(report: PhaseReport) -> None:
    for problem in report.problems:
        print(f"[phase-exit] {problem}")
    verdict = "GREEN" if report.green else "NOT GREEN"
    print(f"[phase-exit] {report.phase or '?'} {verdict} commit={report.commit or '?'}")


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=(__doc__ or "").splitlines()[0])
    subparsers = parser.add_subparsers(dest="command", required=True)
    assemble = subparsers.add_parser(
        "assemble", help="project a phase manifest from a release-exit bundle"
    )
    assemble.add_argument("--phase", required=True)
    assemble.add_argument("--commit", required=True)
    assemble.add_argument("--release-exit-manifest", type=Path, required=True)
    assemble.add_argument("--attestation", type=Path)
    assemble.add_argument("--output", type=Path, required=True)
    verify = subparsers.add_parser("verify", help="evaluate the fixed phase predicate")
    verify.add_argument("manifest", type=Path)
    verify.add_argument("--release-commit", required=True)
    verify.add_argument("--release-exit-manifest", type=Path, required=True)
    subparsers.add_parser("matrix-digest", help="print the generated matrix digest")
    args = parser.parse_args(argv)
    if args.command == "matrix-digest":
        print(generated_matrix_digest())
        return 0
    if args.command == "assemble":
        _, report = assemble_phase_manifest(
            phase_id=args.phase,
            commit=args.commit,
            bundle_manifest=args.release_exit_manifest,
            output=args.output,
            attestation=args.attestation,
        )
    else:
        report = verify_phase_manifest(
            args.manifest,
            release_commit=args.release_commit,
            bundle_manifest=args.release_exit_manifest,
        )
    _print_report(report)
    return 0 if report.green else 1


if __name__ == "__main__":
    sys.exit(main())
