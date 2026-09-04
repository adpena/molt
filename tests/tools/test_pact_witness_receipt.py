from __future__ import annotations

from copy import deepcopy
import json
from pathlib import Path, PurePosixPath
import shutil

import pytest

from tools import pact_witness_receipt as receipt


def _hashed_file(path: Path, *, receipt_path: Path) -> dict[str, str | int]:
    path.parent.mkdir(parents=True, exist_ok=True)
    if not path.exists():
        path.write_bytes(path.name.encode())
    return {
        key: value
        for key, value in receipt.artifact_receipt(
            "fixture", path, receipt_path=receipt_path
        ).items()
        if key != "role"
    }


def _payload(
    tmp_path: Path,
    target: str,
) -> tuple[Path, dict[str, object], dict[str, object]]:
    receipt_path = tmp_path / "acceptance-receipt.json"
    target_triple = "wasm32-wasip1" if target == "wasm" else "x86_64-pc-windows-msvc"
    variant = {
        "cpython": "3.12",
        "abi_tier": "cpython-abi",
        "target_triple": target_triple,
    }
    packages: dict[str, dict[str, str]] = {}
    expected_packages: dict[str, dict[str, str]] = {}
    for package, version, identity in (
        ("numpy", "2.5.1", "a" * 64),
        ("scipy", "1.18.0", "b" * 64),
    ):
        packages[package] = {
            "version": version,
            "module_set": "pact-witness",
            "seal_sha256": "c" * 64,
            "identity_sha256": identity,
        }
        expected_packages[package] = {
            "version": version,
            "module_set": "pact-witness",
            "identity_sha256": identity,
        }
    artifact_items = {
        "candidate_outputs": _hashed_file(
            tmp_path / "artifacts" / "candidate_outputs.npz",
            receipt_path=receipt_path,
        ),
        "reference_oracle": _hashed_file(
            tmp_path / "artifacts" / "reference_oracle.npz",
            receipt_path=receipt_path,
        ),
    }
    if target == "wasm":
        app = tmp_path / "artifacts" / "wasm" / "app.wasm"
        runtime = tmp_path / "artifacts" / "wasm" / "runtime.wasm"
        app_item = _hashed_file(app, receipt_path=receipt_path)
        runtime_item = _hashed_file(runtime, receipt_path=receipt_path)
        manifest = tmp_path / "artifacts" / "wasm" / "execution-manifest.json"
        manifest.write_text(
            json.dumps(
                {
                    "version": 2,
                    "mode": "split-runtime",
                    "modules": {
                        "app": {
                            "path": app.name,
                            "size": app.stat().st_size,
                            "sha256": app_item["sha256"],
                        },
                        "runtime": {
                            "path": runtime.name,
                            "size": runtime.stat().st_size,
                            "sha256": runtime_item["sha256"],
                        },
                    },
                    "entry": {"module": "app", "function": "molt_main"},
                }
            ),
            encoding="utf-8",
        )
        artifact_items["execution_manifest"] = _hashed_file(
            manifest,
            receipt_path=receipt_path,
        )
        artifact_items["target_artifact"] = app_item
    else:
        artifact_items["target_artifact"] = _hashed_file(
            tmp_path / "artifacts" / "native" / "app.exe",
            receipt_path=receipt_path,
        )
    artifacts = [
        {"role": role, **item} for role, item in sorted(artifact_items.items())
    ]
    payload: dict[str, object] = {
        "schema_version": receipt.SCHEMA_VERSION,
        "kind": receipt.KIND,
        "status": receipt.STATUS_PASS,
        "target": target,
        "variant": variant,
        "packages": packages,
        "git": {"source_sha": "d" * 40},
        "artifacts": artifacts,
        "parity_gate": _hashed_file(
            tmp_path / "artifacts" / "parity" / "field_solve_gates.json",
            receipt_path=receipt_path,
        ),
        "iteration_mode": False,
    }
    receipt_path.write_text(json.dumps(payload), encoding="utf-8")
    expected = {
        "target": target,
        "variant": variant,
        "packages": expected_packages,
    }
    return receipt_path, payload, expected


@pytest.mark.parametrize("target", ("native", "wasm"))
def test_acceptance_receipt_validates_exact_portable_target_contract(
    tmp_path: Path, target: str
) -> None:
    receipt_path, payload, expected = _payload(tmp_path, target)

    assert (
        receipt.validate_acceptance_receipt(
            payload,
            receipt_path=receipt_path,
            expected=expected,
        )
        == ()
    )
    assert receipt.acceptance_coordinate(payload) == (
        "3.12",
        "cpython-abi",
        payload["variant"]["target_triple"],  # type: ignore[index]
    )
    assert set(payload["git"]) == {"source_sha"}  # type: ignore[arg-type]
    for package in payload["packages"].values():  # type: ignore[union-attr]
        assert set(package) == {
            "version",
            "module_set",
            "seal_sha256",
            "identity_sha256",
        }
    hashed_items = [*payload["artifacts"], payload["parity_gate"]]  # type: ignore[misc]
    for item in hashed_items:
        relative = PurePosixPath(item["path"])
        assert not relative.is_absolute()
        assert ".." not in relative.parts
        assert item["size"] == (tmp_path / relative).stat().st_size


def test_acceptance_receipt_remains_valid_after_bundle_relocation(
    tmp_path: Path,
) -> None:
    source = tmp_path / "source"
    receipt_path, payload, expected = _payload(source, "wasm")
    destination = tmp_path / "relocated"
    shutil.copytree(source, destination)

    relocated_receipt = destination / receipt_path.name

    assert (
        receipt.validate_acceptance_receipt(
            payload,
            receipt_path=relocated_receipt,
            expected=expected,
        )
        == ()
    )


def test_acceptance_receipt_rejects_mutated_artifact_and_iteration_mode(
    tmp_path: Path,
) -> None:
    receipt_path, payload, expected = _payload(tmp_path, "native")
    candidate_item = next(
        item
        for item in payload["artifacts"]  # type: ignore[union-attr]
        if item["role"] == "candidate_outputs"
    )
    candidate = tmp_path.joinpath(*PurePosixPath(candidate_item["path"]).parts)
    candidate.write_bytes(b"mutated")
    payload["iteration_mode"] = True

    problems = receipt.validate_acceptance_receipt(
        payload,
        receipt_path=receipt_path,
        expected=expected,
    )

    assert any("artifact size mismatch" in problem for problem in problems)
    assert any("artifact checksum mismatch" in problem for problem in problems)
    assert "acceptance receipt iteration_mode must be false" in problems


def test_acceptance_receipt_rejects_registry_identity_and_role_drift(
    tmp_path: Path,
) -> None:
    receipt_path, payload, expected = _payload(tmp_path, "wasm")
    payload["packages"]["numpy"]["identity_sha256"] = "e" * 64  # type: ignore[index]
    payload["target"] = "native"
    payload["artifacts"] = payload["artifacts"][:-1]  # type: ignore[index]

    problems = receipt.validate_acceptance_receipt(
        payload,
        receipt_path=receipt_path,
        expected=expected,
    )

    assert any(
        "packages.numpy.identity_sha256 differs" in problem for problem in problems
    )
    assert any(
        "target differs from shared registry coordinate" in problem
        for problem in problems
    )
    assert any(
        "artifact roles must be sorted and exact" in problem for problem in problems
    )


def test_acceptance_receipt_rejects_mutated_split_runtime_module(
    tmp_path: Path,
) -> None:
    receipt_path, payload, expected = _payload(tmp_path, "wasm")
    manifest_item = next(
        item
        for item in payload["artifacts"]  # type: ignore[union-attr]
        if item["role"] == "execution_manifest"
    )
    manifest_path = tmp_path.joinpath(*PurePosixPath(manifest_item["path"]).parts)
    runtime = manifest_path.parent / "runtime.wasm"
    runtime.write_bytes(b"mutated runtime")

    problems = receipt.validate_acceptance_receipt(
        payload,
        receipt_path=receipt_path,
        expected=expected,
    )

    assert any(
        "modules.runtime artifact size mismatch" in problem for problem in problems
    )
    assert any(
        "modules.runtime artifact checksum mismatch" in problem for problem in problems
    )


@pytest.mark.parametrize(
    "invalid_path",
    (
        "../outside.bin",
        "/absolute/artifact.bin",
        "C:/absolute/artifact.bin",
        "artifacts\\windows-only.bin",
        "artifacts//noncanonical.bin",
    ),
)
def test_acceptance_receipt_rejects_nonportable_or_escaping_paths(
    tmp_path: Path,
    invalid_path: str,
) -> None:
    receipt_path, payload, _ = _payload(tmp_path, "native")
    payload["artifacts"][0]["path"] = invalid_path  # type: ignore[index]

    problems = receipt.validate_acceptance_receipt(
        payload,
        receipt_path=receipt_path,
        require_artifacts=False,
    )

    assert any("portable relative POSIX path" in problem for problem in problems)


def test_acceptance_receipt_rejects_unknown_fields_and_duplicate_artifacts(
    tmp_path: Path,
) -> None:
    receipt_path, payload, _ = _payload(tmp_path, "native")
    payload["packages"]["numpy"]["seal_root"] = str(tmp_path)  # type: ignore[index]
    duplicate = deepcopy(payload["artifacts"][0])  # type: ignore[index]
    payload["artifacts"].append(duplicate)  # type: ignore[union-attr]

    problems = receipt.validate_acceptance_receipt(
        payload,
        receipt_path=receipt_path,
    )

    assert "acceptance receipt packages.numpy schema is invalid" in problems
    assert "acceptance receipt artifact roles must not duplicate" in problems
    assert "acceptance receipt artifact paths must not duplicate" in problems


def test_acceptance_receipt_rejects_cross_class_casefold_collision(
    tmp_path: Path,
) -> None:
    receipt_path, payload, expected = _payload(tmp_path, "wasm")
    artifacts = payload["artifacts"]  # type: ignore[assignment]
    candidate = next(item for item in artifacts if item["role"] == "candidate_outputs")
    runtime = tmp_path / "artifacts" / "wasm" / "runtime.wasm"
    upper_runtime = runtime.with_name("RUNTIME.wasm")
    if not upper_runtime.exists():
        shutil.copyfile(runtime, upper_runtime)
    candidate.update(
        {
            "path": "artifacts/wasm/RUNTIME.wasm",
            "sha256": receipt.stable_file_sha256(
                upper_runtime,
                label="test upper runtime artifact",
            ),
            "size": upper_runtime.stat().st_size,
        }
    )

    problems = receipt.validate_acceptance_receipt(
        payload,
        receipt_path=receipt_path,
        expected=expected,
    )

    assert any(
        "modules.runtime.path collides" in problem
        and "portable filesystem identity" in problem
        for problem in problems
    )


def test_acceptance_receipt_rejects_unicode_normalization_collision(
    tmp_path: Path,
) -> None:
    receipt_path, payload, _ = _payload(tmp_path, "native")
    artifacts = payload["artifacts"]  # type: ignore[assignment]
    candidate = next(item for item in artifacts if item["role"] == "candidate_outputs")
    reference = next(item for item in artifacts if item["role"] == "reference_oracle")
    candidate["path"] = "artifacts/caf\u00e9.bin"
    reference["path"] = "artifacts/cafe\u0301.bin"

    assert receipt.portable_path_identity(candidate["path"]) == (
        receipt.portable_path_identity(reference["path"])
    )
    problems = receipt.validate_acceptance_receipt(
        payload,
        receipt_path=receipt_path,
        require_artifacts=False,
    )

    assert "acceptance receipt artifact paths must not duplicate" in problems
    assert any("portable filesystem identity" in problem for problem in problems)


@pytest.mark.parametrize(
    "value",
    ("CONIN$", "conout$.json", "COM¹.log", "bad?.json", "bad|.json"),
)
def test_acceptance_receipt_uses_shared_cross_platform_path_grammar(
    value: str,
) -> None:
    with pytest.raises(ValueError, match="portable relative POSIX path"):
        receipt.portable_relative_path(value)


def test_acceptance_receipt_accepts_sha256_git_object_id(tmp_path: Path) -> None:
    receipt_path, payload, expected = _payload(tmp_path, "native")
    payload["git"] = {"source_sha": "d" * 64}

    problems = receipt.validate_acceptance_receipt(
        payload,
        receipt_path=receipt_path,
        expected=expected,
    )

    assert not any("git.source_sha" in problem for problem in problems)


def test_acceptance_receipt_rejects_schema_1_host_path_contract(
    tmp_path: Path,
) -> None:
    receipt_path, payload, _ = _payload(tmp_path, "native")
    payload["schema_version"] = 1
    payload["git"] = {"root": str(tmp_path.resolve()), "head": "d" * 40}
    payload["packages"]["numpy"]["seal_root"] = str(tmp_path)  # type: ignore[index]
    artifact = payload["artifacts"][0]  # type: ignore[index]
    artifact["path"] = str((tmp_path / artifact["path"]).resolve())
    del artifact["size"]

    problems = receipt.validate_acceptance_receipt(
        payload,
        receipt_path=receipt_path,
        require_artifacts=False,
    )

    assert "acceptance receipt schema_version must be 2" in problems
    assert "acceptance receipt git must contain exactly source_sha" in problems
    assert "acceptance receipt packages.numpy schema is invalid" in problems
    assert any("artifacts[0] schema is invalid" in problem for problem in problems)


def test_acceptance_receipt_requires_location_for_artifact_validation(
    tmp_path: Path,
) -> None:
    _, payload, _ = _payload(tmp_path, "native")

    problems = receipt.validate_acceptance_receipt(payload)

    assert (
        "acceptance receipt_path is required to validate portable artifacts" in problems
    )
