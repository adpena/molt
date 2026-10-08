"""Explicit owner-selected roots for real proof fixture executable placement.

Synthetic receipt/data fixtures do not select these roots. No automatic volume
fallback, retained source-bound namespaces, and no atomic pathname-race claim.
"""

from __future__ import annotations
import hashlib
import json
import os
from pathlib import Path
import secrets
import sys
from molt.dx import checkout_custody
from tools.proof_queue_pkg import cargo_output_layout, custody_cas


def metadata_root() -> dict[str, object]:
    configured = os.environ.get("MOLT_PROOF_TEST_METADATA_ROOT")
    if configured is None:
        raise ValueError(
            "Windows real queue tests require an existing short "
            "MOLT_PROOF_TEST_METADATA_ROOT supplied by the test owner"
        )
    return cargo_output_layout.declare_root(configured)


def owned_metadata_case(
    root: dict[str, object], nodeid: str, nonce: str, *, source: Path
) -> Path:
    declaration = cargo_output_layout.validate_root(root)
    assert declaration is not None
    identity = {
        "test_source_sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
        "placement_authority_sha256": hashlib.sha256(
            Path(cargo_output_layout.__file__).read_bytes()
        ).hexdigest(),
        "executable_cas_authority_sha256": hashlib.sha256(
            Path(custody_cas.__file__).read_bytes()
        ).hexdigest(),
        "nodeid": nodeid,
        "nonce": nonce,
    }
    digest = hashlib.sha256(json.dumps(identity, sort_keys=True).encode()).hexdigest()
    # Windows caps a Cargo target plus its tool descendants at 260 units, so the
    # case name uses the layout's 24-hex namespace width; the owner record keeps
    # the full digest, and a colliding name fails closed in mkdir.
    case = Path(str(declaration["path"])) / digest[:24]
    case.mkdir()
    (case / "metadata-owner.json").write_text(
        json.dumps(
            {
                "schema": "molt.proof-test-metadata-owner.v1",
                "root": declaration,
                "identity": identity,
                "namespace_sha256": digest,
            },
            sort_keys=True,
        ),
        encoding="utf-8",
    )
    return case


def native_case_path(default: Path, *, source: Path, nodeid: str) -> Path:
    if sys.platform != "win32":
        return default
    return owned_metadata_case(
        metadata_root(), nodeid, secrets.token_hex(16), source=source
    )


def native_build_environment(*, source: Path) -> dict[str, str]:
    env = dict(os.environ)
    configured = env.get("MOLT_PROOF_TEST_CARGO_OUTPUT_ROOT")
    if configured is None and sys.platform == "win32":
        raise ValueError(
            "Windows native supervisor tests require owner-selected MOLT_PROOF_TEST_CARGO_OUTPUT_ROOT"
        )
    case = native_case_path(
        checkout_custody(source.resolve().parents[2]).custody_root
        / "tmp"
        / "supervisor-fixtures",
        source=source,
        nodeid=source.name + "::supervisor-build",
    )
    layout = cargo_output_layout.CargoOutputLayout.create(
        result_root=case,
        declaration=(
            cargo_output_layout.declare_root(configured) if configured else None
        ),
        source_root=source.resolve().parents[2],
    )
    layout.admit_supervisor_target_path()
    env["CARGO_TARGET_DIR"] = str(layout.supervisor_target)
    return env
