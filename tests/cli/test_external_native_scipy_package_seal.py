"""Resolution teeth for the canonical one-root SciPy witness extension set."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
import shutil

import pytest

import tools.proof_queue as proof_queue
from molt.cli.extension_manifest import _default_molt_c_api_version
from molt.cli.external_native import _resolve_external_package_native_artifact_plan
from molt.cli.source_package_seal import SourcePackageInput, stage_source_package_seal
from molt.scientific_stack_versions import scientific_extension_set
from tests.cli.test_cli_extension_commands import _wasm_exporting_i64_unary_symbol

REPO_ROOT = Path(__file__).resolve().parents[2]


def _write_native_extension(
    root: Path,
    *,
    module: str,
    target: str,
    python_exports: tuple[str, ...],
    abi_version: str,
) -> None:
    package_parts = module.split(".")[:-1]
    package_dir = root.joinpath(*package_parts)
    package_dir.mkdir(parents=True, exist_ok=True)
    for depth in range(1, len(package_parts) + 1):
        root.joinpath(*package_parts[:depth], "__init__.py").touch()
    init_symbol = f"PyInit_{target}"
    artifact_name = f"{target}.molt.wasm"
    artifact_bytes = _wasm_exporting_i64_unary_symbol(init_symbol)
    (package_dir / artifact_name).write_bytes(artifact_bytes)
    extension_sha256 = hashlib.sha256(artifact_bytes).hexdigest()
    manifest = {
        "schema_version": 1,
        "name": "scipy",
        "version": "1.18.0",
        "module": module,
        "molt_c_api_version": abi_version,
        "abi_tag": f"molt_abi{abi_version}",
        "python_tag": "py3",
        "target_triple": "wasm32-wasip1",
        "platform_tag": "wasm32_wasip1",
        "loader_kind": "libmolt_source",
        "init_symbol": init_symbol,
        "runtime_linkage": "static_link",
        "artifact_kind": "wasm_relocatable_object",
        "deterministic": True,
        "capabilities": ["module.extension.exec"],
        "extension": artifact_name,
        "extension_sha256": extension_sha256,
        "python_exports": list(python_exports),
        "sealed_from_manifest_sha256": extension_sha256,
        "sealed_from_extension_sha256": extension_sha256,
        "provided_capsules": [],
        "object_closure": {
            "schema_version": 1,
            "root_symbol": init_symbol,
            "init_symbol_owner": "0.o",
            "closure_sha256": extension_sha256,
            "runtime_symbols": [],
            "required_capsules": [],
            "objects": [
                {
                    "object": "0.o",
                    "source_sha256": extension_sha256,
                    "object_sha256": extension_sha256,
                    "defined_symbols": [init_symbol],
                    "undefined_symbols": [],
                    "required_c_api_symbols": [],
                    "required_capsules": [],
                }
            ],
        },
    }
    (package_dir / f"{artifact_name}.extension_manifest.json").write_text(
        json.dumps(manifest), encoding="utf-8"
    )


def _stage_canonical_scipy_root(root: Path, *, omit: str | None = None) -> None:
    destination = root
    root = destination.parent / f".{destination.name}.fixture-payload"
    transaction_root = destination.parent / f".{destination.name}.fixture-transaction"
    if root.exists():
        shutil.rmtree(root)
    if transaction_root.exists():
        shutil.rmtree(transaction_root)
    abi_version = _default_molt_c_api_version(REPO_ROOT)
    extension_set = scientific_extension_set("scipy", "pact-witness")
    for extension in extension_set.extensions:
        if extension.module == omit:
            continue
        _write_native_extension(
            root,
            module=extension.module,
            target=extension.target,
            python_exports=extension.python_exports,
            abi_version=abi_version,
        )
    seal = stage_source_package_seal(
        transaction_root,
        [
            SourcePackageInput(
                path,
                path.relative_to(root).as_posix(),
                "fixture",
            )
            for path in sorted(root.rglob("*"))
            if path.is_file()
        ],
    )
    shutil.copytree(seal.root, destination)
    shutil.rmtree(root)
    shutil.rmtree(transaction_root)


def _resolved_modules(root: Path) -> dict[str, str]:
    extension_set = scientific_extension_set("scipy", "pact-witness")
    plan, errors = _resolve_external_package_native_artifact_plan(
        external_module_roots=[root / "files"],
        admitted_packages={"scipy"},
        required_modules={extension.module for extension in extension_set.extensions},
    )
    assert errors == [], errors
    assert plan is not None
    return {artifact.module: artifact.init_symbol for artifact in plan.artifacts}


def test_canonical_scipy_root_resolves_exact_four_module_set(tmp_path: Path) -> None:
    root = tmp_path / "pact_scipy_witness"
    _stage_canonical_scipy_root(root)
    extension_set = scientific_extension_set("scipy", "pact-witness")

    assert _resolved_modules(root) == {
        extension.module: f"PyInit_{extension.target}"
        for extension in extension_set.extensions
    }


@pytest.mark.parametrize(
    "missing",
    [
        "scipy.ndimage._nd_image",
        "scipy.ndimage._ni_label",
        "scipy.ndimage._rank_filter_1d",
        "scipy._lib._ccallback_c",
    ],
)
def test_each_missing_scipy_extension_fails_canonical_seal_completeness(
    tmp_path: Path, missing: str
) -> None:
    root = tmp_path / "pact_scipy_witness"
    _stage_canonical_scipy_root(root, omit=missing)
    extension_set = scientific_extension_set("scipy", "pact-witness")

    problems = proof_queue._pact_scipy_witness_seal_problems(root, extension_set)

    assert any(missing in problem for problem in problems), problems


def test_extra_scipy_extension_manifest_fails_exact_set_completeness(
    tmp_path: Path,
) -> None:
    root = tmp_path / "pact_scipy_witness"
    _stage_canonical_scipy_root(root)
    unexpected = (
        root
        / "files/scipy/ndimage/_legacy.molt.wasm.extension_manifest.json"
    )
    unexpected.write_text("{}", encoding="utf-8")

    problems = proof_queue._pact_scipy_witness_seal_problems(
        root, scientific_extension_set("scipy", "pact-witness")
    )

    assert any("unexpected SciPy extension manifest" in problem for problem in problems)


def test_identical_duplicate_package_roots_are_order_independent(
    tmp_path: Path,
) -> None:
    first = tmp_path / "first"
    second = tmp_path / "second"
    _stage_canonical_scipy_root(first)
    _stage_canonical_scipy_root(second)
    extension_set = scientific_extension_set("scipy", "pact-witness")

    plan, errors = _resolve_external_package_native_artifact_plan(
        external_module_roots=[first / "files", second / "files"],
        admitted_packages={"scipy"},
        required_modules={extension.module for extension in extension_set.extensions},
    )

    assert errors == []
    assert plan is not None
    assert {artifact.module for artifact in plan.artifacts} == {
        extension.module for extension in extension_set.extensions
    }


def test_conflicting_duplicate_package_roots_fail_before_order_can_choose(
    tmp_path: Path,
) -> None:
    first = tmp_path / "first"
    second = tmp_path / "second"
    _stage_canonical_scipy_root(first)
    _stage_canonical_scipy_root(second)
    manifest_path = (
        second
        / "files/scipy/ndimage/_ni_label.molt.wasm.extension_manifest.json"
    )
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    manifest["python_exports"].append("scipy.ndimage.conflicting_owner")
    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
    extension_set = scientific_extension_set("scipy", "pact-witness")

    plan, errors = _resolve_external_package_native_artifact_plan(
        external_module_roots=[first / "files", second / "files"],
        admitted_packages={"scipy"},
        required_modules={extension.module for extension in extension_set.extensions},
    )

    assert plan is None
    assert any(
        "conflicting native artifact providers" in error
        and "Module-root order is not provider authority" in error
        for error in errors
    )
