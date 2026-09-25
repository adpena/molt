"""Teeth for tools/phase_exit_manifest.py: the fixed §5 phase predicate."""

from __future__ import annotations

import base64
import hashlib
import json
from pathlib import Path
from typing import Any

import pytest

from molt.exact_json import (
    canonical_json_bytes,
    canonical_json_sha256,
    loads_exact,
    write_exact,
)
from molt.toolchain_identity import stable_file_sha256
from tools import legacy_inventory as li
from tools import phase_exit_manifest as pem

COMMIT = "b" * 40
COORDINATE = "linux-x86_64-py312-cpython-language-gil-native"

REQUIREMENTS = f'''schema = "{pem.REQUIREMENTS_SCHEMA}"

[[phase]]
id = "T0"
title = "test phase"
matrix_authority = "tools/verified_subset.py matrix"

[[phase.requirement]]
id = "T0.pact.native"
authority = "tools/pact_witness_receipt.py"
evidence_role = "e1_native"
matrix_cells = ["pact-witness:native:py312:cpython-abi"]

[[phase.requirement]]
id = "T0.perf"
authority = "tools/perf_authority.py"
evidence_role = "e2_scoreboard"
matrix_cells = ["perf:native:release-fast"]

[[phase.requirement]]
id = "T0.verified_subset"
authority = "tools/verified_subset.py"
evidence_role = "e3_*"
matrix_cells = ["verified-subset:*"]

[[phase.requirement]]
id = "T0.structure"
authority = "tools/structural_audit.py"
evidence_role = "e4_structural_audit"
matrix_cells = ["repository"]

[[phase.obligation]]
id = "KA-NATIVE"
description = "native sister lane"
closed_by = "T0.pact.native"

[[phase.obligation]]
id = "E3"
description = "verified subset"
closed_by = "T0.verified_subset"
'''


def _receipt_payload(role: str, *, status: str = "PASS") -> dict[str, Any]:
    common = {
        "command": f"python tools/{role}.py --receipt",
        "toolchain": {"python": "3.12.13"},
        "generated_at": "2026-09-22T12:00:00Z",
    }
    if role == "e1_native":
        return {
            **common,
            "kind": "molt-pact-witness-acceptance",
            "status": status,
            "target": "native",
            "variant": {
                "cpython": "3.12",
                "abi_tier": "cpython-abi",
                "target_triple": "x86_64-pc-windows-msvc",
            },
        }
    if role == "e2_scoreboard":
        verdict = "green" if status == "PASS" else "red"
        return {**common, "cells": [{"benchmark": "b", "verdict": verdict}]}
    return {**common, "kind": role, "status": status}


def test_real_h0_projection_uses_exact_release_bundle_e3_roles() -> None:
    phase = pem.load_phases()["H0"]
    matrix = pem.generated_matrix()
    expanded = pem.expand_requirements(phase, matrix)
    roles = {row.evidence_role for row in expanded}
    assert roles == pem.reg._expected_evidence_roles(
        pem.reg.verified_subset.verified_subset_coordinates()
    )
    for coordinate in matrix["include"]:
        role = f"e3_verified_subset.{coordinate['id']}"
        assert role in roles
        assert pem._receipt_cells(role, {}) == (f"verified-subset:{coordinate['id']}",)


@pytest.fixture
def workspace(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> dict[str, Any]:
    root = tmp_path / "repo"
    (root / "config").mkdir(parents=True)
    (root / "config" / "phase_exit_requirements.toml").write_text(
        REQUIREMENTS, encoding="utf-8", newline="\n"
    )
    monkeypatch.setattr(pem.reg.pa.perf_schema, "VERDICT_GREEN", "green")
    monkeypatch.setattr(
        pem.vs,
        "matrix_payload",
        lambda *_args, **_kwargs: {"include": [{"id": COORDINATE, "python": "3.12"}]},
    )
    monkeypatch.setattr(
        pem.reg,
        "verify_release_bundle",
        lambda *_args, **_kwargs: pem.reg.ReleaseGateReport(COMMIT, "PASS", True, ()),
    )
    monkeypatch.setattr(
        li, "inventory", lambda *_args, **_kwargs: li.Inventory(0, (), ())
    )
    bundle = tmp_path / "bundle"
    bundle.mkdir()
    roles = (
        "e1_native",
        "e2_scoreboard",
        pem.reg.verified_subset_evidence_role(COORDINATE),
        "e4_structural_audit",
    )
    records = []
    for role in roles:
        path = bundle / "evidence" / f"{role}.json"
        path.parent.mkdir(exist_ok=True)
        write_exact(path, _receipt_payload(role))
        records.append(
            {
                "role": role,
                "path": f"evidence/{role}.json",
                "sha256": stable_file_sha256(path, label="test"),
                "size": path.stat().st_size,
            }
        )
    manifest = bundle / "manifest.json"
    write_exact(
        manifest,
        {
            "schema_version": 3,
            "kind": "molt-release-exit",
            "source_sha": COMMIT,
            "status": "PASS",
            "registry": {},
            "evidence": records,
        },
    )
    return {"root": root, "bundle": manifest, "out": tmp_path / "phase" / "T0.json"}


def _attest(manifest_path: Path) -> Path:
    """Write a Sigstore-shaped bundle whose in-toto subject binds the manifest bytes."""
    manifest = loads_exact(manifest_path.read_text(encoding="utf-8"))
    subject = pem._manifest_bytes_sha256(manifest)
    statement = {
        "_type": "https://in-toto.io/Statement/v1",
        "subject": [{"name": manifest_path.name, "digest": {"sha256": subject}}],
        "predicateType": "https://slsa.dev/provenance/v1",
        "predicate": {},
    }
    bundle = {
        "mediaType": "application/vnd.dev.sigstore.bundle.v0.3+json",
        "verificationMaterial": {},
        "dsseEnvelope": {
            "payload": base64.b64encode(json.dumps(statement).encode()).decode(),
            "payloadType": "application/vnd.in-toto+json",
            "signatures": [{"sig": "dGVzdA=="}],
        },
    }
    path = manifest_path.parent / "T0.sigstore.json"
    path.write_text(json.dumps(bundle), encoding="utf-8")
    return path


def _assemble(
    ws: dict[str, Any], *, attest: bool = True
) -> tuple[Path, pem.PhaseReport]:
    ws["out"].parent.mkdir(exist_ok=True)
    output, report = pem.assemble_phase_manifest(
        phase_id="T0",
        commit=COMMIT,
        bundle_manifest=ws["bundle"],
        output=ws["out"],
        root=ws["root"],
    )
    if not attest:
        return output, report
    attestation = _attest(output)
    return pem.assemble_phase_manifest(
        phase_id="T0",
        commit=COMMIT,
        bundle_manifest=ws["bundle"],
        output=ws["out"],
        attestation=attestation,
        root=ws["root"],
    )


def _verify(ws: dict[str, Any], commit: str = COMMIT) -> pem.PhaseReport:
    return pem.verify_phase_manifest(
        ws["out"], release_commit=commit, bundle_manifest=ws["bundle"], root=ws["root"]
    )


def _rewrite(ws: dict[str, Any], mutate) -> None:
    manifest = loads_exact(ws["out"].read_text(encoding="utf-8"))
    mutate(manifest)
    write_exact(ws["out"], manifest)


def test_green_phase_binds_every_clause(workspace: dict[str, Any]) -> None:
    output, report = _assemble(workspace)
    assert report.green, report.problems
    manifest = loads_exact(output.read_text(encoding="utf-8"))
    assert set(manifest) == pem._MANIFEST_KEYS
    assert manifest["matrix_digest"] == canonical_json_sha256(pem.generated_matrix())
    assert manifest["open_obligations"] == []
    assert manifest["legacy_count"] == 0
    assert [row["requirement_id"] for row in manifest["evidence"]] == [
        "T0.pact.native",
        "T0.perf",
        "T0.structure",
        f"T0.verified_subset.{COORDINATE}",
    ]
    assert all(set(row) == pem._EVIDENCE_KEYS for row in manifest["evidence"])
    assert manifest["evidence"][0]["matrix_cells"] == [
        "pact-witness:native:py312:cpython-abi"
    ]
    assert _verify(workspace).green


def test_unsigned_manifest_is_not_green(workspace: dict[str, Any]) -> None:
    _, report = _assemble(workspace, attest=False)
    assert not report.green
    assert any(
        p.startswith("signature: manifest carries no signed attestation")
        for p in report.problems
    )


def test_attestation_must_bind_these_manifest_bytes(workspace: dict[str, Any]) -> None:
    _assemble(workspace)
    _rewrite(workspace, lambda m: m.__setitem__("legacy_count", 0))
    assert _verify(workspace).green
    # Any byte change under the signature slot breaks the binding.
    _rewrite(workspace, lambda m: m["evidence"][0].__setitem__("observed_at", "later"))
    report = _verify(workspace)
    assert any("does not bind these manifest bytes" in p for p in report.problems)


def test_commit_must_match_release_commit(workspace: dict[str, Any]) -> None:
    _assemble(workspace)
    report = _verify(workspace, commit="c" * 40)
    assert not report.green
    assert any(p.startswith("commit:") for p in report.problems)


def test_failing_receipt_opens_its_obligation(workspace: dict[str, Any]) -> None:
    path = workspace["bundle"].parent / "evidence" / "e1_native.json"
    write_exact(path, _receipt_payload("e1_native", status="FAIL"))
    _, report = _assemble(workspace)
    assert not report.green
    manifest = loads_exact(workspace["out"].read_text(encoding="utf-8"))
    assert manifest["open_obligations"] == ["KA-NATIVE"]
    assert any("T0.pact.native status is 'FAIL'" in p for p in report.problems)
    assert any("open obligations remain: KA-NATIVE" in p for p in report.problems)


def test_missing_and_duplicate_rows_are_false(workspace: dict[str, Any]) -> None:
    _assemble(workspace)
    _rewrite(workspace, lambda m: m["evidence"].append(dict(m["evidence"][0])))
    report = _verify(workspace)
    assert any("needs exactly one evidence row, found 2" in p for p in report.problems)
    _rewrite(
        workspace,
        lambda m: m.__setitem__(
            "evidence",
            [r for r in m["evidence"] if r["requirement_id"] != "T0.pact.native"],
        ),
    )
    report = _verify(workspace)
    assert any(
        "T0.pact.native needs exactly one evidence row, found 0" in p
        for p in report.problems
    )


def test_matrix_drift_is_false(
    workspace: dict[str, Any], monkeypatch: pytest.MonkeyPatch
) -> None:
    _assemble(workspace)
    monkeypatch.setattr(
        pem.vs,
        "matrix_payload",
        lambda *_a, **_k: {"include": [{"id": COORDINATE + "-drifted"}]},
    )
    report = _verify(workspace)
    assert any(p.startswith("matrix:") for p in report.problems)


def test_legacy_lanes_are_false_and_stale_counts_are_named(
    workspace: dict[str, Any], monkeypatch: pytest.MonkeyPatch
) -> None:
    _assemble(workspace)
    monkeypatch.setattr(li, "inventory", lambda *_a, **_k: li.Inventory(3, (), ()))
    report = _verify(workspace)
    assert any("legacy_count 0" in p and "stale" in p for p in report.problems)
    assert any("legacy_count is 3, not 0" in p for p in report.problems)


def test_hash_drift_in_bundle_is_false(workspace: dict[str, Any]) -> None:
    _assemble(workspace)
    path = workspace["bundle"].parent / "evidence" / "e4_structural_audit.json"
    payload = _receipt_payload("e4_structural_audit")
    payload["generated_at"] = "2026-09-23T00:00:00Z"
    write_exact(path, payload)
    report = _verify(workspace)
    assert any(
        "artifact_sha256 does not match the bundle evidence bytes" in p
        for p in report.problems
    )


def test_null_fields_from_thin_authorities_are_named(workspace: dict[str, Any]) -> None:
    path = workspace["bundle"].parent / "evidence" / "e1_native.json"
    thin = _receipt_payload("e1_native")
    for key in ("command", "toolchain", "generated_at"):
        del thin[key]
    write_exact(path, thin)
    _, report = _assemble(workspace)
    assert not report.green
    for field in ("command", "toolchain_digest", "observed_at"):
        assert any(
            f"(T0.pact.native) field {field} is missing" in p for p in report.problems
        )


def test_live_registry_loads_and_h0_expands_over_the_generated_matrix() -> None:
    phases = pem.load_phases()
    assert "H0" in phases
    expanded = pem.expand_requirements(phases["H0"], pem.generated_matrix())
    e3 = [r for r in expanded if r.evidence_role.startswith("e3_")]
    assert len(e3) == len(pem.generated_matrix()["include"]) >= 36
    assert len(pem.generated_matrix_digest()) == 64


def _prepare(ws: dict[str, Any]) -> Path:
    return pem.prepare_phase_signing_subject(
        phase_id="T0",
        commit=COMMIT,
        bundle_manifest=ws["bundle"],
        output=ws["out"].with_name("T0.subject.json"),
        root=ws["root"],
    )


def _seal(
    ws: dict[str, Any], subject: Path, attestation: Path
) -> tuple[Path, pem.PhaseReport]:
    return pem.seal_phase_manifest(
        subject=subject,
        attestation=attestation,
        bundle_manifest=ws["bundle"],
        output=ws["out"],
        root=ws["root"],
    )


def test_signing_subject_bytes_have_one_canonical_identity(
    workspace: dict[str, Any],
) -> None:
    output, report = _assemble(workspace)
    assert report.green
    manifest = loads_exact(output.read_text(encoding="utf-8"))
    unsigned = {**manifest, "signed_attestation": None}
    expected = canonical_json_bytes(unsigned)
    assert pem.signing_subject_bytes(manifest) == expected
    assert pem.signing_subject_bytes(unsigned) == expected
    assert pem._manifest_bytes_sha256(manifest) == hashlib.sha256(expected).hexdigest()
    assert manifest["signed_attestation"] is not None


def test_prepare_publishes_exact_subject_but_never_waives_signature(
    workspace: dict[str, Any],
) -> None:
    subject = _prepare(workspace)
    payload = loads_exact(subject.read_text(encoding="utf-8"))
    assert subject.read_bytes() == pem.signing_subject_bytes(payload)
    assert not subject.read_bytes().endswith(b"\n")
    assert payload["signed_attestation"] is None
    report = pem.verify_phase_manifest(
        subject,
        release_commit=COMMIT,
        bundle_manifest=workspace["bundle"],
        root=workspace["root"],
    )
    assert not report.green
    assert report.problems == ("signature: manifest carries no signed attestation",)


def test_prepare_refuses_semantically_failing_subject(
    workspace: dict[str, Any],
) -> None:
    path = workspace["bundle"].parent / "evidence/e1_native.json"
    write_exact(path, _receipt_payload("e1_native", status="FAIL"))
    with pytest.raises(ValueError, match="not ready"):
        _prepare(workspace)
    assert not workspace["out"].with_name("T0.subject.json").exists()


def test_seal_uses_existing_subject_without_reprojection(
    workspace: dict[str, Any], monkeypatch: pytest.MonkeyPatch
) -> None:
    subject = _prepare(workspace)
    raw = subject.read_bytes()
    attestation = _attest(subject)

    def forbidden_projection(**_kwargs):
        pytest.fail(
            "seal attempted to replace the signed subject with a new projection"
        )

    monkeypatch.setattr(pem, "_project_phase_manifest", forbidden_projection)
    output, report = _seal(workspace, subject, attestation)
    assert report.green, report.problems
    assert (
        pem.signing_subject_bytes(loads_exact(output.read_text(encoding="utf-8")))
        == raw
    )
    assert subject.read_bytes() == raw


def test_seal_refuses_evidence_changed_after_prepare(workspace: dict[str, Any]) -> None:
    subject = _prepare(workspace)
    attestation = _attest(subject)
    path = workspace["bundle"].parent / "evidence/e1_native.json"
    payload = _receipt_payload("e1_native")
    payload["command"] = "different command"
    write_exact(path, payload)
    with pytest.raises(
        ValueError, match="bundle evidence bytes|current bundle projection"
    ):
        _seal(workspace, subject, attestation)
    assert not workspace["out"].exists()
    assert not list(workspace["out"].parent.glob(".molt-phase-seal-*.tmp"))


@pytest.mark.parametrize(
    "field,value",
    [
        ("command", "forged command"),
        ("observed_at", "2026-09-24T00:00:00Z"),
        ("toolchain_digest", "0" * 64),
        ("matrix_cells", ["pact-witness:native:py312:cpython-abi", "invented"]),
    ],
)
def test_even_resigned_rows_must_match_all_current_receipt_fields(
    workspace: dict[str, Any], field: str, value: object
) -> None:
    subject = _prepare(workspace)
    payload = loads_exact(subject.read_text(encoding="utf-8"))
    payload["evidence"][0][field] = value
    subject.write_bytes(pem.signing_subject_bytes(payload))
    attestation = _attest(subject)
    with pytest.raises(ValueError, match="current bundle projection"):
        _seal(workspace, subject, attestation)
    assert not workspace["out"].exists()


def test_seal_refuses_noncanonical_unsigned_bytes(workspace: dict[str, Any]) -> None:
    subject = _prepare(workspace)
    attestation = _attest(subject)
    subject.write_bytes(subject.read_bytes() + b"\n")
    with pytest.raises(ValueError, match="canonical unsigned bytes"):
        _seal(workspace, subject, attestation)
    assert not workspace["out"].exists()


def test_seal_refuses_an_already_attached_subject(workspace: dict[str, Any]) -> None:
    output, report = _assemble(workspace)
    assert report.green
    payload = loads_exact(output.read_text(encoding="utf-8"))
    subject = output.with_name("already-signed.json")
    subject.write_bytes(canonical_json_bytes(payload))
    with pytest.raises(ValueError, match="canonical unsigned bytes"):
        _seal(workspace, subject, output.parent / "T0.sigstore.json")


def test_seal_requires_adjacent_attestation(workspace: dict[str, Any]) -> None:
    subject = _prepare(workspace)
    attestation = _attest(subject)
    foreign = subject.parent.parent / attestation.name
    attestation.rename(foreign)
    with pytest.raises(ValueError, match="adjacent"):
        _seal(workspace, subject, foreign)
    assert not workspace["out"].exists()


def test_verify_rejects_non_adjacent_signature_path(workspace: dict[str, Any]) -> None:
    _assemble(workspace)
    _rewrite(
        workspace,
        lambda manifest: manifest["signed_attestation"].__setitem__(
            "path", "../T0.sigstore.json"
        ),
    )
    report = _verify(workspace)
    assert not report.green
    assert any(
        "signature: attestation is unreadable" in problem for problem in report.problems
    )


def test_seal_refuses_signature_of_another_subject(workspace: dict[str, Any]) -> None:
    subject = _prepare(workspace)
    payload = loads_exact(subject.read_text(encoding="utf-8"))
    other = subject.with_name("other.json")
    other.write_bytes(pem.signing_subject_bytes({**payload, "commit": "c" * 40}))
    attestation = _attest(other)
    with pytest.raises(ValueError, match="does not bind these manifest bytes"):
        _seal(workspace, subject, attestation)
    assert not workspace["out"].exists()


def test_prepare_requires_current_source_bound_bundle(
    workspace: dict[str, Any], monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(
        pem.reg,
        "verify_release_bundle",
        lambda *_args, **_kwargs: pem.reg.ReleaseGateReport(None, "PASS", True, ()),
    )
    with pytest.raises(ValueError, match="source_sha is not the release commit"):
        _prepare(workspace)


def test_seal_does_not_replace_existing_manifest(workspace: dict[str, Any]) -> None:
    subject = _prepare(workspace)
    attestation = _attest(subject)
    workspace["out"].write_bytes(b"foreign output")
    with pytest.raises(FileExistsError):
        _seal(workspace, subject, attestation)
    assert workspace["out"].read_bytes() == b"foreign output"


def test_prepare_and_seal_cli_route_through_the_same_authority(
    workspace: dict[str, Any],
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    prepare = pem.prepare_phase_signing_subject
    seal = pem.seal_phase_manifest
    monkeypatch.setattr(
        pem,
        "prepare_phase_signing_subject",
        lambda **kwargs: prepare(**kwargs, root=workspace["root"]),
    )
    monkeypatch.setattr(
        pem,
        "seal_phase_manifest",
        lambda **kwargs: seal(**kwargs, root=workspace["root"]),
    )
    subject = workspace["out"].with_name("T0.subject.json")
    assert (
        pem.main(
            [
                "prepare",
                "--phase",
                "T0",
                "--commit",
                COMMIT,
                "--release-exit-manifest",
                str(workspace["bundle"]),
                "--output",
                str(subject),
            ]
        )
        == 0
    )
    assert "PREPARED (unsigned, not phase green)" in capsys.readouterr().out
    attestation = _attest(subject)
    assert (
        pem.main(
            [
                "seal",
                "--subject",
                str(subject),
                "--attestation",
                str(attestation),
                "--release-exit-manifest",
                str(workspace["bundle"]),
                "--output",
                str(workspace["out"]),
            ]
        )
        == 0
    )
    assert "T0 GREEN" in capsys.readouterr().out
