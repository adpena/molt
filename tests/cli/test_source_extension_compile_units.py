from __future__ import annotations

import hashlib
import json
from dataclasses import replace
from pathlib import Path

import pytest

from molt.cli import source_extensions as extensions
from molt.cli.extension_scan_surface import _ExtensionScanSurface
from molt.cli.source_extension_language import SourceExtensionLanguage


def _command(source: Path, output: Path, *flags: str) -> dict:
    return {
        "directory": str(output.parent.parent),
        "file": str(source),
        "output": str(output),
        "arguments": ["clang", *flags, "-c", str(source), "-o", str(output)],
    }


def test_compile_database_identity_is_output_not_source(tmp_path: Path) -> None:
    source = tmp_path / "dispatch.c"
    roots = tmp_path / "owned.p"
    first, second, same_flags = roots / "v3.o", roots / "v4.o", roots / "v3-other.o"
    rows = [
        _command(source, first, "-DVARIANT=3"),
        _command(source, second, "-DVARIANT=4"),
    ]
    rows.append(_command(source, same_flags, "-DVARIANT=3"))
    rows += [
        rows[0],
        _command(source, tmp_path / "unrelated.p" / "other.o", "-DVARIANT=5"),
    ]
    database = tmp_path / "compile_commands.json"
    database.write_text(json.dumps(rows), encoding="utf-8")

    units, errors = extensions._load_compile_command_units(
        database, required_sources={source}, target_output_roots=(roots,)
    )

    assert errors == []
    assert units is not None
    assert set(units) == {first, second, same_flags}
    assert units[first].compile_args == ("-DVARIANT=3",)
    assert units[second].compile_args == ("-DVARIANT=4",)
    assert units[same_flags].compile_args == units[first].compile_args
    assert units[first].source_path == units[second].source_path == source


@pytest.mark.parametrize("forwarder", ["-mllvm", "-Xassembler", "-Xpreprocessor"])
@pytest.mark.parametrize(
    "operand",
    [
        "-opt-bisect-limit=0",
        "-Iopaque",
        "-xc++",
        "--target=foreign",
        "-c",
        "--driver-mode=cl",
    ],
)
@pytest.mark.parametrize("clang_cl", [False, True])
def test_forwarded_operands_survive_the_complete_compile_unit_pipeline(
    tmp_path, forwarder, operand, clang_cl
):
    source = tmp_path / "dispatch.c"
    output = tmp_path / "real.o"
    transport = (lambda value: "/clang:" + value) if clang_cl else (lambda value: value)
    opaque = (transport(forwarder), transport(operand))
    command = [
        "clang-cl" if clang_cl else "clang",
        "-c",
        str(source),
        "-o",
        str(output),
        *opaque,
    ]
    assert (
        extensions._compile_command_output_path(command, directory=tmp_path) == output
    )
    semantic = extensions._compile_command_semantic_args(
        command, source_path=source, directory=tmp_path
    )
    assert tuple(semantic) == opaque
    args, includes = extensions._compile_command_args_and_include_dirs(
        semantic, directory=tmp_path
    )
    assert args == opaque and includes == ()
    assert extensions._source_extension_replay_compile_args(
        args,
        compiler_target="x86_64-pc-windows-msvc"
        if clang_cl
        else "x86_64-unknown-linux-gnu",
        compiler_command=(command[0],),
    ) == list(opaque)


@pytest.mark.parametrize(
    "option",
    ["-object-file-name=module.o", "-objcmt-migrate-literals", "-offload-arch=gfx900"],
)
def test_long_driver_options_are_not_joined_object_outputs(tmp_path, option):
    source = tmp_path / "dispatch.c"
    output = tmp_path / "real.o"
    command = ["clang", "-c", str(source), "-o", str(output), option]
    assert (
        extensions._compile_command_output_path(command, directory=tmp_path) == output
    )
    assert extensions._compile_command_semantic_args(
        command, source_path=source, directory=tmp_path
    ) == [option]
    assert extensions._source_extension_replay_compile_args(
        (option,), compiler_target="x86_64-unknown-linux-gnu"
    ) == [option]


def test_compile_database_output_identity_ignores_llvm_output_like_operand(tmp_path):
    source, output = tmp_path / "dispatch.c", tmp_path / "real.o"
    row = _command(source, output)
    row["arguments"].extend(("-mllvm", "-opt-bisect-limit=0"))
    database = tmp_path / "compile_commands.json"
    database.write_text(json.dumps([row]), encoding="utf-8")
    units, errors = extensions._load_compile_command_units(database)
    assert errors == [] and units is not None
    assert set(units) == {output}
    assert units[output].compile_args == ("-mllvm", "-opt-bisect-limit=0")


@pytest.mark.parametrize(
    "option",
    [
        "-D",
        "-U",
        "-I",
        "-include",
        "-isystem",
        "-MF",
        "-MT",
        "-mllvm",
        "-Xassembler",
        "-Xpreprocessor",
        "-Xclang",
        "-x",
        "-o",
        "/Fo",
        "/Tc",
    ],
)
def test_missing_operand_is_rejected_by_the_shared_compile_grammar(tmp_path, option):
    source = tmp_path / "dispatch.c"
    with pytest.raises(ValueError, match="operand|language"):
        extensions._compile_command_output_path(
            ["clang", "-c", str(source), option], directory=tmp_path
        )


@pytest.mark.parametrize("option", ["-D", "-U", "-include", "-isystem"])
def test_driver_operand_equal_to_source_is_not_removed(tmp_path, option):
    source = tmp_path / "dispatch.c"
    args = extensions._compile_command_semantic_args(
        ["clang", option, str(source), "-c", str(source), "-o", "real.o"],
        source_path=source,
        directory=tmp_path,
    )
    assert args == [option, str(source)]


@pytest.mark.parametrize(
    "operand", ["-o", "-xc++", "-Iunowned", "-include", "-load", "-emit-llvm"]
)
def test_unmodeled_cc1_custody_fails_closed(tmp_path, operand):
    with pytest.raises(
        ValueError, match="unsupported frontend input/output/language custody"
    ):
        extensions._compile_command_output_path(
            ["clang", "-c", "unit.c", "-o", "real.o", "-Xclang", operand],
            directory=tmp_path,
        )


@pytest.mark.parametrize(
    "conflict", ["flags", "source", "declared_output", "missing_output"]
)
def test_compile_database_rejects_ambiguous_object_identity(
    tmp_path: Path, conflict: str
) -> None:
    source = tmp_path / "dispatch.c"
    output = tmp_path / "owned.p" / "dispatch.o"
    row = _command(source, output, "-DVARIANT=3")
    conflicting = _command(source, output, "-DVARIANT=4")
    if conflict == "source":
        conflicting = _command(tmp_path / "different.c", output, "-DVARIANT=3")
    elif conflict == "declared_output":
        conflicting["output"] = str(output.with_name("different.o"))
    elif conflict == "missing_output":
        conflicting.pop("output")
        conflicting["arguments"] = conflicting["arguments"][:-2]
    database = tmp_path / "compile_commands.json"
    database.write_text(json.dumps([row, conflicting]), encoding="utf-8")

    units, errors = extensions._load_compile_command_units(
        database, required_sources={source}, target_output_roots=(output.parent,)
    )

    assert units is None
    assert len(errors) == 1
    expected = {
        "flags": "conflicting commands for object",
        "source": "conflicting commands for object",
        "declared_output": "output disagrees",
        "missing_output": "has no object output",
    }
    assert expected[conflict] in errors[0]


def _meson_variants(tmp_path: Path, *, omit_v3: bool = False):
    source = tmp_path / "dispatch.c"
    source.write_text("int dispatch;\n", encoding="utf-8")
    build = tmp_path / "build"
    build.mkdir()
    group = {"language": "c", "sources": [str(source)]}
    primary = {
        "id": "extension",
        "name": "extension",
        "type": "shared module",
        "filename": [str(build / "extension.so")],
        "target_sources": [group],
        "linker_parameters": ["/WHOLEARCHIVE:libv3.a", "libv4.a"],
    }
    targets = [primary]
    rows = [_command(source, build / "extension.so.p" / "base.o", "-DVARIANT=0")]
    for variant in (3, 4, 5):
        targets.append(
            {
                "id": f"variant-{variant}",
                "name": f"variant-{variant}",
                "type": "static library",
                "filename": [str(build / f"libv{variant}.a")],
                "target_sources": [group],
            }
        )
        if not (omit_v3 and variant == 3):
            rows.append(
                _command(
                    source,
                    build / f"libv{variant}.a.p" / "unit.o",
                    f"-DVARIANT={variant}",
                )
            )
    plan_path = build / "intro-targets.json"
    plan_path.write_text(json.dumps(targets), encoding="utf-8")
    (build / "compile_commands.json").write_text(json.dumps(rows), encoding="utf-8")
    return extensions._load_meson_intro_targets_source_extension_plan(
        plan_path=plan_path,
        project_root=tmp_path,
        module_name="extension",
        source_root=tmp_path,
        build_root=build,
    )


@pytest.mark.parametrize(
    "compiler, first, last",
    [
        ("clang", ["-o", "first.o"], ["-o", "last.o"]),
        ("clang-cl", ["/Fofirst.o"], ["/Fo", "last.o"]),
        ("clang-cl", ["/Fofirst.o"], ["/clang:-o", "/clang:last.o"]),
    ],
)
def test_compile_database_attests_effective_output_selector(
    tmp_path, compiler, first, last
):
    source = tmp_path / "dispatch.c"
    row = {
        "directory": str(tmp_path),
        "file": str(source),
        "output": "last.o",
        "arguments": [compiler, "-c", str(source), *first, *last],
    }
    database = tmp_path / "compile_commands.json"
    database.write_text(json.dumps([row]), encoding="utf-8")
    units, errors = extensions._load_compile_command_units(database)
    assert errors == []
    assert units is not None and set(units) == {tmp_path / "last.o"}
    row["output"] = "first.o"
    database.write_text(json.dumps([row]), encoding="utf-8")
    units, errors = extensions._load_compile_command_units(database)
    assert units is None
    assert len(errors) == 1 and "output disagrees" in errors[0]


def test_meson_units_preserve_target_scoped_forced_membership(tmp_path: Path) -> None:
    plan, errors = _meson_variants(tmp_path)
    assert errors == []
    assert plan is not None
    units = {unit.owner_target_id: unit for unit in plan.compile_units}
    assert set(units) == {"extension", "variant-3", "variant-4"}
    assert [unit.force_include for unit in units.values()] == [False, True, False]
    assert [unit.compile_args for unit in units.values()] == [
        ("-DVARIANT=0",),
        ("-DVARIANT=3",),
        ("-DVARIANT=4",),
    ]
    assert len(set(unit.producer_object_path for unit in units.values())) == 3
    assert len(set(unit.source_path for unit in units.values())) == 1
    assert (
        units["variant-3"].manifest_payload(build_root=plan.build_root)[
            "producer_object_path"
        ]
        == "libv3.a.p/unit.o"
    )


def test_meson_does_not_borrow_a_sibling_target_command(tmp_path: Path) -> None:
    plan, errors = _meson_variants(tmp_path, omit_v3=True)
    assert plan is None
    assert len(errors) == 1
    assert "no owned entry" in errors[0]
    assert "variant-3" in errors[0]


def _fact(source: Path, output: Path, *, undefined=(), defined=(), dependencies=()):
    return extensions._SourceExtensionObjectFact(
        source_path=source,
        language=SourceExtensionLanguage.C,
        object_path=output,
        source_sha256=hashlib.sha256(source.read_bytes()).hexdigest(),
        object_sha256="b" * 64,
        defined_symbols=tuple(defined),
        undefined_symbols=tuple(undefined),
        defined_function_symbols=tuple(defined),
        compile_command=(),
        symbol_authority="native_nm_command_v1",
        symbol_command=("llvm-nm",),
        dependencies=tuple(
            extensions._SourceExtensionDependencyFact(
                path=path, sha256=hashlib.sha256(path.read_bytes()).hexdigest()
            )
            for path in dependencies
        ),
    )


def test_object_receipt_canonicalization_preserves_opaque_payload_bytes(
    tmp_path, monkeypatch
):
    source, output = tmp_path / "unit.c", tmp_path / "unit.o"
    source.write_bytes(b"int unit;")
    output.write_bytes(b"object")
    inspection = extensions._SourceExtensionArtifactSymbolInspection(
        defined_symbols=frozenset({"unit"}),
        undefined_symbols=frozenset(),
        defined_function_symbols=frozenset(),
        symbol_authority="native_nm_command_v1",
        wasm_imports=None,
        wasm_function_import_signatures=(),
        artifact_digest=hashlib.sha256(output.read_bytes()).hexdigest(),
    )
    monkeypatch.setattr(
        extensions,
        "_inspect_source_extension_artifact_symbols",
        lambda *_args, **_kwargs: inspection,
    )
    opaque = ("-mllvm", "-opt-record-file=" + str(output), "-Xassembler", str(output))
    fact, error = extensions._source_extension_object_fact(
        source_path=source,
        object_path=output,
        language=SourceExtensionLanguage.C,
        compile_command=(
            "clang",
            *opaque,
            "-x",
            "c",
            "-c",
            str(source),
            "-o",
            str(output),
        ),
    )
    assert error is None and fact is not None
    assert fact.compile_command[1:5] == opaque
    assert fact.compile_command[-2:] == ("-o", "@object-root/unit.o")


def test_c_api_requirements_follow_compiled_objects_not_shared_text(
    tmp_path: Path,
) -> None:
    source = tmp_path / "dispatch.c"
    source.write_text(
        "#define PyLong_FromLong ignored_by_compiled_authority\n"
        "#if 0\nvoid ignored(void) { PyCode_NewWithPosOnlyArgs(); }\n#endif\n",
        encoding="utf-8",
    )
    first = _fact(
        source,
        tmp_path / "v3.o",
        undefined=("PyLong_FromLong", "ordinary_external"),
        defined=("PyInit_example", "PyUnitProvided"),
    )
    second = _fact(
        source, tmp_path / "v4.o", undefined=("PyTuple_New", "PyUnitProvided")
    )
    requirements, error = extensions._source_extension_required_c_api_by_object(
        molt_root=Path(__file__).resolve().parents[2],
        object_facts=(first, second),
    )
    assert error is None and requirements is not None
    assert requirements.required_by_object == {
        first.object_path: ("PyLong_FromLong",),
        second.object_path: ("PyTuple_New",),
    }
    assert requirements.missing_symbols == requirements.fail_fast_symbols == ()
    assert requirements.project_defined_symbols == ("PyInit_example", "PyUnitProvided")
    closure = extensions._SourceExtensionObjectClosure(
        init_symbol="PyInit_example",
        init_symbol_owner=first,
        objects=(first, second),
        undefined_symbols=("PyLong_FromLong", "PyTuple_New", "ordinary_external"),
    )
    payload = closure.manifest_payload(
        required_c_api_by_object=requirements.required_by_object
    )
    assert [item["required_c_api_symbols"] for item in payload["objects"]] == [
        ["PyLong_FromLong"],
        ["PyTuple_New"],
    ]


def test_object_metadata_uses_only_owned_dependencies_and_shared_bytes(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    source = tmp_path / "dispatch.c"
    source.write_text("/* requirements come from compiled facts */\n", encoding="utf-8")
    first_header, second_header = tmp_path / "v3.h", tmp_path / "v4.h"
    first_header.write_text(
        "#define MAKE(n) PyVariant_ ## n\nimport_array();\n", encoding="utf-8"
    )
    second_header.write_text("PyUFunc_ImportUFuncAPI();\n", encoding="utf-8")
    first = _fact(
        source,
        tmp_path / "v3.o",
        undefined=("PyVariant_Value", "PyUnitRuntime", "PyUnitFailFast"),
        dependencies=(first_header,),
    )
    second = _fact(
        source,
        tmp_path / "v4.o",
        undefined=("PyVariant_Value", "PyUnitMissing", "PyUnitCompileOnly"),
        dependencies=(second_header,),
    )
    surface = _ExtensionScanSurface(
        runtime_backed=frozenset({"PyUnitRuntime"}),
        source_compile_only=frozenset({"PyUnitCompileOnly"}),
        fail_fast=frozenset({"PyUnitFailFast"}),
        header_path=tmp_path / "Python.h",
    )
    monkeypatch.setattr(
        extensions,
        "_load_c_api_scan_surface",
        lambda *_args, **_kwargs: (surface, surface.header_path, None),
    )
    reads: dict[Path, int] = {}
    read_bytes = Path.read_bytes

    def record_read(path: Path) -> bytes:
        reads[path] = reads.get(path, 0) + 1
        return read_bytes(path)

    monkeypatch.setattr(Path, "read_bytes", record_read)
    requirements, error = extensions._source_extension_required_c_api_by_object(
        molt_root=tmp_path,
        object_facts=(first, second),
    )
    assert error is None and requirements is not None
    assert reads == {source: 1, first_header: 1, second_header: 1}
    assert requirements.required_by_object == {
        first.object_path: ("PyUnitFailFast", "PyUnitRuntime"),
        second.object_path: ("PyUnitCompileOnly", "PyUnitMissing", "PyVariant_Value"),
    }
    assert requirements.project_generated_c_api_by_object == {
        first.object_path: ("PyVariant_Value",),
        second.object_path: (),
    }
    assert requirements.project_generated_c_api_prefixes == ("PyVariant_",)
    assert requirements.required_capsules_by_object == {
        first.object_path: ("numpy.core._multiarray_umath._ARRAY_API",),
        second.object_path: ("numpy.core._multiarray_umath._UFUNC_API",),
    }
    assert requirements.missing_symbols == (
        "PyUnitCompileOnly",
        "PyUnitMissing",
        "PyVariant_Value",
    )
    assert requirements.fail_fast_symbols == ("PyUnitFailFast",)


def test_object_requirement_scan_rejects_duplicate_objects(tmp_path: Path) -> None:
    source = tmp_path / "dispatch.c"
    source.write_text("int dispatch;\n", encoding="utf-8")
    fact = _fact(source, tmp_path / "unit.o")
    requirements, error = extensions._source_extension_required_c_api_by_object(
        molt_root=Path(__file__).resolve().parents[2],
        object_facts=(fact, fact),
    )
    assert requirements is None
    assert error is not None and "duplicate object" in error


@pytest.mark.parametrize("changed_input", ["source", "dependency"])
def test_object_metadata_scan_checks_each_compiled_input_reference(
    tmp_path: Path,
    changed_input: str,
) -> None:
    source, dependency = tmp_path / "dispatch.c", tmp_path / "config.h"
    source.write_text("int dispatch;\n", encoding="utf-8")
    dependency.write_text("import_array();\n", encoding="utf-8")
    first = _fact(source, tmp_path / "v3.o", dependencies=(dependency,))
    if changed_input == "source":
        second = replace(first, object_path=tmp_path / "v4.o", source_sha256="f" * 64)
    else:
        second = replace(
            first,
            object_path=tmp_path / "v4.o",
            dependencies=(replace(first.dependencies[0], sha256="f" * 64),),
        )
    requirements, error = extensions._source_extension_required_c_api_by_object(
        molt_root=Path(__file__).resolve().parents[2],
        object_facts=(first, second),
    )
    assert requirements is None
    assert error is not None and "checksum differs from compiled custody" in error
