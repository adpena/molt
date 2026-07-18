from __future__ import annotations

import json
from pathlib import Path
import sys

from tools import check_reproducible_build as reproducibility


def test_repeated_build_requires_two_observations(tmp_path: Path) -> None:
    source = tmp_path / "source.py"
    source.write_text("print('ok')\n", encoding="utf-8")

    matched, details = reproducibility._build_repeated_and_compare(
        str(source), "dev", True, False, 1
    )

    assert matched is False
    assert details["error"] == "runs must be at least 2"


def test_repeated_build_compares_every_observation(
    tmp_path: Path, monkeypatch
) -> None:
    source = tmp_path / "source.py"
    artifact = tmp_path / "artifact.o"
    source.write_text("print('ok')\n", encoding="utf-8")
    observations = [b"same", b"same", b"different"]

    def fake_build_once(*_args):
        artifact.write_bytes(observations.pop(0))
        return str(artifact), ""

    monkeypatch.setattr(reproducibility, "_build_once", fake_build_once)
    matched, details = reproducibility._build_repeated_and_compare(
        str(source), "dev", True, False, 3
    )

    assert matched is False
    assert details["runs"] == 3
    assert details["unique_hashes"] == 2


def test_batch_receipt_counts_artifact_and_audit_results(
    tmp_path: Path, monkeypatch
) -> None:
    source = tmp_path / "source.py"
    receipt = tmp_path / "receipt.json"
    source.write_text("print('ok')\n", encoding="utf-8")
    monkeypatch.setattr(
        reproducibility,
        "_build_repeated_and_compare",
        lambda *_args, **_kwargs: (
            True,
            {"source": str(source), "runs": 2, "match": True},
        ),
    )
    monkeypatch.setattr(
        reproducibility,
        "check_ir_determinism",
        lambda _programs, _runs: [{"check": "ir", "status": "pass"}],
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_reproducible_build.py",
            "--batch",
            str(source),
            "--audit-ir",
            "--json-out",
            str(receipt),
        ],
    )

    assert reproducibility.main() == 0
    payload = json.loads(receipt.read_text(encoding="utf-8"))
    assert payload["schema"] == "molt.reproducibility-proof.v2"
    assert payload["selected"] == 2
    assert payload["executed"] == 2
    assert payload["status"] == "success"


def test_batch_receipt_fails_closed_for_missing_registered_source(
    tmp_path: Path, monkeypatch
) -> None:
    receipt = tmp_path / "receipt.json"
    missing = tmp_path / "missing.py"
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_reproducible_build.py",
            "--batch",
            str(missing),
            "--json-out",
            str(receipt),
        ],
    )

    assert reproducibility.main() == 2
    payload = json.loads(receipt.read_text(encoding="utf-8"))
    assert payload["selected"] == 1
    assert payload["executed"] == 0
    assert payload["errors"] == 1
    assert payload["status"] == "failure"


def test_single_build_mode_emits_counted_receipt(
    tmp_path: Path, monkeypatch
) -> None:
    source = tmp_path / "source.py"
    receipt = tmp_path / "receipt.json"
    source.write_text("print('ok')\n", encoding="utf-8")
    monkeypatch.setattr(
        reproducibility,
        "_build_repeated_and_compare",
        lambda *_args, **_kwargs: (
            True,
            {"source": str(source), "runs": 2, "match": True},
        ),
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_reproducible_build.py",
            "--build",
            str(source),
            "--json-out",
            str(receipt),
        ],
    )

    assert reproducibility.main() == 0
    payload = json.loads(receipt.read_text(encoding="utf-8"))
    assert payload["mode"] == "build"
    assert payload["selected"] == payload["executed"] == payload["passed"] == 1
    assert payload["status"] == "success"


def test_compare_mode_emits_counted_receipt(tmp_path: Path, monkeypatch) -> None:
    artifact = tmp_path / "artifact.bin"
    artifact.write_bytes(b"identical")
    build_a = tmp_path / "a.json"
    build_b = tmp_path / "b.json"
    receipt = tmp_path / "receipt.json"
    build_a.write_text(json.dumps({"output": str(artifact)}), encoding="utf-8")
    build_b.write_text(json.dumps({"output": str(artifact)}), encoding="utf-8")
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_reproducible_build.py",
            str(build_a),
            str(build_b),
            "--json-out",
            str(receipt),
        ],
    )

    assert reproducibility.main() == 0
    payload = json.loads(receipt.read_text(encoding="utf-8"))
    assert payload["mode"] == "compare"
    assert payload["selected"] == payload["executed"] == payload["passed"] == 1
    assert payload["status"] == "success"
