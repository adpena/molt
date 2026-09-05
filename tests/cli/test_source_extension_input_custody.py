from __future__ import annotations

from copy import deepcopy
from pathlib import Path
from typing import Any

import pytest

from molt.cli import source_extension_input_custody as custody
from molt.cli.source_extension_input_custody import (
    SourceExtensionInputCustodyError,
    rewrite_source_extension_manifest_input_references,
    source_extension_input_custody_manifest,
    source_extension_input_custody_path,
    source_extension_manifest_input_rows,
    stage_source_extension_manifest_inputs,
    validate_source_extension_manifest_input_custody,
)
from molt.cli.source_extension_manifest_codec import _canonical_sequence_digest
from molt.cli.source_extensions import (
    canonicalize_source_extension_manifest_required_capsules,
    canonicalize_source_extension_manifest_runtime_python_imports,
    source_extension_manifest_required_capsule_imports_by_source,
    source_extension_manifest_runtime_python_imports,
)
from molt.file_hashing import _sha256_bytes


def _manifest(first: Path, second: Path, dependency: Path) -> dict[str, object]:
    source_digest = _sha256_bytes(first.read_bytes())
    dependency_digest = _sha256_bytes(dependency.read_bytes())
    return {
        "sources": [str(first), str(second)],
        "object_closure": {
            "objects": [
                {
                    "source": str(first),
                    "language": "c",
                    "source_sha256": source_digest,
                    "dependencies": [
                        {"path": str(dependency), "sha256": dependency_digest}
                    ],
                },
                {
                    "source": str(second),
                    "language": "c",
                    "source_sha256": source_digest,
                    "dependencies": [],
                },
            ]
        },
    }


def test_stage_and_rewrite_share_content_addresses_across_roles(tmp_path: Path) -> None:
    first = tmp_path / "checkout-a" / "source.c"
    second = tmp_path / "checkout-b" / "renamed.c"
    dependency = tmp_path / "checkout-a" / "header.h"
    for path, value in (
        (first, b"int source(void);\n"),
        (second, b"int source(void);\n"),
        (dependency, b"#define VALUE 1\n"),
    ):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(value)
    manifest = _manifest(first, second, dependency)
    source_manifest_path = tmp_path / "build" / "extension_manifest.json"
    output_manifest_path = (
        tmp_path / "sealed" / "pkg" / "module.molt.wasm.extension_manifest.json"
    )

    staged = stage_source_extension_manifest_inputs(
        manifest,
        manifest_path=source_manifest_path,
        publish_root=tmp_path / "sealed",
    )

    source_digest = _sha256_bytes(first.read_bytes())
    dependency_digest = _sha256_bytes(dependency.read_bytes())
    assert staged[first.resolve()] == source_extension_input_custody_path(source_digest)
    assert staged[second.resolve()] == source_extension_input_custody_path(
        source_digest
    )
    assert staged[dependency.resolve()] == source_extension_input_custody_path(
        dependency_digest
    )
    custody_files = [
        path
        for path in (tmp_path / "sealed" / "provenance").rglob("*")
        if path.is_file()
    ]
    assert len(custody_files) == 2

    rewritten = deepcopy(manifest)
    rewrite_source_extension_manifest_input_references(
        rewritten,
        source_manifest_path=source_manifest_path,
        output_manifest_path=output_manifest_path,
        publish_root=tmp_path / "sealed",
        staged_inputs=staged,
    )
    closure = rewritten["object_closure"]
    assert isinstance(closure, dict)
    objects = closure["objects"]
    assert isinstance(objects, list)
    rewritten["sources"] = [item["source"] for item in objects]
    rewritten["input_custody"] = source_extension_input_custody_manifest()
    validate_source_extension_manifest_input_custody(rewritten)
    assert objects[0]["source"] == objects[1]["source"]
    assert objects[0]["source"].endswith(source_digest)


def test_input_projection_merges_byte_identical_header_aliases(tmp_path: Path) -> None:
    source = tmp_path / "source.c"
    first_header = tmp_path / "first.h"
    second_header = tmp_path / "second.h"
    source.write_bytes(b"int source;\n")
    first_header.write_bytes(b"#define HEADER 1\n")
    second_header.write_bytes(first_header.read_bytes())
    manifest = _manifest(source, source, first_header)
    digest = _sha256_bytes(first_header.read_bytes())
    manifest["object_closure"]["objects"][0]["dependencies"].append(
        {"path": str(second_header), "sha256": digest}
    )
    manifest_path = tmp_path / "extension_manifest.json"
    staged = stage_source_extension_manifest_inputs(
        manifest, manifest_path=manifest_path, publish_root=tmp_path / "sealed"
    )
    rewrite_source_extension_manifest_input_references(
        manifest,
        source_manifest_path=manifest_path,
        output_manifest_path=tmp_path / "sealed" / "extension_manifest.json",
        publish_root=tmp_path / "sealed",
        staged_inputs=staged,
    )
    assert manifest["object_closure"]["objects"][0]["dependencies"] == [
        {
            "path": source_extension_input_custody_path(digest).as_posix(),
            "sha256": digest,
        }
    ]


def test_stage_rejects_corrupt_existing_content_address(tmp_path: Path) -> None:
    source = tmp_path / "source.c"
    dependency = tmp_path / "header.h"
    source.write_bytes(b"int source(void);\n")
    dependency.write_bytes(b"#define VALUE 1\n")
    manifest = _manifest(source, source, dependency)
    digest = _sha256_bytes(source.read_bytes())
    destination = tmp_path / "sealed" / source_extension_input_custody_path(digest)
    destination.parent.mkdir(parents=True)
    destination.write_bytes(b"tampered")

    with pytest.raises(
        SourceExtensionInputCustodyError,
        match="content-addressed source-extension input collision",
    ):
        stage_source_extension_manifest_inputs(
            manifest,
            manifest_path=tmp_path / "extension_manifest.json",
            publish_root=tmp_path / "sealed",
        )


def test_custody_validator_rejects_path_and_digest_drift(tmp_path: Path) -> None:
    source = tmp_path / "source.c"
    dependency = tmp_path / "header.h"
    source.write_bytes(b"int source(void);\n")
    dependency.write_bytes(b"#define VALUE 1\n")
    manifest = _manifest(source, source, dependency)
    staged = stage_source_extension_manifest_inputs(
        manifest,
        manifest_path=tmp_path / "extension_manifest.json",
        publish_root=tmp_path / "sealed",
    )
    rewrite_source_extension_manifest_input_references(
        manifest,
        source_manifest_path=tmp_path / "extension_manifest.json",
        output_manifest_path=tmp_path / "sealed" / "extension_manifest.json",
        publish_root=tmp_path / "sealed",
        staged_inputs=staged,
    )
    closure = manifest["object_closure"]
    assert isinstance(closure, dict)
    objects = closure["objects"]
    assert isinstance(objects, list)
    manifest["sources"] = [item["source"] for item in objects]
    manifest["input_custody"] = source_extension_input_custody_manifest()
    objects[0]["source"] += "/source.c"
    manifest["sources"] = [item["source"] for item in objects]

    with pytest.raises(
        SourceExtensionInputCustodyError,
        match="custody path does not match its digest",
    ):
        validate_source_extension_manifest_input_custody(manifest)


@pytest.mark.parametrize(
    "closure",
    [
        None,
        {},
        {"objects": []},
        {"objects": [None]},
        {"objects": [{}]},
        {"objects": [{"source": "source.c", "source_sha256": "invalid"}]},
        {"objects": [{"source": " ", "source_sha256": "0" * 64}]},
    ],
)
def test_runtime_import_scan_rejects_invalid_closure_without_erasing_facts(
    tmp_path: Path, closure: object
) -> None:
    source = tmp_path / "source.c"
    source.write_text("int source(void);\n", encoding="utf-8")
    manifest: dict[str, Any] = {
        "object_closure": closure,
        "sources": [str(source)],
        "runtime_python_import_modules": ["retained"],
    }
    before = deepcopy(manifest)
    with pytest.raises(SourceExtensionInputCustodyError):
        source_extension_manifest_input_rows(manifest)
    errors = canonicalize_source_extension_manifest_runtime_python_imports(
        manifest, manifest_path=tmp_path / "extension_manifest.json"
    )
    assert errors
    assert manifest == before


def test_compact_dependency_imports_share_validated_input_authority(
    tmp_path: Path,
) -> None:
    source = tmp_path / "source.c"
    header = tmp_path / "header.h"
    source.write_text("int source(void);\n", encoding="utf-8")
    header.write_text(
        'int module_exec(void) { PyImport_ImportModule("decimal"); return 0; }\n',
        encoding="utf-8",
    )
    expanded: dict[str, Any] = _manifest(source, source, header)
    compact = deepcopy(expanded)
    dependency = compact["object_closure"]["objects"][0].pop("dependencies")[0]
    values = [dependency["path"], dependency["sha256"]]
    strings = sorted(values)
    reference = _canonical_sequence_digest(values)
    compact["object_closure"]["objects"][0]["dependencies_ref"] = reference
    compact["build_authorities"] = {
        "schema_version": 1,
        "strings": strings,
        "sequences": {reference: [strings.index(value) for value in values]},
    }
    assert source_extension_manifest_input_rows(compact) == (
        source_extension_manifest_input_rows(expanded)
    )
    manifest_path = tmp_path / "extension_manifest.json"
    assert source_extension_manifest_runtime_python_imports(
        compact, manifest_path=manifest_path
    ) == (("decimal",), [])
    compact["runtime_python_import_modules"] = ["retained"]
    before = deepcopy(compact)
    header.unlink()
    assert canonicalize_source_extension_manifest_runtime_python_imports(
        compact, manifest_path=manifest_path
    )
    assert compact == before


def test_reference_rewrite_failure_does_not_partially_mutate_manifest(
    tmp_path: Path,
) -> None:
    source = tmp_path / "source.c"
    header = tmp_path / "header.h"
    source.write_bytes(b"int source(void);\n")
    header.write_bytes(b"#define VALUE 1\n")
    manifest = _manifest(source, source, header)
    before = deepcopy(manifest)
    with pytest.raises(SourceExtensionInputCustodyError, match="mapping is incomplete"):
        rewrite_source_extension_manifest_input_references(
            manifest,
            source_manifest_path=tmp_path / "extension_manifest.json",
            output_manifest_path=tmp_path / "sealed" / "extension_manifest.json",
            publish_root=tmp_path / "sealed",
            staged_inputs={
                source.resolve(): source_extension_input_custody_path(
                    _sha256_bytes(source.read_bytes())
                )
            },
        )
    assert manifest == before


def test_scan_checks_the_bytes_it_consumes_once_per_input(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "source.c"
    header = tmp_path / "header.h"
    original = (
        b'int module_exec(void) { PyImport_ImportModule("decimal"); return 0; }\n'
    )
    source.write_bytes(original)
    header.write_bytes(b"#define VALUE 1\n")
    manifest = _manifest(source, source, header)
    read_bytes = Path.read_bytes
    reads: dict[Path, int] = {}

    def changing_read(path: Path) -> bytes:
        reads[path] = reads.get(path, 0) + 1
        content = read_bytes(path)
        if path == source:
            source.write_bytes(b"int module_exec(void) { return 0; }\n")
        return content

    monkeypatch.setattr(Path, "read_bytes", changing_read)
    assert source_extension_manifest_runtime_python_imports(
        manifest, manifest_path=tmp_path / "extension_manifest.json"
    ) == (("decimal",), [])
    assert reads[source] == 1
    assert reads[header] == 1


def test_staging_change_does_not_publish_a_poisoned_content_address(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "source.c"
    header = tmp_path / "header.h"
    source.write_bytes(b"int source(void);\n")
    header.write_bytes(b"#define VALUE 1\n")
    manifest = _manifest(source, source, header)
    destination = (
        tmp_path
        / "sealed"
        / source_extension_input_custody_path(_sha256_bytes(source.read_bytes()))
    )
    atomic_copy = custody._atomic_copy_file

    def changed_copy(src: Path, dst: Path, *, expected_sha256: str) -> None:
        src.write_bytes(b"changed after source resolution\n")
        atomic_copy(src, dst, expected_sha256=expected_sha256)

    monkeypatch.setattr(custody, "_atomic_copy_file", changed_copy)
    with pytest.raises(SourceExtensionInputCustodyError, match="changed while staging"):
        stage_source_extension_manifest_inputs(
            manifest,
            manifest_path=tmp_path / "extension_manifest.json",
            publish_root=tmp_path / "sealed",
        )
    assert not destination.exists()


def test_capsule_scan_retains_missing_diagnostics_and_projects_headers(
    tmp_path: Path,
) -> None:
    source = tmp_path / "source.c"
    header = tmp_path / "header.h"
    source.write_text("int source(void);\n", encoding="utf-8")
    header.write_text(
        "int module_exec(void) { import_array(); return 0; }\n", encoding="utf-8"
    )
    manifest: dict[str, Any] = _manifest(source, source, header)
    manifest_path = tmp_path / "extension_manifest.json"
    assert (
        canonicalize_source_extension_manifest_required_capsules(
            manifest, manifest_path=manifest_path
        )
        == []
    )
    capsule = "numpy.core._multiarray_umath._ARRAY_API"
    assert manifest["object_closure"]["objects"][0]["required_capsules"] == [capsule]
    header.unlink()
    before = deepcopy(manifest)
    assert canonicalize_source_extension_manifest_required_capsules(
        manifest, manifest_path=manifest_path
    )
    assert manifest == before
    scanned, errors = source_extension_manifest_required_capsule_imports_by_source(
        manifest, manifest_path=manifest_path, allow_missing_sources=True
    )
    assert scanned == {}
    assert errors and all("source missing:" in error for error in errors)
