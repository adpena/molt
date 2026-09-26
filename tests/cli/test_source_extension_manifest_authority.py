from __future__ import annotations

import copy
import hashlib
import json
import os
import shutil
import threading
from contextlib import contextmanager
from pathlib import Path, PurePosixPath, PureWindowsPath
from typing import Any

import pytest

from molt.file_locks import _acquire_file_lock, _release_file_lock
from molt.cli.source_extension_manifest_codec import (
    _BUILD_SEQUENCE_FIELDS,
    _OBJECT_SEQUENCE_FIELDS,
    _compact_source_extension_manifest,
    _expand_source_extension_manifest_authorities,
    _manifest_dependencies,
    _manifest_sequence,
    _validate_compact_source_extension_manifest,
)
from molt.cli.source_extension_input_custody import (
    SourceExtensionInputCustodyError,
    project_source_extension_manifest_inputs,
    source_extension_input_custody_path,
    source_extension_manifest_input_rows,
)
from molt.cli.source_extension_language import SourceExtensionLanguage
from molt.cli.source_extensions import (
    canonicalize_source_extension_manifest_runtime_python_imports,
    source_extension_manifest_path,
)
from molt.cli.source_extension_reproducibility import (
    _canonicalize_locations,
    _canonicalize_location_string_ordered,
    _require_location_neutral,
    _residual_producer_paths,
)
from molt.cli.source_extension_object_closure import (
    SourceExtensionObjectClosureError,
    finalize_source_extension_object_closure,
    source_extension_object_closure_digest,
    validate_source_extension_object_closure_sources,
)
from molt.cli.source_extension_object_closure_schema import (
    SOURCE_EXTENSION_NATIVE_SYMBOL_AUTHORITY,
    SOURCE_EXTENSION_OBJECT_CLOSURE_SCHEMA_VERSION,
    SOURCE_EXTENSION_WASM_SYMBOL_AUTHORITY,
)
from molt.cli.source_extension_set_identity import (
    SOURCE_EXTENSION_SET_SCHEMA_VERSION,
    _source_extension_reproduction_comparison,
)
from molt.cli.source_extension_publication import (
    _source_extension_publication_custody,
    publish_source_extension_candidate,
    recover_source_extension_publication,
)
from molt.cli.source_package_seal import (
    SourcePackageInput,
    stage_source_package_seal,
    verify_source_package_seal,
)
from molt.cli.source_extension_set_validation import (
    ValidatedSourceExtensionSetSeal,
    validate_source_extension_set_seal_contents,
    rebind_source_extension_set_receipt,
    require_source_extension_set_receipt_identity,
)
from molt.cli.source_extension_set_registry import (
    SourceExtensionSet,
    SourceExtensionSource,
    SourceExtensionSpec,
)
from molt.cli.source_package_seal import SourcePackageSealVerificationError


_SEQUENCE_OWNER_FIELDS = (
    *(("build", field) for field in _BUILD_SEQUENCE_FIELDS),
    *(
        ("object", field)
        for field in (
            "compile_command",
            "symbol_command",
            "dependencies",
            *_OBJECT_SEQUENCE_FIELDS,
        )
    ),
)


@pytest.mark.parametrize(
    ("field", "stale_symbol"),
    (("defined_symbols", "stale_defined"), ("undefined_symbols", "stale_undefined")),
)
def test_native_object_closure_rejects_stale_aggregate_symbol_provenance(
    field: str, stale_symbol: str
) -> None:
    manifest: dict[str, Any] = {
        "module": "pkg._native",
        "init_symbol": "PyInit__native",
        "artifact_kind": "static_archive",
        "object_closure": {
            "schema_version": SOURCE_EXTENSION_OBJECT_CLOSURE_SCHEMA_VERSION,
            "root_symbol": "PyInit__native",
            "init_symbol_owner": "native.o",
            "defined_symbols": ["PyInit__native"],
            "undefined_symbols": [],
            "runtime_symbols": [],
            "required_c_api_symbols": [],
            "required_capsules": [],
            "project_generated_c_api_symbols": [],
            "objects": [
                {
                    "source": "native.c",
                    "object": "native.o",
                    "language": "c",
                    "source_sha256": "1" * 64,
                    "object_sha256": "2" * 64,
                    "compile_command": ["cc", "-x", "c", "-c", "native.c"],
                    "symbol_command": ["nm", "native.o"],
                    "symbol_authority": SOURCE_EXTENSION_NATIVE_SYMBOL_AUTHORITY,
                    "dependencies": [],
                    "defined_symbols": ["PyInit__native"],
                    "undefined_symbols": [],
                    "required_c_api_symbols": [],
                    "required_capsules": [],
                    "project_generated_c_api_symbols": [],
                }
            ],
        },
        "build": {},
    }
    finalize_source_extension_object_closure(manifest)
    manifest["object_closure"][field].append(stale_symbol)

    with pytest.raises(
        SourceExtensionObjectClosureError,
        match=rf"{field} differs from the exact object projection.*{stale_symbol}",
    ):
        source_extension_object_closure_digest(
            manifest["object_closure"], manifest=manifest
        )


@pytest.mark.parametrize("escape_field", ["source", "dependency"])
def test_published_object_closure_rejects_payload_escape(
    tmp_path: Path,
    escape_field: str,
) -> None:
    publish_root = tmp_path / "publish"
    manifest_dir = publish_root / "pkg"
    provenance = publish_root / "provenance"
    manifest_dir.mkdir(parents=True)
    provenance.mkdir()
    inside_source = provenance / "native.c"
    inside_source.write_bytes(b"int native(void) { return 0; }\n")
    outside = tmp_path / ("outside.c" if escape_field == "source" else "outside.h")
    outside.write_bytes(b"outside\n")
    source = outside if escape_field == "source" else inside_source
    dependencies = []
    if escape_field == "dependency":
        dependencies.append(
            {
                "path": "../../outside.h",
                "sha256": hashlib.sha256(outside.read_bytes()).hexdigest(),
            }
        )
    closure = {
        "objects": [
            {
                "source": os.path.relpath(source, manifest_dir).replace(os.sep, "/"),
                "language": "c",
                "source_sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
                "dependencies": dependencies,
            }
        ]
    }

    with pytest.raises(
        SourceExtensionObjectClosureError,
        match="escapes sealed payload root",
    ):
        validate_source_extension_object_closure_sources(
            closure,
            manifest_dir=manifest_dir,
            allowed_root=publish_root,
        )


@contextmanager
def _held_publication_custody(destination: Path):
    lock_path = destination.parent / f".{destination.name}.producer.lock"
    handle = _acquire_file_lock(
        lock_path,
        timeout_s=1.0,
        timeout_message=f"cannot acquire fixture publication lock {lock_path}",
    )
    try:
        yield _source_extension_publication_custody(destination, handle)
    finally:
        _release_file_lock(handle)


def _publish_candidate(**kwargs: Any) -> dict[str, Any]:
    destination = kwargs["destination"]
    assert isinstance(destination, Path)
    with _held_publication_custody(destination) as custody:
        return publish_source_extension_candidate(custody=custody, **kwargs)


def _recover_publication(
    destination: Path, transaction_root: Path
) -> dict[str, Any] | None:
    with _held_publication_custody(destination) as custody:
        return recover_source_extension_publication(transaction_root, custody=custody)


def _manifest(object_count: int = 132) -> dict[str, object]:
    shared_dependencies = sorted(
        (
            {"path": f"../../inputs/header-{index}.h", "sha256": f"{index:064x}"}
            for index in range(64)
        ),
        key=lambda dependency: (
            dependency["sha256"],
            Path(dependency["path"]).name,
            dependency["path"],
        ),
    )
    objects = []
    for index in range(object_count):
        objects.append(
            {
                "source": f"../../inputs/source-{index}.c",
                "object": f"{index}.o",
                "language": "c",
                "source_sha256": hashlib.sha256(f"source-{index}".encode()).hexdigest(),
                "object_sha256": hashlib.sha256(f"object-{index}".encode()).hexdigest(),
                "defined_symbols": sorted(
                    {
                        "shared_defined",
                        f"defined_{index}",
                        *({"PyInit__native"} if index == 0 else set()),
                    }
                ),
                "undefined_symbols": ["shared_undefined"],
                "compile_command": [
                    "clang",
                    "-x",
                    "c",
                    "-c",
                    f"../../inputs/source-{index}.c",
                    "-o",
                    f"@object-root/{index}.o",
                    "-MF",
                    f"@object-root/{index}.d",
                    "-MT",
                    f"{index}.o",
                    "-mllvm",
                    "-inline-threshold=0",
                    "-mllvm",
                    "-inline-threshold=0",
                ],
                "symbol_authority": SOURCE_EXTENSION_WASM_SYMBOL_AUTHORITY,
                "dependencies": copy.deepcopy(shared_dependencies),
                "required_c_api_symbols": [],
                "required_capsules": [],
                "project_generated_c_api_symbols": [],
            }
        )
    manifest: dict[str, Any] = {
        "module": "pkg._native",
        "init_symbol": "PyInit__native",
        "artifact_kind": "wasm_relocatable_object",
        "target_triple": "wasm32-wasip1",
        "link_requirements": {
            "target_triple": "wasm32-wasip1",
            "items": [],
            "retained_symbols": [],
        },
        "build": {
            "compiler": ["clang"],
            "extra_compile_args": ["-DVALUE=1", "-DVALUE=2", "-DVALUE=1"],
            "include_dirs": ["@source/include", "@source/include", "@build/include"],
        },
        "object_closure": {
            "schema_version": SOURCE_EXTENSION_OBJECT_CLOSURE_SCHEMA_VERSION,
            "root_symbol": "PyInit__native",
            "init_symbol_owner": "0.o",
            "defined_symbols": sorted(
                {symbol for item in objects for symbol in item["defined_symbols"]}
            ),
            "undefined_symbols": ["shared_undefined"],
            "runtime_symbols": [],
            "required_c_api_symbols": [],
            "required_capsules": [],
            "project_generated_c_api_symbols": [],
            "wasm_imports": [],
            "objects": objects,
        },
    }
    finalize_source_extension_object_closure(manifest)
    return manifest


@pytest.mark.parametrize("selector", ["--target=wasm32-unknown-unknown", "-m32"])
def test_closure_identity_rejects_foreign_compiler_target(selector: str) -> None:
    manifest = _manifest(1)
    manifest["object_closure"]["objects"][0]["compile_command"].append(selector)
    with pytest.raises(SourceExtensionObjectClosureError, match="target custody"):
        finalize_source_extension_object_closure(manifest)


def test_object_units_can_share_retained_source_content() -> None:
    manifest = _manifest(2)
    objects = manifest["object_closure"]["objects"]
    first, second = objects
    second["compile_command"] = [
        first["source"] if arg == second["source"] else arg
        for arg in second["compile_command"]
    ]
    second["source"] = first["source"]
    second["source_sha256"] = first["source_sha256"]
    finalize_source_extension_object_closure(manifest)
    compact = _compact_source_extension_manifest(manifest)
    assert source_extension_object_closure_digest(
        compact["object_closure"], manifest=compact
    )


def test_producer_unit_identity_survives_compaction_and_binds_closure() -> None:
    manifest = _manifest(2)
    manifest["source_plan"] = {"target_selector": "native"}
    objects = manifest["object_closure"]["objects"]
    for index, item in enumerate(objects):
        item["producer_unit"] = {
            "target_id": f"variant{index}",
            "object": f"variant{index}.a.p/unit.o",
        }
    finalize_source_extension_object_closure(manifest)
    compact = _compact_source_extension_manifest(manifest)
    restored = _expand_source_extension_manifest_authorities(compact)
    assert [
        item["producer_unit"] for item in restored["object_closure"]["objects"]
    ] == [item["producer_unit"] for item in objects]
    original = source_extension_object_closure_digest(
        manifest["object_closure"], manifest=manifest
    )
    objects[0]["producer_unit"]["target_id"] = "different_owner"
    assert (
        source_extension_object_closure_digest(
            manifest["object_closure"], manifest=manifest
        )
        != original
    )
    objects[1]["producer_unit"]["object"] = objects[0]["producer_unit"]["object"]
    with pytest.raises(
        SourceExtensionObjectClosureError, match="duplicate producer object"
    ):
        finalize_source_extension_object_closure(manifest)


@pytest.mark.parametrize(
    "output",
    ["../unit.o", "/unit.o", "C:/unit.o", "a\\unit.o", "a/./unit.o", "a//unit.o"],
)
def test_producer_object_paths_are_canonical_build_relative(output: str) -> None:
    manifest = _manifest(1)
    manifest["object_closure"]["objects"][0]["producer_unit"] = {
        "target_id": "variant",
        "object": output,
    }
    with pytest.raises(SourceExtensionObjectClosureError, match="build-root-relative"):
        finalize_source_extension_object_closure(manifest)


def test_source_plan_requires_per_object_producer_custody() -> None:
    manifest = _manifest(1)
    manifest["source_plan"] = {"target_selector": "native"}
    with pytest.raises(
        SourceExtensionObjectClosureError, match="missing producer_unit"
    ):
        finalize_source_extension_object_closure(manifest)


@pytest.mark.parametrize("conflict", ["object", "source_sha256"])
def test_shared_source_does_not_weaken_object_or_checksum_custody(
    conflict: str,
) -> None:
    manifest = _manifest(2)
    first, second = manifest["object_closure"]["objects"]
    second["source"] = first["source"]
    if conflict == "object":
        second["object"] = first["object"]
    with pytest.raises(SourceExtensionObjectClosureError, match="custody"):
        finalize_source_extension_object_closure(manifest)


@pytest.mark.parametrize("producer_args", [None, ["/DLL"], ["-o"], [1]])
def test_compact_manifest_rejects_missing_or_foreign_producer_link_custody(
    producer_args,
) -> None:
    manifest = _manifest(1)
    manifest["source_plan"] = {
        "kind": "meson-intro-targets",
        "producer_link_args": producer_args,
    }
    with pytest.raises(ValueError):
        _validate_compact_source_extension_manifest(
            _compact_source_extension_manifest(manifest)
        )


def test_132_unit_manifest_compaction_reconstructs_exact_commands_and_content() -> None:
    manifest = _manifest()
    original = copy.deepcopy(manifest)
    original_bytes = len(json.dumps(original, sort_keys=True, indent=2).encode())

    compact = _compact_source_extension_manifest(manifest)
    _validate_compact_source_extension_manifest(compact)

    compact_bytes = len(json.dumps(compact, sort_keys=True, indent=2).encode())
    assert compact_bytes < original_bytes * 0.4
    build = compact["build"]
    assert isinstance(build, dict)
    assert _manifest_sequence(compact, build, "extra_compile_args") == [
        "-DVALUE=1",
        "-DVALUE=2",
        "-DVALUE=1",
    ]
    assert _manifest_sequence(compact, build, "include_dirs") == [
        "@source/include",
        "@source/include",
        "@build/include",
    ]
    compact_objects = compact["object_closure"]["objects"]
    original_objects = original["object_closure"]["objects"]
    for current, before in zip(compact_objects, original_objects, strict=True):
        assert (
            _manifest_sequence(compact, current, "compile_command")
            == before["compile_command"]
        )
        assert _manifest_sequence(compact, current, "symbol_command") is None
        assert current["symbol_authority"] == before["symbol_authority"]
        assert _manifest_dependencies(compact, current) == before["dependencies"]


def test_compact_unit_identity_detects_per_unit_operand_divergence() -> None:
    compact = _compact_source_extension_manifest(_manifest(object_count=2))
    first = compact["object_closure"]["objects"][0]
    operand = next(
        item
        for item in first["compile_command_operands"]
        if str(item["value"]).endswith(".o")
    )
    operand["value"] = "@object-root/diverged.o"
    with pytest.raises(ValueError, match="unit identity is false"):
        _validate_compact_source_extension_manifest(compact)


@pytest.mark.parametrize("language", list(SourceExtensionLanguage))
def test_object_language_survives_compaction_and_digest_input_projection(
    tmp_path: Path, language: SourceExtensionLanguage
) -> None:
    manifest = _manifest(object_count=1)
    item = manifest["object_closure"]["objects"][0]
    item["language"] = language.value
    command = item["compile_command"]
    command[command.index("-x") + 1] = language.driver_language
    finalize_source_extension_object_closure(manifest)
    compact = _compact_source_extension_manifest(manifest)
    _validate_compact_source_extension_manifest(compact)
    source_manifest_path = tmp_path / "build" / "extension_manifest.json"
    projected = project_source_extension_manifest_inputs(
        compact,
        source_manifest_path=source_manifest_path,
        output_manifest_path=tmp_path / "publish" / "pkg" / "extension_manifest.json",
        publish_root=tmp_path / "publish",
        staged_inputs={
            source_extension_manifest_path(
                path, manifest_path=source_manifest_path
            ): source_extension_input_custody_path(digest)
            for _field, path, digest in source_extension_manifest_input_rows(compact)
        },
    )
    projected_item = projected["object_closure"]["objects"][0]
    assert Path(projected_item["source"]).suffix == ""
    assert projected_item["language"] == language.value
    recompacted = _compact_source_extension_manifest(projected)
    _validate_compact_source_extension_manifest(recompacted)
    assert recompacted["object_closure"]["objects"][0]["language"] == language.value


def test_language_tampering_changes_object_closure_and_unit_identity() -> None:
    manifest = _manifest(object_count=1)
    original_digest = manifest["object_closure"]["closure_sha256"]
    manifest["object_closure"]["objects"][0]["language"] = "cpp"
    command = manifest["object_closure"]["objects"][0]["compile_command"]
    command[command.index("-x") + 1] = "c++"
    finalize_source_extension_object_closure(manifest)
    assert manifest["object_closure"]["closure_sha256"] != original_digest
    compact = _compact_source_extension_manifest(manifest)
    compact["object_closure"]["objects"][0]["language"] = "c"
    with pytest.raises(
        ValueError, match="unit identity is false|differs from declared"
    ):
        _validate_compact_source_extension_manifest(compact)


@pytest.mark.parametrize("language", [None, "c++", "cuda", False, {}])
def test_object_closure_rejects_missing_or_noncanonical_language(
    language: object,
) -> None:
    manifest = _manifest(object_count=1)
    item = manifest["object_closure"]["objects"][0]
    if language is None:
        item.pop("language")
    else:
        item["language"] = language
    with pytest.raises(SourceExtensionObjectClosureError, match="language"):
        finalize_source_extension_object_closure(manifest)


def test_object_closure_rejects_command_language_contradiction() -> None:
    manifest = _manifest(object_count=1)
    command = manifest["object_closure"]["objects"][0]["compile_command"]
    command[command.index("-x") + 1] = "c++"
    with pytest.raises(
        SourceExtensionObjectClosureError, match="differs from declared"
    ):
        finalize_source_extension_object_closure(manifest)


@pytest.mark.parametrize(
    "command",
    [
        ["clang", "-c", "native.c"],
        ["clang", "-c", "native.c", "-x", "c"],
        ["clang", "-x", "c", "-c", "native.c", "-x", "c++"],
    ],
)
def test_language_evidence_is_required_by_both_manifest_identities(
    command: list[str],
) -> None:
    manifest = _manifest(object_count=1)
    manifest["object_closure"]["objects"][0]["compile_command"] = command
    with pytest.raises(SourceExtensionObjectClosureError, match="canonical.*language"):
        finalize_source_extension_object_closure(copy.deepcopy(manifest))
    with pytest.raises(ValueError, match="canonical.*language"):
        _compact_source_extension_manifest(copy.deepcopy(manifest))


def test_command_template_roundtrip_preserves_literal_placeholder_tokens() -> None:
    manifest = _manifest(object_count=1)
    command = manifest["object_closure"]["objects"][0]["compile_command"]
    command.extend(["-DPLACEHOLDER=%{operand}", "/Fo%{source}"])
    opaque_start = len(command)
    command.extend(["-mllvm", "-opt-bisect-limit=0", "-Xassembler", "/Foopaque"])
    original = list(command)
    compact = _compact_source_extension_manifest(manifest)
    item = compact["object_closure"]["objects"][0]
    assert all(
        operand["index"] < opaque_start for operand in item["compile_command_operands"]
    )
    assert _manifest_sequence(compact, item, "compile_command") == original
    _validate_compact_source_extension_manifest(compact)


def test_compaction_rejects_dependency_metadata_outside_canonical_pair() -> None:
    manifest = _manifest(object_count=1)
    manifest["object_closure"]["objects"][0]["dependencies"][0]["ambient"] = "drift"
    with pytest.raises(ValueError, match="dependencies is invalid"):
        _compact_source_extension_manifest(manifest)


def test_object_closure_requires_content_ordered_dependencies() -> None:
    manifest = _manifest(object_count=1)
    dependencies = manifest["object_closure"]["objects"][0]["dependencies"]
    assert [item["sha256"] for item in dependencies] == sorted(
        item["sha256"] for item in dependencies
    )
    finalize_source_extension_object_closure(manifest)
    dependencies.sort(key=lambda item: item["path"])
    with pytest.raises(
        SourceExtensionObjectClosureError, match="dependencies are not canonical"
    ):
        finalize_source_extension_object_closure(manifest)


def test_compact_manifest_rejects_unused_string_authority() -> None:
    compact = _compact_source_extension_manifest(_manifest(object_count=1))
    compact["build_authorities"]["strings"].append("zzzz-unused-authority")
    with pytest.raises(ValueError, match="unused string authority"):
        _validate_compact_source_extension_manifest(compact)


def test_compact_manifest_rejects_unused_sequence_authority() -> None:
    compact = _compact_source_extension_manifest(_manifest(object_count=1))
    strings = compact["build_authorities"]["strings"]
    digest = hashlib.sha256(
        json.dumps([strings[0]], separators=(",", ":")).encode("utf-8")
    ).hexdigest()
    compact["build_authorities"]["sequences"][digest] = [0]
    with pytest.raises(ValueError, match="unused or dangling sequence authority"):
        _validate_compact_source_extension_manifest(compact)


@pytest.mark.parametrize("reference", [None, [], {}, True, False, 1])
@pytest.mark.parametrize(
    ("owner_kind", "field"),
    _SEQUENCE_OWNER_FIELDS,
)
def test_shared_sequence_family_rejects_invalid_reference_before_lookup(
    owner_kind: str, field: str, reference: object
) -> None:
    compact = _compact_source_extension_manifest(_manifest(object_count=1))
    item = compact["object_closure"]["objects"][0]
    owner = compact["build"] if owner_kind == "build" else item
    owner.pop(field, None)
    owner[f"{field}_ref"] = reference
    if field == "symbol_command":
        item["symbol_authority"] = SOURCE_EXTENSION_NATIVE_SYMBOL_AUTHORITY
    with pytest.raises(ValueError, match=f"{field} references an invalid sequence"):
        _manifest_sequence(compact, owner, field)
    with pytest.raises(ValueError, match=f"{field} references an invalid sequence"):
        _validate_compact_source_extension_manifest(compact)


@pytest.mark.parametrize("reference", [None, [], {}, True, False])
def test_invalid_dependency_reference_becomes_producer_diagnostic(
    tmp_path: Path, reference: object
) -> None:
    manifest = _compact_source_extension_manifest(_manifest(object_count=1))
    manifest["object_closure"]["objects"][0]["dependencies_ref"] = reference
    manifest["runtime_python_import_modules"] = ["retained"]
    before = copy.deepcopy(manifest)
    with pytest.raises(ValueError, match="dependencies references an invalid sequence"):
        _manifest_dependencies(manifest, manifest["object_closure"]["objects"][0])
    with pytest.raises(SourceExtensionInputCustodyError, match="invalid sequence"):
        source_extension_manifest_input_rows(manifest)
    errors = canonicalize_source_extension_manifest_runtime_python_imports(
        manifest, manifest_path=tmp_path / "extension_manifest.json"
    )
    assert errors == ["dependencies references an invalid sequence authority"]
    assert manifest == before


@pytest.mark.parametrize(("_owner_kind", "field"), _SEQUENCE_OWNER_FIELDS)
@pytest.mark.parametrize(
    ("inline", "reference"),
    [(None, None), (None, "reference"), ([], None), ([], "reference")],
)
def test_shared_sequence_family_rejects_both_present_even_when_null(
    _owner_kind: str, field: str, inline: object, reference: object
) -> None:
    owner = {field: inline, f"{field}_ref": reference}
    with pytest.raises(ValueError, match="has both inline and referenced authority"):
        _manifest_sequence({}, owner, field)
    if field == "dependencies":
        with pytest.raises(
            ValueError, match="has both inline and referenced authority"
        ):
            _manifest_dependencies({}, owner)


@pytest.mark.parametrize(("_owner_kind", "field"), _SEQUENCE_OWNER_FIELDS)
def test_shared_sequence_family_distinguishes_absent_empty_and_null(
    _owner_kind: str, field: str
) -> None:
    assert _manifest_sequence({}, {}, field) is None
    assert _manifest_sequence({}, {field: []}, field) == []
    with pytest.raises(ValueError, match="inline authority is invalid"):
        _manifest_sequence({}, {field: None}, field)
    if field == "dependencies":
        assert _manifest_dependencies({}, {field: []}) == []
        with pytest.raises(
            ValueError, match="inline dependencies authority is invalid"
        ):
            _manifest_dependencies({}, {field: None})


@pytest.mark.parametrize(("owner_kind", "field"), _SEQUENCE_OWNER_FIELDS)
@pytest.mark.parametrize("value", [None, False, 0, "", {}])
def test_compaction_does_not_discard_explicit_invalid_inline_fields(
    owner_kind: str, field: str, value: object
) -> None:
    manifest = _manifest(object_count=1)
    owner = (
        manifest["build"]
        if owner_kind == "build"
        else manifest["object_closure"]["objects"][0]
    )
    owner[field] = value
    with pytest.raises(ValueError, match=field):
        _compact_source_extension_manifest(manifest)


def test_explicit_null_compile_operands_are_not_absent() -> None:
    manifest = _compact_source_extension_manifest(_manifest(object_count=1))
    item = manifest["object_closure"]["objects"][0]
    item["compile_command_operands"] = None
    with pytest.raises(ValueError, match="compile_command_operands is invalid"):
        _manifest_sequence(manifest, item, "compile_command")
    with pytest.raises(ValueError, match="compile_command_operands is invalid"):
        _validate_compact_source_extension_manifest(manifest)


@pytest.mark.parametrize("value", [[], {}, True, False])
def test_compact_string_pool_rejects_invalid_values_before_canonicalization(
    value: object,
) -> None:
    manifest = _compact_source_extension_manifest(_manifest(object_count=1))
    manifest["build_authorities"]["strings"].append(value)
    with pytest.raises(ValueError, match="build authority is invalid"):
        _validate_compact_source_extension_manifest(manifest)


@pytest.mark.parametrize("value", [[], {}, True, False])
def test_compact_sequence_pool_rejects_invalid_indexes_before_lookup(
    value: object,
) -> None:
    manifest = _compact_source_extension_manifest(_manifest(object_count=1))
    sequence = next(iter(manifest["build_authorities"]["sequences"].values()))
    sequence[0] = value
    with pytest.raises(ValueError, match="invalid sequence indexes"):
        _validate_compact_source_extension_manifest(manifest)


def test_path_canonicalization_handles_joined_flags_double_slashes_and_urls(
    tmp_path: Path,
) -> None:
    root = tmp_path / "target-root"
    spelling = root.as_posix()
    doubled = spelling.replace("/", "//")
    payload = {
        "argv": [
            f"-I{doubled}//include",
            "-L" + str(root / "lib"),
            f"/LIBPATH:{spelling}/lib",
            f"--sysroot={doubled}//sysroot",
            f"@{spelling}/response.rsp",
        ],
        "url": f"https://example.invalid/{spelling}/include",
    }
    canonical = _canonicalize_locations(payload, ((root, "@target"),))
    assert canonical["argv"] == [
        "-I@target/include",
        "-L@target/lib",
        "/LIBPATH:@target/lib",
        "--sysroot=@target/sysroot",
        "@@target/response.rsp",
    ]
    assert canonical["url"] == payload["url"]
    _require_location_neutral(canonical, authority="test manifest")


@pytest.mark.parametrize(
    "residual",
    [
        "-I/usr/local/include",
        "/usr",
        "-isystem /opt/sdk/include",
        "@/tmp/compiler.rsp",
        "file:///usr/local/include",
        "~/sdk/include",
        "$HOME/sdk/include",
        "${HOME}/sdk/include",
        "%USERPROFILE%/sdk/include",
        r"/LIBPATH:C:\sdk\lib",
        r"\\server\share\sdk\include",
    ],
)
def test_nested_residual_path_gate_rejects_every_compiler_path_form(
    residual: str,
) -> None:
    findings = _residual_producer_paths(
        {"outer": [{"extension": {"build": {"argv": [residual]}}}]}
    )
    assert findings
    assert "$.outer[0].extension.build.argv[0]" in findings[0]


def test_location_projection_is_invariant_across_windows_linux_and_macos() -> None:
    windows = [
        _canonicalize_location_string_ordered(
            value, ((PureWindowsPath("C:/work/repo"), "@repo"),)
        )
        for value in (r"C:\work\repo\src\module.c", r"-IC:\work\repo\include")
    ]
    linux = [
        _canonicalize_location_string_ordered(
            value, ((PurePosixPath("/home/agent/repo"), "@repo"),)
        )
        for value in ("/home/agent/repo/src/module.c", "-I/home/agent/repo/include")
    ]
    macos = [
        _canonicalize_location_string_ordered(
            value, ((PurePosixPath("/Users/agent/repo"), "@repo"),)
        )
        for value in ("/Users/agent/repo/src/module.c", "-I/Users/agent/repo/include")
    ]
    assert windows == linux == macos == ["@repo/src/module.c", "-I@repo/include"]


def _write_identity_fixture(
    root: Path, *, producer_root: str, artifact: str
) -> dict[str, str]:
    source = root / "pkg/__init__.py"
    source.parent.mkdir(parents=True)
    source.write_text("VALUE = 1\n", encoding="utf-8")
    artifact_path = root / "pkg/_native.molt.wasm"
    from tests.cli.test_cli_extension_commands import _wasm_exporting_i64_unary_symbol
    from tests.cli.test_source_extension_producer import (
        _fixture_source_plan,
        _write_meson_metadata,
        _write_target_metadata,
    )
    from tests.python_environment_test_support import build_environment_manifest

    # A valid WASM custom section varies bytes without inventing symbol evidence.
    custom = b"\x07fixture" + artifact.encode("ascii")
    assert len(custom) < 128
    artifact_path.write_bytes(
        _wasm_exporting_i64_unary_symbol("PyInit__native")
        + b"\x00"
        + bytes([len(custom)])
        + custom
    )
    artifact_sha256 = hashlib.sha256(artifact_path.read_bytes()).hexdigest()
    sidecar = root / "pkg/_native.molt.wasm.extension_manifest.json"
    source_bytes = b"int native(void) { return 0; }\n"
    source_sha256 = hashlib.sha256(source_bytes).hexdigest()
    retained = root / source_extension_input_custody_path(source_sha256)
    retained.parent.mkdir(parents=True, exist_ok=True)
    retained.write_bytes(source_bytes)
    source_reference = os.path.relpath(retained, sidecar.parent).replace(os.sep, "/")
    wheel = root / "provenance/wheels/native.whl"
    wheel.parent.mkdir(parents=True, exist_ok=True)
    wheel.write_bytes(b"wheel:pkg._native")
    wheel_sha256 = hashlib.sha256(wheel.read_bytes()).hexdigest()
    source_plan = _fixture_source_plan(
        root, artifact_path, "pkg._native", modules=("pkg._native",)
    )
    payload = {
        "schema_version": 1,
        "name": "pkg",
        "version": "1.0.0",
        "module": "pkg._native",
        "init_symbol": "PyInit__native",
        "extension_sha256": artifact_sha256,
        "wheel_sha256": wheel_sha256,
        "wheel": "../provenance/wheels/native.whl",
        "deterministic": True,
        "python_tag": "py3",
        "target_python": "py312",
        "abi_tier": "cpython-abi",
        "target_triple": "wasm32-wasip1",
        "artifact_kind": "wasm_relocatable_object",
        "capabilities": ["module.extension.exec"],
        "python_exports": ["pkg"],
        "provided_capsules": [],
        "link_requirements": {
            "target_triple": "wasm32-wasip1",
            "items": [],
            "retained_symbols": [],
        },
        "source_plan": source_plan,
        "build": {
            "producer_host_sha256": hashlib.sha256(producer_root.encode()).hexdigest(),
            "source_plan_digest": source_plan["digest"],
        },
        "object_closure": {
            "schema_version": SOURCE_EXTENSION_OBJECT_CLOSURE_SCHEMA_VERSION,
            "root_symbol": "PyInit__native",
            "init_symbol_owner": "0.o",
            "defined_symbols": ["PyInit__native"],
            "undefined_symbols": [],
            "runtime_symbols": [],
            "required_c_api_symbols": [],
            "required_capsules": [],
            "project_generated_c_api_symbols": [],
            "wasm_imports": [],
            "objects": [
                {
                    "source": source_reference,
                    "object": "0.o",
                    "producer_unit": {
                        "target_id": "_native",
                        "object": "_native.so.p/0.o",
                    },
                    "language": "c",
                    "source_sha256": source_sha256,
                    "object_sha256": artifact_sha256,
                    "defined_symbols": ["PyInit__native"],
                    "undefined_symbols": [],
                    "dependencies": [],
                    "compile_command": ["clang", "-x", "c", "-c", source_reference],
                    "symbol_authority": SOURCE_EXTENSION_WASM_SYMBOL_AUTHORITY,
                    "required_c_api_symbols": [],
                    "required_capsules": [],
                    "project_generated_c_api_symbols": [],
                }
            ],
        },
    }
    finalize_source_extension_object_closure(payload)
    payload = _compact_source_extension_manifest(payload)
    sidecar.write_text(json.dumps(payload), encoding="utf-8")
    extension_set = SourceExtensionSet(
        package="pkg",
        package_version="1.0.0",
        name="test",
        seal_name="pkg-test",
        source=SourceExtensionSource("git", "e" * 40),
        variants=(),
        build_dependency_group="source-build-scipy",
        meson_setup_args=(),
        use_pkg_config=True,
        required_config_tools=("pkg-config",),
        required_installed_files=("pkg/__init__.py",),
        extensions=(
            SourceExtensionSpec(
                module="pkg._native",
                target="_native",
                python_exports=("pkg",),
                capabilities=("module.extension.exec",),
                provided_capsules=(),
                exclude_linked_static_libraries=(),
            ),
        ),
    )
    set_manifest = {
        "schema_version": SOURCE_EXTENSION_SET_SCHEMA_VERSION,
        "kind": "molt-source-extension-set",
        "package": "pkg",
        "package_version": "1.0.0",
        "name": "test",
        "seal_name": "pkg-test",
        "cpython": "3.12",
        "source_head": "e" * 40,
        "submodules": [],
        "target_triple": "wasm32-wasip1",
        "abi_tier": "cpython-abi",
        "installed_package_files": ["pkg/__init__.py"],
        "target_metadata": _write_target_metadata(root),
        "meson": _write_meson_metadata(root, extension_set),
        "build_environment": build_environment_manifest(),
        "extensions": [
            {
                "module": "pkg._native",
                "target": "_native",
                "python_exports": ["pkg"],
                "capabilities": ["module.extension.exec"],
                "provided_capsules": [],
                "exclude_linked_static_libraries": [],
                "artifact_sha256": artifact_sha256,
                "wheel_sha256": wheel_sha256,
                "object_closure_sha256": payload["object_closure"]["closure_sha256"],
            }
        ],
    }
    (root / "extension_set_manifest.json").write_text(
        json.dumps(set_manifest), encoding="utf-8"
    )
    return {
        path.relative_to(root).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in root.rglob("*")
        if path.is_file()
    }


@pytest.mark.parametrize("suffix", [".molt.wasm", ".molt.a"])
def test_extension_identity_rejects_extra_raw_artifact(
    tmp_path: Path, suffix: str
) -> None:
    root = tmp_path / suffix.removeprefix(".")
    _write_identity_fixture(root, producer_root="/producer/host", artifact="a" * 64)
    extra = root / f"pkg/extra{suffix}"
    extra.write_bytes(b"unregistered")

    with pytest.raises(
        ValueError, match="artifacts differ from configured complete set"
    ):
        _identity_from_root(root)


def test_extension_identity_rejects_artifact_sidecar_digest_drift(
    tmp_path: Path,
) -> None:
    root = tmp_path / "digest-drift"
    _write_identity_fixture(root, producer_root="/producer/host", artifact="a" * 64)
    artifact = root / "pkg/_native.molt.wasm"
    artifact.write_bytes(b"tampered")

    with pytest.raises(ValueError, match="artifact checksum differs from bytes"):
        _identity_from_root(root)


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("target_python", "py313"),
        ("target_triple", "x86_64-unknown-linux-gnu"),
        ("abi_tier", "source-compat"),
        ("artifact_kind", "static_archive"),
        ("module", "pkg._other"),
    ],
)
def test_extension_identity_rejects_sidecar_variant_drift(
    tmp_path: Path, field: str, value: str
) -> None:
    root = tmp_path / field
    _write_identity_fixture(root, producer_root="/producer/host", artifact="a" * 64)
    sidecar = root / "pkg/_native.molt.wasm.extension_manifest.json"
    payload = json.loads(sidecar.read_text(encoding="utf-8"))
    payload[field] = value
    sidecar.write_text(json.dumps(payload), encoding="utf-8")

    with pytest.raises(ValueError, match="extension sidecar"):
        _identity_from_root(root)


def test_canonical_identity_is_cross_platform_while_attestation_remains_exact(
    tmp_path: Path,
) -> None:
    windows = tmp_path / "windows"
    linux = tmp_path / "linux"
    _write_identity_fixture(windows, producer_root="C:/build/worker", artifact="a" * 64)
    _write_identity_fixture(
        linux, producer_root="/home/worker/build", artifact="a" * 64
    )
    windows_identity = _identity_from_root(windows)
    linux_identity = _identity_from_root(linux)
    assert windows_identity["canonical_sha256"] == linux_identity["canonical_sha256"]
    assert (
        windows_identity["producer_attestation_sha256"]
        != linux_identity["producer_attestation_sha256"]
    )

    divergent = tmp_path / "divergent"
    _write_identity_fixture(
        divergent, producer_root="/Users/worker/build", artifact="9" * 64
    )
    divergent_identity = _identity_from_root(divergent)
    comparison = _source_extension_reproduction_comparison(
        expected_incumbent_sha256=windows_identity["canonical_sha256"],
        expected_candidate_sha256=windows_identity["canonical_sha256"],
        incumbent_seal_sha256="2" * 64,
        incumbent_identity=windows_identity,
        candidate_seal_sha256="3" * 64,
        candidate_identity=divergent_identity,
    )
    assert comparison["reproduced"] is False


def test_extension_callable_abi_is_canonical_content_identity(
    tmp_path: Path,
) -> None:
    root = tmp_path / "callable-abi"
    _write_identity_fixture(
        root,
        producer_root="/producer/host",
        artifact="a" * 64,
    )
    sidecar = root / "pkg/_native.molt.wasm.extension_manifest.json"
    payload = json.loads(sidecar.read_text(encoding="utf-8"))
    export = {
        "module": "pkg._native",
        "name": "invoke",
        "binding": "module_attr",
        "abi": "molt.object_call_v1",
        "effects": [],
        "deterministic": True,
    }
    payload["callable_exports"] = [export]
    sidecar.write_text(json.dumps(payload), encoding="utf-8")
    object_call_identity = _identity_from_root(root)

    export["abi"] = "molt.object_callargs_v1"
    sidecar.write_text(json.dumps(payload), encoding="utf-8")
    callargs_identity = _identity_from_root(root)

    assert (
        object_call_identity["canonical_sha256"]
        != callargs_identity["canonical_sha256"]
    )


def test_extension_support_destination_is_canonical_content_identity(
    tmp_path: Path,
) -> None:
    root = tmp_path / "support-destination"
    _write_identity_fixture(
        root,
        producer_root="/producer/host",
        artifact="a" * 64,
    )
    sidecar = root / "pkg/_native.molt.wasm.extension_manifest.json"
    support_sha256 = hashlib.sha256(b"VALUE = 1\n").hexdigest()
    for name in ("a.py", "b.py"):
        support = root / "pkg" / name
        support.write_bytes(b"VALUE = 1\n")
    payload = json.loads(sidecar.read_text(encoding="utf-8"))
    support_entry = {"path": "pkg/a.py", "sha256": support_sha256}
    payload["support_files"] = [support_entry]
    sidecar.write_text(json.dumps(payload), encoding="utf-8")
    manifest_path = root / "extension_set_manifest.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    manifest["installed_package_files"] = ["pkg/__init__.py", "pkg/a.py", "pkg/b.py"]
    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
    first_identity = _identity_from_root(root)

    support_entry["path"] = "pkg/b.py"
    sidecar.write_text(json.dumps(payload), encoding="utf-8")
    second_identity = _identity_from_root(root)

    assert first_identity["canonical_sha256"] != second_identity["canonical_sha256"]


def test_producer_attestation_covers_complete_verified_inventory(
    tmp_path: Path,
) -> None:
    root = tmp_path / "inventory"
    _write_identity_fixture(root, producer_root="/producer/host", artifact="a" * 64)
    baseline = _identity_from_root(root)
    log = root / "provenance/logs/full-command.json"
    log.parent.mkdir(parents=True, exist_ok=True)
    log.write_text('{"command":["clang"]}', encoding="utf-8")
    extended = _identity_from_root(root)
    assert extended["canonical_sha256"] == baseline["canonical_sha256"]
    assert (
        extended["producer_attestation_sha256"]
        != baseline["producer_attestation_sha256"]
    )


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("module", "pkg..._native"),
        ("module", "pkg.class"),
        ("module", "pkg/../../escape"),
        ("module", r"pkg.\\escape"),
        ("target", "../escape"),
        ("target", r"..\\escape"),
        ("target", "C:escape"),
        ("target", "CON"),
        ("target", "nul.txt"),
        ("target", "a?b"),
        ("target", "x."),
        ("target", "x "),
        ("target", "x\x1f"),
    ],
)
def test_extension_identity_rejects_sidecar_path_escape(
    tmp_path: Path, field: str, value: str
) -> None:
    root = tmp_path / f"escape-{field}-{len(value)}"
    _write_identity_fixture(root, producer_root="/producer/host", artifact="a" * 64)
    manifest_path = root / "extension_set_manifest.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    manifest["extensions"][0][field] = value
    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
    with pytest.raises(ValueError, match="module is not import syntax|safe filename"):
        _identity_from_root(root)


def _receipt_from_root(root: Path) -> ValidatedSourceExtensionSetSeal:
    seal = stage_source_package_seal(
        root.parent / f"{root.name}-identity-store",
        [
            SourcePackageInput(path, path.relative_to(root).as_posix(), "fixture")
            for path in sorted(root.rglob("*"))
            if path.is_file()
        ],
    )
    return validate_source_extension_set_seal_contents(seal.root)


def _identity_from_root(root: Path) -> dict[str, Any]:
    return _receipt_from_root(root).identity_payload()


def _stage_identity_fixture(
    tmp_path: Path, *, label: str, artifact: str
) -> tuple[ValidatedSourceExtensionSetSeal, dict[str, Any]]:
    payload = tmp_path / f"payload-{label}"
    _write_identity_fixture(
        payload, producer_root=f"/producer/{label}", artifact=artifact
    )
    receipt = _receipt_from_root(payload)
    return receipt, receipt.identity_payload()


def test_publication_preserves_incumbent_on_divergent_candidate_expectation(
    tmp_path: Path,
) -> None:
    incumbent, incumbent_identity = _stage_identity_fixture(
        tmp_path, label="incumbent", artifact="a" * 64
    )
    candidate, _candidate_identity = _stage_identity_fixture(
        tmp_path, label="candidate", artifact="9" * 64
    )
    destination = tmp_path / "canonical"
    shutil.copytree(incumbent.seal.root, destination)

    with pytest.raises(ValueError, match="canonical identity mismatch"):
        _publish_candidate(
            destination=destination,
            candidate_receipt=candidate,
            transaction_root=tmp_path / "transaction",
            expected_incumbent_seal_sha256=incumbent.seal.seal_sha256,
            expected_incumbent_identity_sha256=incumbent_identity["canonical_sha256"],
            expected_candidate_identity_sha256="0" * 64,
        )

    assert (destination / "source-package-seal.json").read_bytes() == (
        incumbent.seal.root / "source-package-seal.json"
    ).read_bytes()


@pytest.mark.parametrize(
    "boundary", ["before-candidate-rename", "after-candidate-rename"]
)
def test_publication_aborted_journal_remains_terminal_after_later_upgrade(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    boundary: str,
) -> None:
    from molt.cli import source_extension_publication as publication

    incumbent, incumbent_identity = _stage_identity_fixture(
        tmp_path, label="old", artifact="a" * 64
    )
    candidate, candidate_identity = _stage_identity_fixture(
        tmp_path, label="failed", artifact="9" * 64
    )
    destination = tmp_path / "canonical"
    shutil.copytree(incumbent.seal.root, destination)
    transaction = tmp_path / "failed-transaction"
    real_publish = publication.durable_publish_directory_exclusive

    def fail_candidate_publication(source: Path, target: Path) -> None:
        if Path(source).name == "candidate" and Path(target) == destination:
            if boundary == "after-candidate-rename":
                real_publish(source, target)
            raise OSError(f"simulated failure {boundary}")
        real_publish(source, target)

    with monkeypatch.context() as patch:
        patch.setattr(
            publication,
            "durable_publish_directory_exclusive",
            fail_candidate_publication,
        )
        with pytest.raises(OSError, match="simulated failure"):
            _publish_candidate(
                destination=destination,
                candidate_receipt=candidate,
                transaction_root=transaction,
                expected_incumbent_seal_sha256=incumbent.seal.seal_sha256,
                expected_incumbent_identity_sha256=incumbent_identity[
                    "canonical_sha256"
                ],
                expected_candidate_identity_sha256=candidate_identity[
                    "canonical_sha256"
                ],
            )

    verify_source_package_seal(destination, expected_sha256=incumbent.seal.seal_sha256)
    recovered = _recover_publication(destination, transaction)
    assert recovered is not None and recovered["state"] == "aborted-restored"
    quarantine_key = (
        "quarantined_destination"
        if boundary == "after-candidate-rename"
        else "quarantined_candidate"
    )
    quarantine = Path(recovered[quarantine_key])
    verify_source_package_seal(quarantine, expected_sha256=candidate.seal.seal_sha256)
    record_path = transaction / "identity-publication.json"
    aborted_record = record_path.read_bytes()
    preserved_quarantine = {
        path.relative_to(quarantine).as_posix(): path.read_bytes()
        for path in quarantine.rglob("*")
        if path.is_file()
    }

    successor, successor_identity = _stage_identity_fixture(
        tmp_path, label="successor", artifact="8" * 64
    )
    result = _publish_candidate(
        destination=destination,
        candidate_receipt=successor,
        transaction_root=tmp_path / "successor-transaction",
        expected_incumbent_seal_sha256=incumbent.seal.seal_sha256,
        expected_incumbent_identity_sha256=incumbent_identity["canonical_sha256"],
        expected_candidate_identity_sha256=successor_identity["canonical_sha256"],
    )
    assert result["state"] == "committed"
    verify_source_package_seal(destination, expected_sha256=successor.seal.seal_sha256)

    # A terminal failure is historical evidence, not authority to demand A again.
    assert _recover_publication(destination, transaction) == recovered
    verify_source_package_seal(destination, expected_sha256=successor.seal.seal_sha256)
    assert record_path.read_bytes() == aborted_record
    assert {
        path.relative_to(quarantine).as_posix(): path.read_bytes()
        for path in quarantine.rglob("*")
        if path.is_file()
    } == preserved_quarantine


@pytest.mark.parametrize("attestation_drift", [False, True])
def test_publication_noop_requires_both_exact_seal_and_identity(
    tmp_path: Path,
    attestation_drift: bool,
) -> None:
    incumbent, incumbent_identity = _stage_identity_fixture(
        tmp_path, label="host-a", artifact="a" * 64
    )
    if attestation_drift:
        candidate, candidate_identity = _stage_identity_fixture(
            tmp_path, label="host-b", artifact="a" * 64
        )
    else:
        candidate, candidate_identity = incumbent, incumbent_identity
    assert (
        incumbent_identity["canonical_sha256"] == candidate_identity["canonical_sha256"]
    )
    assert (
        incumbent_identity["producer_attestation_sha256"]
        != candidate_identity["producer_attestation_sha256"]
    ) is attestation_drift
    destination = tmp_path / "canonical"
    shutil.copytree(incumbent.seal.root, destination)

    result = _publish_candidate(
        destination=destination,
        candidate_receipt=candidate,
        transaction_root=tmp_path / "transaction",
        expected_incumbent_seal_sha256=incumbent.seal.seal_sha256,
        expected_incumbent_identity_sha256=incumbent_identity["canonical_sha256"],
        expected_candidate_identity_sha256=candidate_identity["canonical_sha256"],
    )
    assert result["no_op"] is (not attestation_drift)
    assert (destination / "source-package-seal.json").read_bytes() == (
        candidate.seal.root / "source-package-seal.json"
    ).read_bytes()


def test_publication_detects_stale_incumbent_race_before_retirement(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt.cli import source_extension_publication as publication
    from molt.cli.source_package_seal import SourcePackageSealVerificationError

    incumbent, incumbent_identity = _stage_identity_fixture(
        tmp_path, label="race-old", artifact="a" * 64
    )
    candidate, candidate_identity = _stage_identity_fixture(
        tmp_path, label="race-new", artifact="9" * 64
    )
    stale, _stale_identity = _stage_identity_fixture(
        tmp_path, label="race-stale", artifact="8" * 64
    )
    destination = tmp_path / "canonical"
    shutil.copytree(incumbent.seal.root, destination)
    real_resume = publication._resume_source_extension_publication

    def race(
        record_path: Path, custody: Any, *, verifier: Any = None
    ) -> dict[str, Any]:
        shutil.rmtree(destination)
        shutil.copytree(stale.seal.root, destination)
        return real_resume(record_path, custody, verifier=verifier)

    monkeypatch.setattr(publication, "_resume_source_extension_publication", race)
    with pytest.raises(SourcePackageSealVerificationError):
        _publish_candidate(
            destination=destination,
            candidate_receipt=candidate,
            transaction_root=tmp_path / "transaction",
            expected_incumbent_seal_sha256=incumbent.seal.seal_sha256,
            expected_incumbent_identity_sha256=incumbent_identity["canonical_sha256"],
            expected_candidate_identity_sha256=candidate_identity["canonical_sha256"],
        )
    assert (destination / "source-package-seal.json").read_bytes() == (
        stale.seal.root / "source-package-seal.json"
    ).read_bytes()


@pytest.mark.parametrize("forge_lock_path", [False, True])
def test_publication_rejects_forged_destination_lock_custody(
    tmp_path: Path,
    forge_lock_path: bool,
) -> None:
    from dataclasses import replace

    owned = (tmp_path / "owned").resolve()
    foreign = (tmp_path / "foreign").resolve()
    with _held_publication_custody(owned) as custody:
        forged = replace(
            custody,
            destination=foreign,
            lock_path=(
                foreign.parent / f".{foreign.name}.producer.lock"
                if forge_lock_path
                else custody.lock_path
            ),
        )
        with pytest.raises(SourcePackageSealVerificationError, match="custody"):
            recover_source_extension_publication(
                tmp_path / "transaction", custody=forged
            )
    assert not foreign.exists()
    assert not (tmp_path / "transaction").exists()


def test_publication_rejects_released_producer_lock_custody(tmp_path: Path) -> None:
    destination = tmp_path / "canonical"
    lock_path = destination.parent / f".{destination.name}.producer.lock"
    handle = _acquire_file_lock(
        lock_path,
        timeout_s=1.0,
        timeout_message="fixture lock unavailable",
    )
    custody = _source_extension_publication_custody(destination, handle)
    _release_file_lock(handle)
    with pytest.raises(SourcePackageSealVerificationError, match="live exclusive"):
        recover_source_extension_publication(tmp_path / "transaction", custody=custody)


@pytest.mark.parametrize(
    "mutation",
    [
        "missing-field",
        "unknown-kind",
        "unknown-state",
        "list-state",
        "dict-state",
        "bad-hash",
        "extra-authority",
        "relative-path",
        "invalid-path",
        "noncanonical-path",
    ],
)
def test_publication_recovery_rejects_malformed_record_without_mutation(
    tmp_path: Path, mutation: str
) -> None:
    destination = (tmp_path / "canonical").resolve()
    destination.mkdir()
    marker = destination / "incumbent.txt"
    marker.write_text("preserve\n", encoding="utf-8")
    transaction = (tmp_path / "transaction").resolve()
    transaction.mkdir()
    publication_root = transaction / "identity-publication"
    record: dict[str, object] = {
        "schema_version": 2,
        "kind": "source-extension-seal-compare-and-swap",
        "state": "prepared",
        "destination": str(destination),
        "candidate": str(publication_root / "candidate"),
        "retired": str(publication_root / "retired"),
        "quarantined_candidate": str(publication_root / "quarantined-candidate"),
        "quarantined_destination": str(publication_root / "quarantined-destination"),
        "incumbent_seal_sha256": "1" * 64,
        "candidate_seal_sha256": "2" * 64,
        "incumbent_identity_sha256": "3" * 64,
        "candidate_identity_sha256": "4" * 64,
    }
    if mutation == "missing-field":
        record.pop("candidate_identity_sha256")
    elif mutation == "unknown-kind":
        record["kind"] = "legacy-publication"
    elif mutation == "unknown-state":
        record["state"] = "mystery"
    elif mutation == "list-state":
        record["state"] = ["prepared"]
    elif mutation == "dict-state":
        record["state"] = {"state": "prepared"}
    elif mutation == "bad-hash":
        record["candidate_seal_sha256"] = "NOT-A-HASH"
    elif mutation == "extra-authority":
        record["ambient_destination"] = str(tmp_path / "escape")
    elif mutation == "relative-path":
        record["candidate"] = "identity-publication/candidate"
    elif mutation == "invalid-path":
        record["candidate"] = "C:\\invalid\0path"
    else:
        record["destination"] = str(
            destination.parent / "nonexistent" / ".." / destination.name
        )
    (transaction / "identity-publication.json").write_text(
        json.dumps(record), encoding="utf-8"
    )

    with _held_publication_custody(destination) as custody:
        with pytest.raises(SourcePackageSealVerificationError):
            recover_source_extension_publication(transaction, custody=custody)

    assert marker.read_text(encoding="utf-8") == "preserve\n"
    assert not publication_root.exists()


def test_destination_scoped_custody_serializes_competing_cas_publishers(
    tmp_path: Path,
) -> None:
    incumbent, incumbent_identity = _stage_identity_fixture(
        tmp_path, label="race-incumbent", artifact="a" * 64
    )
    candidates = [
        _stage_identity_fixture(tmp_path, label=label, artifact=artifact)
        for label, artifact in (("race-a", "8" * 64), ("race-b", "9" * 64))
    ]
    destination = tmp_path / "canonical"
    shutil.copytree(incumbent.seal.root, destination)
    barrier = threading.Barrier(2)
    results: list[tuple[int, dict[str, Any]]] = []
    failures: list[tuple[int, BaseException]] = []

    def compete(index: int) -> None:
        candidate, candidate_identity = candidates[index]
        barrier.wait()
        try:
            with _held_publication_custody(destination) as custody:
                result = publish_source_extension_candidate(
                    custody=custody,
                    destination=destination,
                    candidate_receipt=candidate,
                    transaction_root=tmp_path / f"transaction-{index}",
                    expected_incumbent_seal_sha256=incumbent.seal.seal_sha256,
                    expected_incumbent_identity_sha256=(
                        incumbent_identity["canonical_sha256"]
                    ),
                    expected_candidate_identity_sha256=(
                        candidate_identity["canonical_sha256"]
                    ),
                )
                results.append((index, result))
        except BaseException as exc:
            failures.append((index, exc))

    threads = [threading.Thread(target=compete, args=(index,)) for index in range(2)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join(timeout=10.0)
        assert not thread.is_alive()

    assert len(results) == 1
    assert len(failures) == 1
    assert "mismatch" in str(failures[0][1])
    winner, result = results[0]
    assert result["upgraded"] is True
    assert (destination / "source-package-seal.json").read_bytes() == (
        candidates[winner][0].seal.root / "source-package-seal.json"
    ).read_bytes()


def test_validated_receipt_is_deeply_immutable_and_projection_has_no_io(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from dataclasses import FrozenInstanceError

    receipt, identity = _stage_identity_fixture(
        tmp_path, label="immutable", artifact="a" * 64
    )
    mutated = receipt.manifest_payload()
    mutated["extensions"][0]["capabilities"].append("filesystem.write")
    assert receipt.manifest_payload()["extensions"][0]["capabilities"] == [
        "module.extension.exec"
    ]
    with pytest.raises(FrozenInstanceError):
        setattr(receipt.canonical_identity, "canonical_sha256", "0" * 64)

    def forbid_io(*_args: Any, **_kwargs: Any) -> Any:
        raise AssertionError("receipt consumer performed filesystem IO")

    monkeypatch.setattr(Path, "read_bytes", forbid_io)
    monkeypatch.setattr(Path, "read_text", forbid_io)
    assert receipt.identity_payload() == identity
    assert (
        require_source_extension_set_receipt_identity(
            receipt,
            identity["canonical_sha256"],
        )
        is receipt
    )


def test_receipt_rebinding_requires_exact_verified_seal(tmp_path: Path) -> None:
    receipt, _identity = _stage_identity_fixture(
        tmp_path, label="rebind", artifact="a" * 64
    )
    copied = tmp_path / "copied"
    shutil.copytree(receipt.seal.root, copied)
    rebound = rebind_source_extension_set_receipt(
        receipt,
        verify_source_package_seal(copied, expected_sha256=receipt.seal.seal_sha256),
    )
    assert rebound.seal.root == copied.resolve()
    assert rebound.validation is receipt.validation
    assert rebound.canonical_identity is receipt.canonical_identity
    other, _ = _stage_identity_fixture(tmp_path, label="other", artifact="9" * 64)
    with pytest.raises(ValueError, match="different seal bytes"):
        rebind_source_extension_set_receipt(receipt, other.seal)


def test_receipt_issuance_detects_installed_byte_mutation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt.cli import source_extension_set_validation as validation

    receipt, _ = _stage_identity_fixture(tmp_path, label="mutate", artifact="a" * 64)
    original = validation._validate_source_extension_set_payload

    def mutate_after_validation(root: Path, manifest: Any, **kwargs: Any) -> Any:
        facts = original(root, manifest, **kwargs)
        (root / "pkg/__init__.py").write_bytes(b"tampered")
        return facts

    monkeypatch.setattr(
        validation, "_validate_source_extension_set_payload", mutate_after_validation
    )
    with pytest.raises(SourcePackageSealVerificationError):
        validation.validate_source_extension_set_seal_contents(receipt.seal.root)


@pytest.mark.parametrize(
    ("path", "value", "error"),
    [
        (("schema_version",), True, "manifest schema is invalid"),
        (("schema_version",), 5.0, "manifest schema is invalid"),
        (("target",), "wasm", "keys differ from schema"),
        (("target_metadata", "schema_version"), True, "metadata contract differs"),
        (("target_metadata", "schema_version"), 4.0, "metadata contract differs"),
        (
            ("target_metadata", "build_toolchain", "target_triple"),
            "wasm32-wasip1",
            "build-machine toolchain is invalid",
        ),
        (
            ("target_metadata", "build_toolchain", "compiler_kind"),
            "",
            "build-machine coordinates",
        ),
        (
            ("target_metadata", "build_toolchain", "commands", "c"),
            ["unattested-compiler"],
            "identity is invalid",
        ),
        (
            ("target_metadata", "build_toolchain", "commands", "c"),
            ["clang", "--target=wasm32-wasip1"],
            "command is invalid",
        ),
        (
            ("target_metadata", "digests", "meson_native_sha256"),
            "0" * 64,
            "meson_native_sha256 is false",
        ),
        (
            ("target_metadata", "paths", "pkg_config_dir"),
            None,
            "no Meson pkg-config path",
        ),
        (
            ("target_metadata", "abi", "include_dirs"),
            ["@molt/include", None],
            "invalid Meson include paths",
        ),
        (
            (
                "target_metadata",
                "toolchain",
                "link_probe_archives",
                "compiler_builtins",
                "path",
            ),
            None,
            "no Meson compiler-builtins path",
        ),
        (
            ("target_metadata", "target", "requested"),
            True,
            "requested-target authority",
        ),
        (("target_metadata", "toolchain", "commands", "c"), True, "non-empty list"),
        (
            ("target_metadata", "toolchain", "commands", "c"),
            ["clang", True],
            "must be strings",
        ),
        (
            ("target_metadata", "toolchain", "commands", "c"),
            [""],
            "requires an executable",
        ),
        (
            ("target_metadata", "toolchain", "tools", "cc", "command"),
            ["clang", False],
            "must be strings",
        ),
    ],
)
def test_structural_receipt_rejects_inexact_schema_and_command_types(
    tmp_path: Path,
    path: tuple[str, ...],
    value: object,
    error: str,
) -> None:
    root = tmp_path / "invalid-contract"
    _write_identity_fixture(root, producer_root="/producer/host", artifact="a" * 64)
    manifest_path = root / "extension_set_manifest.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    owner = manifest
    for key in path[:-1]:
        owner = owner[key]
    owner[path[-1]] = value

    # Keep all byte/digest custody valid so the actual type gate is exercised.
    target_metadata = manifest["target_metadata"]
    target_identity = dict(target_metadata)
    target_identity.pop("digest")
    target_metadata["digest"] = hashlib.sha256(
        json.dumps(target_identity, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()
    target_sidecar = (
        root / "provenance/metadata/target/source-extension-target-metadata.json"
    )
    target_sidecar.write_text(json.dumps(target_metadata), encoding="utf-8")
    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
    with pytest.raises(ValueError, match=error):
        _identity_from_root(root)


@pytest.mark.parametrize("machine", ["cross", "native"])
def test_structural_receipt_rejects_rehashed_unbound_meson_machine(
    tmp_path: Path, machine: str
) -> None:
    root = tmp_path / f"unbound-{machine}"
    _write_identity_fixture(root, producer_root="/producer/host", artifact="a" * 64)
    machine_file = root / f"provenance/metadata/target/meson.{machine}"
    machine_file.write_bytes(
        machine_file.read_bytes().replace(b"'clang'", b"'rogue-clang'", 1)
    )
    manifest_path = root / "extension_set_manifest.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    target_metadata = manifest["target_metadata"]
    target_metadata["digests"][f"meson_{machine}_sha256"] = hashlib.sha256(
        machine_file.read_bytes()
    ).hexdigest()
    target_identity = dict(target_metadata)
    target_identity.pop("digest")
    target_metadata["digest"] = hashlib.sha256(
        json.dumps(target_identity, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()
    (
        root / "provenance/metadata/target/source-extension-target-metadata.json"
    ).write_text(json.dumps(target_metadata), encoding="utf-8")
    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")

    with pytest.raises(ValueError, match=f"Meson {machine} file differs from bound"):
        _identity_from_root(root)


def test_recovery_reuses_observed_receipt_across_expected_identity_checks(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt.cli import source_extension_publication as publication

    receipt, identity = _stage_identity_fixture(
        tmp_path, label="incumbent", artifact="1" * 64
    )
    calls: list[Path] = []
    validate = publication.validate_source_extension_set_seal_contents

    def observed(root: Path) -> ValidatedSourceExtensionSetSeal:
        calls.append(root)
        return validate(root)

    monkeypatch.setattr(
        publication, "validate_source_extension_set_seal_contents", observed
    )
    verifier = publication._PublicationVerifier()
    assert not verifier.matches(receipt.seal.root, "f" * 64, "e" * 64)
    result = verifier.verified_at(
        receipt.seal.root, receipt.seal.seal_sha256, identity["canonical_sha256"]
    )
    assert result == receipt.seal
    assert calls == [receipt.seal.root]


@pytest.mark.parametrize(
    ("mutation", "error"),
    [
        ("outside-source", "producer source remapping"),
        ("missing-destination", "producer source remapping"),
        ("unsealed-destination", "sealed inventory"),
        ("wrong-digest", "sealed inventory"),
        ("uppercase-digest", "canonical lowercase"),
        ("duplicate", "duplicates support file"),
        ("unknown-field", "exactly path and sha256"),
        ("string-entry", "exactly path and sha256"),
        ("null", "must be a list"),
        ("parent-path", "canonical portable"),
    ],
)
def test_sealed_support_receipt_requires_exact_inventory_members(
    tmp_path: Path,
    mutation: str,
    error: str,
) -> None:
    root = tmp_path / "support-custody"
    _write_identity_fixture(root, producer_root="/producer/host", artifact="a" * 64)
    support_bytes = (root / "pkg/__init__.py").read_bytes()
    support_sha256 = hashlib.sha256(support_bytes).hexdigest()
    (tmp_path / "outside.py").write_bytes(support_bytes)
    entry: dict[str, Any] = {"path": "pkg/__init__.py", "sha256": support_sha256}
    entries: Any = [entry]
    if mutation == "outside-source":
        entry.update(path="pkg/alias.py", source="../../../../../../outside.py")
    elif mutation == "missing-destination":
        entry.update(path="pkg/alias.py", source="pkg/__init__.py")
    elif mutation == "unsealed-destination":
        entry["path"] = "pkg/alias.py"
    elif mutation == "wrong-digest":
        entry["sha256"] = "0" * 64
    elif mutation == "uppercase-digest":
        entry["sha256"] = support_sha256.upper()
    elif mutation == "duplicate":
        entries.append(dict(entry))
    elif mutation == "unknown-field":
        entry["extra"] = "ignored"
    elif mutation == "string-entry":
        entries = ["pkg/__init__.py"]
    elif mutation == "null":
        entries = None
    elif mutation == "parent-path":
        entry["path"] = "../outside.py"
    sidecar = root / "pkg/_native.molt.wasm.extension_manifest.json"
    manifest = json.loads(sidecar.read_text(encoding="utf-8"))
    manifest["support_files"] = entries
    sidecar.write_text(json.dumps(manifest), encoding="utf-8")
    with pytest.raises(ValueError, match=error):
        _identity_from_root(root)


def test_sealed_execution_metadata_is_inventory_only_and_immutable(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from dataclasses import FrozenInstanceError
    from molt.cli.source_extension_set_identity import (
        validate_source_extension_execution_metadata,
    )

    expected = hashlib.sha256(b"sealed support").hexdigest()
    manifest = {
        "module": "pkg._native",
        "support_files": [{"path": "pkg/support.py", "sha256": expected}],
    }
    inventory = {"pkg/support.py": expected}

    def forbid_io(*_args: Any, **_kwargs: Any) -> Any:
        raise AssertionError("sealed support metadata performed filesystem IO")

    with monkeypatch.context() as isolated:
        isolated.setattr(Path, "open", forbid_io)
        facts = validate_source_extension_execution_metadata(
            manifest, inventory_sha256=inventory
        )
    assert facts.support_files[0].digest_payload() == {
        "path": "pkg/support.py",
        "sha256": expected,
    }
    inventory["pkg/support.py"] = "0" * 64
    manifest["support_files"][0]["sha256"] = "0" * 64
    assert facts.support_files[0].sha256 == expected
    with pytest.raises(FrozenInstanceError):
        setattr(facts.support_files[0], "sha256", "0" * 64)


def test_structural_receipt_rejects_missing_direct_callable_symbol(
    tmp_path: Path,
) -> None:
    root = tmp_path / "missing-callable"
    _write_identity_fixture(root, producer_root="/producer/host", artifact="a" * 64)
    sidecar = root / "pkg/_native.molt.wasm.extension_manifest.json"
    manifest = json.loads(sidecar.read_text(encoding="utf-8"))
    manifest["callable_exports"] = [
        {
            "module": "pkg._native",
            "name": "invoke",
            "binding": "direct_symbol",
            "symbol": "molt_missing_direct_callable",
            "abi": "molt.object_call_v1",
            "arity": 1,
            "effects": [],
            "deterministic": True,
        }
    ]
    sidecar.write_text(json.dumps(manifest), encoding="utf-8")
    with pytest.raises(ValueError, match="direct_symbol callable export.*absent"):
        _identity_from_root(root)


def test_structural_receipt_rejects_artifact_aba_during_inspection(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt.cli import source_extension_set_validation_sidecars as sidecars

    root = tmp_path / "artifact-aba"
    _write_identity_fixture(root, producer_root="/producer/host", artifact="a" * 64)
    inspect = sidecars.validate_source_extension_artifact_object_closure

    def mutate_and_restore(**kwargs: Any) -> list[str]:
        errors = inspect(**kwargs)
        path = kwargs["artifact_path"]
        before = path.read_bytes()
        try:
            path.write_bytes(before + b"temporary mutation")
        finally:
            path.write_bytes(before)
        return errors

    monkeypatch.setattr(
        sidecars,
        "validate_source_extension_artifact_object_closure",
        mutate_and_restore,
    )
    with pytest.raises(ValueError, match="changed|stable|modified"):
        _identity_from_root(root)
