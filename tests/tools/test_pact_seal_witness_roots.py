from __future__ import annotations

import hashlib
import json
from pathlib import Path

import tools.pact_seal_witness_roots as recipe
from molt.cli.extension_manifest import _default_molt_c_api_version
from tests.cli.test_cli_extension_commands import _wasm_exporting_i64_unary_symbol


REPO_ROOT = Path(__file__).resolve().parents[2]


def _write_sealed_root(root: Path, *, molt_c_api_version: str, abi_tag: str) -> Path:
    artifact_dir = root / "numpy" / "_core"
    artifact_dir.mkdir(parents=True)
    (root / "numpy" / "__init__.py").write_text("V = 1\n", encoding="utf-8")
    (artifact_dir / "__init__.py").write_text("", encoding="utf-8")
    artifact_bytes = _wasm_exporting_i64_unary_symbol("PyInit__multiarray_umath")
    artifact_path = artifact_dir / "_multiarray_umath.molt.wasm"
    artifact_path.write_bytes(artifact_bytes)
    extension_sha256 = hashlib.sha256(artifact_bytes).hexdigest()
    manifest = {
        "schema_version": 1,
        "name": "numpy-probe",
        "version": "0.1.0",
        "module": "numpy._core._multiarray_umath",
        "molt_c_api_version": molt_c_api_version,
        "abi_tag": abi_tag,
        "python_tag": "py3",
        "target_triple": "wasm32-wasip1",
        "platform_tag": "wasm32_wasip1",
        "loader_kind": "libmolt_source",
        "init_symbol": "PyInit__multiarray_umath",
        "runtime_linkage": "static_link",
        "artifact_kind": "wasm_relocatable_object",
        "capabilities": ["module.extension.exec"],
        "extension": "_multiarray_umath.molt.wasm",
        "extension_sha256": extension_sha256,
        "python_exports": ["numpy"],
        # Sealed custody markers so re-derivation tolerates missing sources.
        "sealed_from_manifest_sha256": extension_sha256,
        "sealed_from_extension_sha256": extension_sha256,
        "provided_capsules": [],
        "object_closure": {
            "schema_version": 1,
            "root_symbol": "PyInit__multiarray_umath",
            "init_symbol_owner": "0_multiarray.o",
            "closure_sha256": extension_sha256,
            "runtime_symbols": [],
            "required_capsules": [],
            "objects": [
                {
                    "object": "0_multiarray.o",
                    "source_sha256": extension_sha256,
                    "object_sha256": extension_sha256,
                    "defined_symbols": ["PyInit__multiarray_umath"],
                    "undefined_symbols": [],
                    "required_c_api_symbols": [],
                    "required_capsules": [],
                }
            ],
        },
    }
    artifact_manifest_path = (
        artifact_dir / "_multiarray_umath.molt.wasm.extension_manifest.json"
    )
    artifact_manifest_path.write_text(
        json.dumps(manifest, indent=2) + "\n", encoding="utf-8"
    )
    root_manifest = dict(manifest)
    root_manifest["extension"] = "numpy/_core/_multiarray_umath.molt.wasm"
    (root / "extension_manifest.json").write_text(
        json.dumps(root_manifest, indent=2) + "\n", encoding="utf-8"
    )
    return artifact_manifest_path


def test_recipe_restamps_stale_root_to_current_abi(tmp_path: Path) -> None:
    root = tmp_path / "pact_numpy_sealed"
    _write_sealed_root(root, molt_c_api_version="1", abi_tag="molt_abi1")

    expected_abi = _default_molt_c_api_version(REPO_ROOT)
    expected_tag = f"molt_abi{expected_abi.split('.', 1)[0]}"

    # Stale roots must be reported by --check.
    assert recipe.main(["--check", "--root", str(root)]) == 1
    # Regenerating brings every manifest to the current ABI.
    assert recipe.main(["--root", str(root)]) == 0
    # And --check is then green (idempotent).
    assert recipe.main(["--check", "--root", str(root)]) == 0

    for rel in (
        "extension_manifest.json",
        "numpy/_core/_multiarray_umath.molt.wasm.extension_manifest.json",
    ):
        manifest = json.loads((root / rel).read_text(encoding="utf-8"))
        assert manifest["molt_c_api_version"] == expected_abi
        assert manifest["abi_tag"] == expected_tag


def test_recipe_fails_closed_on_binary_checksum_mismatch(tmp_path: Path) -> None:
    root = tmp_path / "pact_numpy_sealed"
    _write_sealed_root(root, molt_c_api_version="1", abi_tag="molt_abi1")
    # Corrupt the binary so its bytes no longer match the manifest checksum.
    artifact = root / "numpy" / "_core" / "_multiarray_umath.molt.wasm"
    artifact.write_bytes(artifact.read_bytes() + b"\x00tamper")

    # Regeneration must fail closed rather than re-stamp a mismatched binary.
    assert recipe.main(["--root", str(root)]) == 2
    # The root ABI must remain untouched (still stale, not falsely advanced).
    manifest = json.loads(
        (root / "extension_manifest.json").read_text(encoding="utf-8")
    )
    assert manifest["molt_c_api_version"] == "1"


def test_recipe_check_fails_on_stale_runtime_import_custody(
    tmp_path: Path,
    capsys,
) -> None:
    root = tmp_path / "pact_numpy_sealed"
    expected_abi = _default_molt_c_api_version(REPO_ROOT)
    expected_tag = f"molt_abi{expected_abi.split('.', 1)[0]}"
    _write_sealed_root(root, molt_c_api_version=expected_abi, abi_tag=expected_tag)
    stale_source = tmp_path / "deleted" / "npy_static_data.c"
    for rel in (
        "extension_manifest.json",
        "numpy/_core/_multiarray_umath.molt.wasm.extension_manifest.json",
    ):
        manifest_path = root / rel
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        manifest["object_closure"]["objects"][0]["source"] = str(stale_source)
        manifest["object_closure"]["objects"][0]["source_sha256"] = "1" * 64
        manifest.pop("runtime_python_import_modules", None)
        manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")

    assert recipe.main(["--check", "--root", str(root)]) == 1
    captured = capsys.readouterr()
    assert "runtime_python_import_modules" in captured.out
    assert "object_closure.objects[0].source" in captured.out


def test_recipe_reports_missing_roots(tmp_path: Path) -> None:
    missing = tmp_path / "does_not_exist"
    assert recipe.main(["--root", str(missing)]) == 1
