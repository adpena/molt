from __future__ import annotations

import copy
import json
from pathlib import Path

import pytest

from tools.proof_queue_pkg import custody_cas
from tools.proof_queue_pkg import execution_receipt_details as details


def _context() -> dict:
    return {
        "run_id": "receipt-detail-unit",
        "execution_nonce_sha256": "a" * 64,
        "child_process_custody": {
            "policy": {
                "descendants": "declared",
                "allowed": [
                    {"path": f"tool-{i}", "sha256": "b" * 64} for i in range(800)
                ],
            },
            "receipt": {
                "broker_complete": True,
                "events": [{"event": "hook-start", "runtime": "python"}],
                "errors": [],
                "violations": [{"surface": "os.exec", "requested": "undeclared"}],
            },
        },
        "execution_environment": {
            "identical": True,
            "prelaunch": {
                "identity_sha256": "c" * 64,
                "variables": {
                    f"MOLT_TEST_{i}": {"hmac_sha256": "d" * 64} for i in range(800)
                },
                "passed_names": [f"MOLT_TEST_{i}" for i in range(800)],
                "omitted_names": ["UNUSED"],
                "override_names": ["MOLT_TEST_0"],
                "cargo_policies": {"RUSTFLAGS": ["-Cdebuginfo=0"]},
            },
        },
        "custody_authorities": {
            "identical": True,
            "prelaunch": [
                {"path": f"authority-{i}", "sha256": "e" * 64} for i in range(800)
            ],
        },
        "source_custody": {
            "identical": False,
            "evidence_eligible": False,
            "ineligible_reasons": [
                "source-dirty-prelaunch",
                "transient-input-mutation",
            ],
        },
        "live_input_custody": {"stable": False, "event_count": 14, "error_count": 0},
        "toolchain_capture": {"telemetry": {}},
        "process_supervisor": {"receipt": {"complete": True, "root_exit_code": 101}},
    }


def test_complete_inventories_roundtrip_below_wire_ceiling(tmp_path: Path) -> None:
    original = _context()
    before = copy.deepcopy(original)
    root = tmp_path / "custody-cas"
    wire = details.compact_context(original, cas_root=root)
    encoded = json.dumps(wire, sort_keys=True, separators=(",", ":"))
    assert len(json.dumps(original).encode()) > details.CONTEXT_LIMIT_BYTES
    assert len(encoded.encode()) < details.CONTEXT_LIMIT_BYTES
    assert original == before
    assert details.expand_context(json.loads(encoded), cas_root=root) == original
    assert wire["source_custody"] is original["source_custody"]
    assert wire["live_input_custody"] is original["live_input_custody"]
    assert wire["process_supervisor"] is original["process_supervisor"]
    # Final size telemetry is updated after projection by the producer.
    assert wire["toolchain_capture"] is original["toolchain_capture"]
    assert "hmac_sha256" not in encoded
    assert "authority-799" not in encoded
    assert "tool-799" not in encoded
    payload = custody_cas.read_ref(wire["execution_details"], expected_root=root)
    assert len(payload["fields"]) == 10
    assert (
        payload["fields"]["execution_environment/prelaunch/variables"]
        == (original["execution_environment"]["prelaunch"]["variables"])
    )
    assert details.compact_context(original, cas_root=root) == wire


@pytest.mark.parametrize(
    "field,value",
    [
        ("count", 0),
        ("count", True),
        ("count", 800.0),
        ("kind", "array"),
        ("sha256", "0" * 64),
        ("schema", "unknown"),
    ],
)
def test_projection_substitution_is_rejected(
    tmp_path: Path, field: str, value: object
) -> None:
    root = tmp_path / "custody-cas"
    wire = details.compact_context(_context(), cas_root=root)
    wire["execution_environment"]["prelaunch"]["variables"][field] = value
    with pytest.raises(ValueError, match="projection differs"):
        details.expand_context(wire, cas_root=root)


@pytest.mark.parametrize(
    "mutation",
    [
        "missing-pointer",
        "extra-pointer",
        "changed-value",
        "run",
        "nonce",
        "schema",
        "kind",
    ],
)
def test_resealed_foreign_or_incomplete_detail_is_rejected(
    tmp_path: Path, mutation: str
) -> None:
    root = tmp_path / "custody-cas"
    wire = details.compact_context(_context(), cas_root=root)
    payload = custody_cas.read_ref(wire["execution_details"], expected_root=root)
    if mutation == "missing-pointer":
        payload["fields"].pop("child_process_custody/policy/allowed")
    elif mutation == "extra-pointer":
        payload["fields"]["source_custody/ineligible_reasons"] = []
    elif mutation == "changed-value":
        payload["fields"]["child_process_custody/receipt/violations"] = []
    else:
        key = {"run": "run_id", "nonce": "execution_nonce_sha256"}.get(
            mutation, mutation
        )
        payload[key] = "foreign"
    with pytest.raises(
        ValueError, match="binding|field closure|projection differs|schema mismatch"
    ):
        wire["execution_details"] = custody_cas.put_json(root, payload).as_dict()
        details.expand_context(wire, cas_root=root)


@pytest.mark.parametrize(
    "mutation",
    [
        "missing-ref",
        "missing-blob",
        "corrupt-blob",
        "foreign-root",
        "inline",
        "missing-projection",
    ],
)
def test_detached_or_legacy_detail_cannot_be_admitted(
    tmp_path: Path, mutation: str
) -> None:
    root = tmp_path / "custody-cas"
    original = _context()
    wire = details.compact_context(original, cas_root=root)
    if mutation == "missing-ref":
        wire.pop("execution_details")
    elif mutation == "missing-blob":
        Path(wire["execution_details"]["path"]).unlink()
    elif mutation == "corrupt-blob":
        Path(wire["execution_details"]["path"]).write_bytes(b"corrupt")
    elif mutation == "foreign-root":
        wire["execution_details"] = details.compact_context(
            original, cas_root=tmp_path / "other-cas"
        )["execution_details"]
    elif mutation == "inline":
        wire = original
    else:
        wire["execution_environment"]["prelaunch"].pop("variables")
    with pytest.raises((ValueError, OSError)):
        details.expand_context(wire, cas_root=root)


@pytest.mark.parametrize(
    "key,value",
    [
        ("run_id", None),
        ("run_id", ""),
        ("execution_nonce_sha256", None),
        ("execution_nonce_sha256", "short"),
        ("execution_nonce_sha256", "g" * 64),
    ],
)
def test_detail_requires_execution_identity(
    tmp_path: Path, key: str, value: object
) -> None:
    original = _context()
    root = tmp_path / "custody-cas"
    wire = details.compact_context(original, cas_root=root)
    original[key] = wire[key] = value
    for operation, context in (
        (details.compact_context, original),
        (details.expand_context, wire),
    ):
        with pytest.raises(ValueError, match="requires a run/nonce binding"):
            operation(context, cas_root=root)


def test_projection_is_not_recursive_and_optional_inventories_are_exact(
    tmp_path: Path,
) -> None:
    root = tmp_path / "custody-cas"
    context = {"run_id": "minimal", "execution_nonce_sha256": "f" * 64}
    wire = details.compact_context(context, cas_root=root)
    assert details.expand_context(wire, cas_root=root) == context
    with pytest.raises(ValueError, match="already compact"):
        details.compact_context(wire, cas_root=root)
    context["custody_authorities"] = {"prelaunch": "not-an-inventory"}
    with pytest.raises(ValueError, match="object or array"):
        details.compact_context(context, cas_root=root)
