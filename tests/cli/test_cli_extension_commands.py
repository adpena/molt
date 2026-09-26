from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import zipfile
from pathlib import Path
from typing import Any

from molt.cli import wasm_link_inputs
import molt.cli as cli
import molt.wasm_artifact as wasm_artifact
from molt._wasm_runtime_exports import wasm_static_link_runtime_symbols_for_imports
from molt.cli import extension_commands as cli_commands
from molt.cli import entrypoint_parser as cli_entrypoint_parser
from molt.cli import llvm_wasi_tools as cli_llvm_wasi_tools
from molt.cli import source_extension_target as cli_source_extension_target
from molt.cli import source_extension_link_inputs as cli_source_extension_link_inputs
from molt.cli import source_extensions as cli_source_extensions
from molt.source_extension_link_inputs import SourceExtensionLinkInputs
from molt.cli.extension_manifest import (
    _CURRENT_MOLT_C_API_VERSION,
    _default_molt_c_api_version,
    _manifest_support_file_payloads,
)
from molt.cli.source_extension_input_custody import (
    resolve_source_extension_manifest_input,
)
from molt.cli.source_extension_object_closure import (
    finalize_source_extension_object_closure,
    source_extension_object_closure_digest,
    source_extension_wasm_import_receipts,
)
from molt.cli.source_extension_object_closure_schema import (
    SOURCE_EXTENSION_OBJECT_CLOSURE_SCHEMA_VERSION,
    SOURCE_EXTENSION_WASM_SYMBOL_AUTHORITY,
)
from molt.cli.source_extension_manifest_codec import (
    _compact_source_extension_manifest,
    _manifest_sequence,
)
from molt.cli import source_extension_toolchain as cli_source_extension_toolchain
from molt.cli import wasm_toolchain as cli_wasm_toolchain
from molt.c_api_symbols import is_c_api_external_requirement
import pytest

from tests.cli.process_guard import run_cli_test_process
from tests.cli.native_link_test_support import static_archive_bytes
from tests.wasm_object_fixtures import (
    wasm_exporting_i64_unary_symbol as _wasm_exporting_i64_unary_symbol,
    wasm_exporting_i64_unary_symbols as _wasm_exporting_i64_unary_symbols,
)


ROOT = Path(__file__).resolve().parents[2]


def _source_extension_target_plan(
    requested: str,
) -> cli_source_extension_target.SourceExtensionTargetPlan:
    return cli_source_extension_target.resolve_source_extension_target_plan(
        requested,
        host_platform="linux",
        host_arch="x86_64",
    )


def _resolved_llvm_tool(
    role: cli_llvm_wasi_tools.LlvmToolRole,
    command: tuple[str, ...],
) -> cli_llvm_wasi_tools.ResolvedLlvmTool:
    return cli_llvm_wasi_tools.ResolvedLlvmTool(
        role=role,
        command=command,
        path=Path(command[0]),
        version="22.1.0",
        sha256="a" * 64,
    )


def _stub_metadata_build_machine(monkeypatch: pytest.MonkeyPatch):
    host_plan = _source_extension_target_plan("native")
    commands = {
        "c": ("/host/clang",),
        "cpp": ("/host/clang++",),
        "ar": ("/host/llvm-ar",),
        "nm": ("/host/llvm-nm",),
    }
    tools = cli_llvm_wasi_tools.LlvmWasiToolFamily(
        cc=_resolved_llvm_tool("cc", commands["c"]),
        cxx=_resolved_llvm_tool("cxx", commands["cpp"]),
        ar=_resolved_llvm_tool("ar", commands["ar"]),
        nm=_resolved_llvm_tool("nm", commands["nm"]),
        wasm_ld=None,
        ranlib=None,
        strip=None,
    )
    host = cli_source_extension_toolchain._ResolvedSourceExtensionToolchain(
        target_plan=host_plan,
        compiler_kind="host",
        tools=tools,
        commands=commands,
        wasi_sysroot=None,
        link_inputs=SourceExtensionLinkInputs(
            host_plan.target_triple, None, None, None
        ),
        detail="attested test build machine",
    )
    monkeypatch.setattr(
        cli_source_extension_toolchain,
        "resolve_source_extension_target_plan",
        lambda requested: (
            host_plan
            if requested == "native"
            else pytest.fail("unexpected host request")
        ),
    )

    def resolve(plan, *, environment):
        assert plan == host_plan
        assert not (
            {"CC", "CXX", "MOLT_CROSS_CC", "MOLT_CROSS_CXX", "MOLT_WASM_CC"}
            & environment.keys()
        )
        return host

    monkeypatch.setattr(
        cli_source_extension_toolchain,
        "_resolve_source_extension_native_toolchain",
        resolve,
    )
    return host


def _write_fake_compiler_depfile(cmd: list[str], *dependencies: Path) -> None:
    extra_inputs = "".join(f" {path}" for path in dependencies)
    forwarded_depfile = next(
        (arg.removeprefix("/clang:-MF") for arg in cmd if arg.startswith("/clang:-MF")),
        None,
    )
    if forwarded_depfile is not None:
        Path(forwarded_depfile).write_text(
            f"object.obj: {cmd[cmd.index('/c') + 1]}{extra_inputs}\n", encoding="utf-8"
        )
        return
    if "-MF" not in cmd:
        return
    dependency_file = Path(cmd[cmd.index("-MF") + 1])
    dependency_file.write_text(
        f"object.o: {cmd[cmd.index('-c') + 1]}{extra_inputs}\n",
        encoding="utf-8",
    )


def _materialize_fake_extension_command(cmd: list[str]) -> Path:
    cl_output = next((arg[3:] for arg in cmd if arg.startswith("/Fo")), None)
    if cl_output is not None:
        output = Path(cl_output)
        payload = b"object"
    elif "-o" in cmd:
        output = Path(cmd[cmd.index("-o") + 1])
        payload = b"object" if "-c" in cmd else b"wasm-object"
    else:
        archive_mode_index = cmd.index("rcsD")
        output = Path(cmd[archive_mode_index + 1])
        payload = static_archive_bytes()
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_bytes(payload)
    _write_fake_compiler_depfile(cmd)
    return output


def test_resolve_wasm_linker_prefers_matching_wasi_sdk_linker(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    sdk = tmp_path / "wasi-sdk"
    sysroot = sdk / "share" / "wasi-sysroot"
    sysroot.mkdir(parents=True)
    (sysroot / "VERSION").write_text("llvm-version: 22.1.0\n", encoding="utf-8")
    linker = (
        sdk
        / "bin"
        / ("wasm-ld.exe" if cli_wasm_toolchain.os.name == "nt" else "wasm-ld")
    )
    linker.parent.mkdir()
    linker.write_bytes(b"linker")
    monkeypatch.setattr(
        wasm_link_inputs, "resolve_wasi_sysroot", lambda **_kwargs: sysroot
    )
    monkeypatch.setattr(
        cli_wasm_toolchain, "_wasm_linker_version", lambda _path, **_kwargs: "22.1.7"
    )

    identity = cli_wasm_toolchain.resolve_wasm_linker(env={})

    assert identity is not None
    assert identity.path == linker.resolve()
    assert identity.version == "22.1.7"
    assert identity.wasi_sdk_llvm_version == "22.1.0"


def test_resolve_wasm_linker_rejects_wasi_sdk_release_mismatch(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    sysroot = tmp_path / "wasi-sysroot-33.0+m"
    sysroot.mkdir()
    (sysroot / "VERSION").write_text("llvm-version: 22.1.0\n", encoding="utf-8")
    linker = tmp_path / "wasm-ld.exe"
    linker.write_bytes(b"linker")
    monkeypatch.setenv("MOLT_WASM_LD", str(linker))
    monkeypatch.setattr(
        wasm_link_inputs, "resolve_wasi_sysroot", lambda **_kwargs: sysroot
    )
    monkeypatch.setattr(
        cli_wasm_toolchain, "_wasm_linker_version", lambda _path, **_kwargs: "21.1.8"
    )

    with pytest.raises(
        cli_wasm_toolchain.WasmLinkerContractError,
        match="requires LLVM 22.1.0",
    ):
        cli_wasm_toolchain.resolve_wasm_linker()


def test_resolve_wasm_linker_preserves_debian_role_alias(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    directory = tmp_path / "usr" / "lib" / "llvm-22" / "bin"
    directory.mkdir(parents=True)
    driver = directory / "lld"
    driver.write_bytes(b"shared lld driver")
    alias = directory / "wasm-ld"
    try:
        alias.symlink_to(driver.name)
    except OSError:
        os.link(driver, alias)
    monkeypatch.setenv("MOLT_WASM_LD", str(alias))
    monkeypatch.setattr(
        wasm_link_inputs, "resolve_wasi_sysroot", lambda **_kwargs: None
    )
    monkeypatch.setattr(
        cli_wasm_toolchain, "_wasm_linker_version", lambda _path, **_kwargs: "22.1.8"
    )

    identity = cli_wasm_toolchain.resolve_wasm_linker()

    assert identity is not None
    assert identity.path == alias.absolute()
    assert identity.path != driver.absolute()
    assert identity.sha256 == hashlib.sha256(b"shared lld driver").hexdigest()


def test_resolve_wasm_linker_rejects_generic_lld_override(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    driver = tmp_path / "lld"
    driver.write_bytes(b"generic lld")
    monkeypatch.setenv("MOLT_WASM_LD", str(driver))
    monkeypatch.setattr(
        wasm_link_inputs, "resolve_wasi_sysroot", lambda **_kwargs: None
    )

    with pytest.raises(
        cli_wasm_toolchain.WasmLinkerContractError,
        match="generic lld.*not wasm linkers",
    ):
        cli_wasm_toolchain.resolve_wasm_linker()


def test_manifest_source_resolver_never_guesses_relocation_roots(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    source_root = tmp_path / "source"
    source_root.mkdir()
    source = source_root / "source.c"
    source.write_text("int value;", encoding="utf-8")
    manifest_path = tmp_path / "sealed" / "extension_manifest.json"
    manifest_path.parent.mkdir()
    monkeypatch.chdir(source_root)

    resolved, errors = resolve_source_extension_manifest_input(
        "source.c", manifest_path=manifest_path
    )
    assert resolved is None
    assert errors == []
    resolved, errors = resolve_source_extension_manifest_input(
        str(source), manifest_path=manifest_path
    )
    assert resolved == source.resolve()
    assert errors == []


def test_manifest_support_file_object_can_alias_build_source_path(
    tmp_path: Path,
) -> None:
    root = tmp_path / "sealed"
    source = (
        tmp_path
        / "upstream"
        / "scipy"
        / "_external"
        / "packaging_version"
        / "src"
        / "version.py"
    )
    source.parent.mkdir(parents=True)
    source.write_text("class Version: pass\n", encoding="utf-8")
    errors: list[str] = []

    support_files = _manifest_support_file_payloads(
        [
            {
                "path": "scipy/_external/packaging_version/version.py",
                "source": str(source),
            }
        ],
        field_name="support_files",
        root=root,
        errors=errors,
    )

    assert errors == []
    assert [entry.rel_path for entry in support_files] == [
        "scipy/_external/packaging_version/version.py"
    ]
    assert [entry.source_path for entry in support_files] == [source.resolve()]
    assert support_files[0].digest_payload() == {
        "path": "scipy/_external/packaging_version/version.py",
        "sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
    }


def _install_extension_object_symbol_facts(
    monkeypatch: pytest.MonkeyPatch,
    *,
    default_init_symbol: str,
    by_stem: dict[str, tuple[set[str], set[str]]] | None = None,
    linked_stems: tuple[str, ...] | None = None,
) -> None:
    symbol_facts = by_stem or {}
    linked_symbol_facts = (
        [symbol_facts[stem] for stem in linked_stems]
        if linked_stems is not None
        else list(symbol_facts.values())
    )

    def fake_object_symbols(
        path: Path,
        *,
        nm_command: tuple[str, ...] | None = None,
        target_triple: str | None = None,
        aggregate_linker_closure: bool = False,
    ) -> cli_source_extensions._SourceExtensionArtifactSymbolInspection | None:
        artifact_bytes = path.read_bytes()
        is_wasm = artifact_bytes.startswith(b"\0asm\x01\0\0\0")
        symbol_authority = (
            cli_source_extensions.SOURCE_EXTENSION_WASM_SYMBOL_AUTHORITY
            if is_wasm
            else cli_source_extensions.SOURCE_EXTENSION_NATIVE_SYMBOL_AUTHORITY
        )
        import_interface = (
            wasm_artifact.read_wasm_import_interface(path) if is_wasm else None
        )
        wasm_imports = (
            import_interface.imports if import_interface is not None else None
        )

        def inspection(
            defined: set[str], undefined: set[str]
        ) -> cli_source_extensions._SourceExtensionArtifactSymbolInspection:
            return cli_source_extensions._SourceExtensionArtifactSymbolInspection(
                defined_symbols=frozenset(defined),
                undefined_symbols=frozenset(undefined),
                defined_function_symbols=frozenset(defined),
                symbol_authority=symbol_authority,
                wasm_imports=wasm_imports,
                wasm_function_import_signatures=(
                    import_interface.function_signatures
                    if import_interface is not None
                    else ()
                ),
                wasm_function_exports=(
                    tuple(
                        sorted(
                            export.name
                            for export in wasm_artifact.read_wasm_function_exports(path)
                        )
                    )
                    if is_wasm
                    else ()
                ),
                artifact_bytes=artifact_bytes if is_wasm else None,
                artifact_digest=hashlib.sha256(artifact_bytes).hexdigest(),
            )

        del nm_command, target_triple
        if path.name.endswith((".molt.wasm", ".molt.a")):
            defined = {
                symbol
                for symbols, _undefined in linked_symbol_facts
                for symbol in symbols
            }
            undefined = {
                symbol
                for _defined, symbols in linked_symbol_facts
                for symbol in symbols
                if symbol not in defined
            }
            if symbol_facts:
                if aggregate_linker_closure:
                    undefined.difference_update(defined)
                return inspection(defined, undefined)
        stem = path.stem.split("_", 1)[1] if "_" in path.stem else path.stem
        if stem in symbol_facts:
            defined, undefined = symbol_facts[stem]
            if aggregate_linker_closure:
                undefined = set(undefined) - set(defined)
            return inspection(set(defined), set(undefined))
        return inspection(
            {default_init_symbol}, {"PyModule_Create2", "molt_c_api_version"}
        )

    monkeypatch.setattr(
        cli_source_extensions,
        "_inspect_source_extension_artifact_symbols",
        fake_object_symbols,
    )


def _write_fake_wasi_sysroot(root: Path) -> Path:
    sysroot = root / "wasi-sysroot"
    include_dir = sysroot / "include"
    include_dir.mkdir(parents=True)
    (include_dir / "errno.h").write_text("#define EINVAL 28\n")
    return sysroot


def _finalize_test_extension_object_closure(
    manifest: dict[str, object],
    *,
    artifact_path: Path,
) -> None:
    closure = manifest["object_closure"]
    assert isinstance(closure, dict)
    objects = closure["objects"]
    assert isinstance(objects, list) and objects
    inspection = cli_source_extensions._inspect_source_extension_artifact_symbols(
        artifact_path,
        aggregate_linker_closure=True,
    )
    assert inspection is not None and inspection.wasm_imports is not None
    closure["defined_symbols"] = sorted(inspection.defined_symbols)
    closure["undefined_symbols"] = sorted(inspection.undefined_symbols)
    receipts = list(source_extension_wasm_import_receipts(inspection.wasm_imports))
    closure["wasm_imports"] = receipts
    declared_sources = manifest.get("sources")
    source_fallbacks = (
        declared_sources
        if isinstance(declared_sources, list) and len(declared_sources) == len(objects)
        else [str(artifact_path)] * len(objects)
    )
    for item, source_fallback in zip(objects, source_fallbacks, strict=True):
        assert isinstance(item, dict)
        item.setdefault("source", source_fallback)
        item.setdefault("language", "c")
        item.setdefault(
            "source_sha256",
            hashlib.sha256(Path(source_fallback).read_bytes()).hexdigest(),
        )
        item.setdefault(
            "object_sha256", hashlib.sha256(artifact_path.read_bytes()).hexdigest()
        )
        item.setdefault("defined_symbols", [])
        item.setdefault("undefined_symbols", [])
        item.setdefault(
            "compile_command",
            ["fixture-compiler", "-x", "c", "-c", str(item["source"])],
        )
        item["symbol_authority"] = SOURCE_EXTENSION_WASM_SYMBOL_AUTHORITY
        item.pop("symbol_command", None)
        item.setdefault("dependencies", [])
        item.setdefault("required_c_api_symbols", [])
        item.setdefault("required_capsules", [])
        item.setdefault("project_generated_c_api_symbols", [])
        item["defined_symbols"] = sorted(
            set(item["defined_symbols"]) & set(inspection.defined_symbols)
        )
        item["undefined_symbols"] = sorted(
            set(item["undefined_symbols"]) & set(inspection.undefined_symbols)
        )
        if item.get("object") == closure.get("init_symbol_owner"):
            item["defined_symbols"] = sorted(
                set(item["defined_symbols"]) | {str(closure["root_symbol"])}
            )
    closure["runtime_symbols"] = list(
        wasm_static_link_runtime_symbols_for_imports(
            (
                *inspection.undefined_symbols,
                *(receipt["name"] for receipt in receipts),
            ),
            typed_imports=(
                (receipt["module"], receipt["name"]) for receipt in receipts
            ),
        )
    )
    for field in (
        "required_c_api_symbols",
        "required_capsules",
        "project_generated_c_api_symbols",
    ):
        closure[field] = sorted(
            {
                value
                for item in objects
                for value in item[field]
                if isinstance(value, str) and value
            }
        )
    manifest.setdefault("build", {})
    finalize_source_extension_object_closure(manifest)


def _write_extension_project(
    project_root: Path,
    *,
    extension_extra_lines: list[str] | None = None,
) -> None:
    src_dir = project_root / "src"
    src_dir.mkdir(parents=True, exist_ok=True)
    (src_dir / "demoext.c").write_text(
        "#include <Python.h>\n"
        "#include <molt/molt.h>\n"
        "int demoext_version(void) { return (int)molt_c_api_version(); }\n"
        "static PyModuleDef demoext_module = {\n"
        "    PyModuleDef_HEAD_INIT,\n"
        '    "demoext",\n'
        "    NULL,\n"
        "    -1,\n"
        "    NULL,\n"
        "};\n"
        "PyMODINIT_FUNC PyInit_demoext(void) {\n"
        "    return PyModule_Create(&demoext_module);\n"
        "}\n"
    )
    (project_root / "pyproject.toml").write_text(
        "\n".join(
            [
                "[project]",
                'name = "demo-ext"',
                'version = "0.1.0"',
                "",
                "[tool.molt.extension]",
                'module = "demoext"',
                'sources = ["src/demoext.c"]',
                'capabilities = ["fs.read"]',
                'molt_c_api_version = "2"',
                *(extension_extra_lines or []),
                "",
            ]
        )
    )


def _write_meson_source_plan_project(
    project_root: Path,
    *,
    linked_static_library: bool = False,
    aggregate_static_library: bool = False,
    static_library_suffix: str = ".a",
    nested_linker: bool = False,
) -> Path:
    if static_library_suffix not in {".a", ".lib"}:
        raise ValueError("unsupported fixture archive suffix")
    src_dir = project_root / "pkg"
    include_dir = src_dir / "include"
    generated_dir = project_root / "build" / "generated"
    meson_info_dir = project_root / "build" / "meson-info"
    include_dir.mkdir(parents=True, exist_ok=True)
    generated_dir.mkdir(parents=True, exist_ok=True)
    meson_info_dir.mkdir(parents=True, exist_ok=True)
    (src_dir / "__init__.py").write_text("VALUE = 1\n", encoding="utf-8")
    (include_dir / "demoext.h").write_text(
        "#define NPY_HEADER_ONLY_MACRO 17\n"
        "#define NPY_GENERATED_DECL(name) int npy_generated_ ## name(void)\n"
        "int helper_generated(void);\n"
    )
    (src_dir / "demoext.c").write_text(
        "#include <Python.h>\n"
        '#include "demoext.h"\n'
        "static void unused_version_compat(void) {\n"
        "    (void)_PyFloat_FormatAdvancedWriter;\n"
        "}\n"
        "static PyModuleDef demoext_module = {\n"
        "    PyModuleDef_HEAD_INIT,\n"
        '    "demoext",\n'
        "    NULL,\n"
        "    -1,\n"
        "    NULL,\n"
        "};\n"
        "PyMODINIT_FUNC PyInit_demoext(void) {\n"
        "    (void)NPY_HEADER_ONLY_MACRO;\n"
        "#ifdef NPY_DISABLED_alias\n"
        "    (void)NPY_DISABLED_Alias;\n"
        "#else\n"
        "    (void)PyLong_FromLong(1);\n"
        "#endif\n"
        "#ifdef DEMO_COMPILE_DB\n"
        "    (void)PyTuple_New(0);\n"
        "#else\n"
        "    (void)PyCode_NewWithPosOnlyArgs;\n"
        "#endif\n"
        "#if (0 && 0) && \\\n"
        "    defined(NPY_DISABLED_SVE)\n"
        "    (void)NPY_DISABLED_SVE;\n"
        "#endif\n"
        "#if CYTHON_DISABLED_WRITER\n"
        "    (void)_PyLong_FormatAdvancedWriter;\n"
        "#endif\n"
        "#if PY_VERSION_HEX >= 0x030d0000\n"
        "    (void)PyUnicode_CopyCharacters;\n"
        "#endif\n"
        "    (void)npy_generated_int8;\n"
        "    (void)helper_generated();\n"
        "    return PyModule_Create(&demoext_module);\n"
        "}\n",
        encoding="utf-8",
    )
    (generated_dir / "helper_generated.c").write_text(
        "#include <Python.h>\n"
        "int helper_generated(void) { return (int)PyLong_AsLong(PyLong_FromLong(7)); }\n",
        encoding="utf-8",
    )
    (generated_dir / "generated_only.h").write_text("#define GENERATED_ONLY 1\n")
    if linked_static_library:
        (src_dir / "unique.cpp").write_text(
            "int array__unique_hash(void) { return 1; }\n",
            encoding="utf-8",
        )
    if aggregate_static_library:
        (generated_dir / "loops_arithmetic.dispatch.c").write_text(
            "int FLOAT_add_indexed(void) { return 1; }\n",
            encoding="utf-8",
        )
        (generated_dir / "simd.dispatch.c").write_text(
            "int SIMD_not_linked(void) { return 1; }\n",
            encoding="utf-8",
        )
    linker_parameters: list[str] = []
    if linked_static_library:
        linker_parameters.append("pkg/libunique_hash.a")
    if aggregate_static_library:
        linker_parameters.append("pkg/lib_multiarray_umath_mtargets.a")
    intro_targets = [
        {
            "id": "pkg.demoext",
            "name": "demoext",
            "type": "shared module",
            "filename": str(project_root / "build" / "pkg" / "demoext.so"),
            "target_sources": [
                {
                    "language": "c",
                    "machine": "host",
                    "parameters": ["-I", "pkg/include", "-DINTRO_ONLY=1"],
                    "sources": ["pkg/demoext.c"],
                    "generated_sources": [],
                },
                {
                    "language": "c",
                    "machine": "host",
                    "parameters": ["-Igenerated", "-DINTRO_GENERATED_ONLY=1"],
                    "sources": [],
                    "generated_sources": [
                        "generated/helper_generated.c",
                        "generated/generated_only.h",
                    ],
                },
            ],
            "linker_parameters": linker_parameters,
        }
    ]
    if linked_static_library:
        intro_targets.append(
            {
                "id": "pkg.libunique_hash",
                "name": "unique_hash",
                "type": "static library",
                "filename": str(project_root / "build" / "pkg" / "libunique_hash.a"),
                "target_sources": [
                    {
                        "language": "cpp",
                        "machine": "host",
                        "parameters": ["-I", "pkg/include", "-DSTATIC_LIB=1"],
                        "sources": ["pkg/unique.cpp"],
                        "generated_sources": ["generated/cleaned_unique.c"],
                    }
                ],
            }
        )
    if aggregate_static_library:
        intro_targets.extend(
            [
                {
                    "id": "pkg.lib_multiarray_umath_mtargets",
                    "name": "_multiarray_umath_mtargets",
                    "type": "static library",
                    "filename": str(
                        project_root
                        / "build"
                        / "pkg"
                        / "lib_multiarray_umath_mtargets.a"
                    ),
                    "defined_in": str(project_root / "meson.build"),
                    "build_by_default": True,
                    "target_sources": [
                        {
                            "language": None,
                            "machine": "host",
                            "parameters": ["csrDT"],
                        }
                    ],
                },
                {
                    "id": "pkg.libloops_arithmetic_dispatch",
                    "name": "loops_arithmetic.dispatch.h_baseline",
                    "type": "static library",
                    "filename": str(
                        project_root
                        / "build"
                        / "pkg"
                        / "libloops_arithmetic.dispatch.h_baseline.a"
                    ),
                    "defined_in": str(project_root / "meson.build"),
                    "build_by_default": True,
                    "target_sources": [
                        {
                            "language": "c",
                            "machine": "host",
                            "parameters": ["-Igenerated", "-DDISPATCH=1"],
                            "sources": [],
                            "generated_sources": [
                                "generated/loops_arithmetic.dispatch.c"
                            ],
                        }
                    ],
                },
                {
                    "id": "pkg.lib_simd_mtargets",
                    "name": "_simd_mtargets",
                    "type": "static library",
                    "filename": str(
                        project_root / "build" / "pkg" / "lib_simd_mtargets.a"
                    ),
                    "defined_in": str(project_root / "meson.build"),
                    "build_by_default": True,
                    "target_sources": [
                        {
                            "language": None,
                            "machine": "host",
                            "parameters": ["csrDT"],
                        }
                    ],
                },
                {
                    "id": "pkg.lib_simd_dispatch",
                    "name": "_simd.dispatch.h_baseline",
                    "type": "static library",
                    "filename": str(
                        project_root
                        / "build"
                        / "pkg"
                        / "lib_simd.dispatch.h_baseline.a"
                    ),
                    "defined_in": str(project_root / "meson.build"),
                    "build_by_default": True,
                    "target_sources": [
                        {
                            "language": "c",
                            "machine": "host",
                            "parameters": ["-Igenerated", "-DSIMD=1"],
                            "sources": [],
                            "generated_sources": ["generated/simd.dispatch.c"],
                        }
                    ],
                },
            ]
        )
    if nested_linker:
        intro_targets[0]["linker_parameters"] = []
        intro_targets[0]["target_sources"].append(
            {"linker": ["lld-link"], "parameters": linker_parameters}
        )

    # Project the same fixture graph across archive output conventions.
    def archive_names(value: Any) -> Any:
        if isinstance(value, str):
            return value.replace(".a.p/", static_library_suffix + ".p/").removesuffix(
                ".a"
            ) + (static_library_suffix if value.endswith(".a") else "")
        if isinstance(value, list):
            return [archive_names(item) for item in value]
        if isinstance(value, dict):
            return {key: archive_names(item) for key, item in value.items()}
        return value

    intro_targets = archive_names(intro_targets)
    intro_path = meson_info_dir / "intro-targets.json"
    intro_path.write_text(json.dumps(intro_targets, indent=2) + "\n")
    compile_commands = [
        {
            "directory": str(project_root),
            "file": "pkg/demoext.c",
            "arguments": [
                "cc",
                "-I",
                "pkg/include",
                "-DDEMO_COMPILE_DB=1",
                "-DCYTHON_DISABLED_WRITER=0",
                "-c",
                "pkg/demoext.c",
                "-o",
                "build/pkg/demoext.so.p/demoext.c.o",
            ],
        },
        {
            "directory": str(project_root / "build"),
            "file": "generated/helper_generated.c",
            "arguments": [
                "cc",
                "-Igenerated",
                "-DHELPER_COMPILE_DB=1",
                "-c",
                "generated/helper_generated.c",
                "-o",
                "pkg/demoext.so.p/helper_generated.c.o",
            ],
        },
    ]
    if linked_static_library:
        compile_commands.append(
            {
                "directory": str(project_root),
                "file": "pkg/unique.cpp",
                "arguments": [
                    "c++",
                    "-I",
                    "pkg/include",
                    "-DSTATIC_LIB=1",
                    "-c",
                    "pkg/unique.cpp",
                    "-o",
                    "build/pkg/libunique_hash.a.p/unique.cpp.o",
                ],
            }
        )
    if aggregate_static_library:
        compile_commands.append(
            {
                "directory": str(project_root / "build"),
                "file": "generated/loops_arithmetic.dispatch.c",
                "arguments": [
                    "cc",
                    "-Igenerated",
                    "-DDISPATCH=1",
                    "-c",
                    "generated/loops_arithmetic.dispatch.c",
                    "-o",
                    "pkg/libloops_arithmetic.dispatch.h_baseline.a.p/"
                    "loops_arithmetic.dispatch.c.o",
                ],
            }
        )
        compile_commands.append(
            {
                "directory": str(project_root / "build"),
                "file": "generated/simd.dispatch.c",
                "arguments": [
                    "cc",
                    "-Igenerated",
                    "-DSIMD=1",
                    "-c",
                    "generated/simd.dispatch.c",
                    "-o",
                    "pkg/lib_simd.dispatch.h_baseline.a.p/simd.dispatch.c.o",
                ],
            }
        )
        (project_root / "build" / "build.ninja").write_text(
            (
                "build pkg/lib_multiarray_umath_mtargets.a: STATIC_LINKER "
                "pkg/libloops_arithmetic.dispatch.h_baseline.a.p/"
                "loops_arithmetic.dispatch.c.o\n"
                "  LINK_ARGS = csrDT\n"
                "build pkg/lib_simd_mtargets.a: STATIC_LINKER "
                "pkg/lib_simd.dispatch.h_baseline.a.p/simd.dispatch.c.o\n"
                "  LINK_ARGS = csrDT\n"
            ).replace(".a", static_library_suffix),
            encoding="utf-8",
        )
    (project_root / "build" / "compile_commands.json").write_text(
        json.dumps(archive_names(compile_commands), indent=2) + "\n",
        encoding="utf-8",
    )
    (project_root / "pyproject.toml").write_text(
        "\n".join(
            [
                "[project]",
                'name = "demo-meson-ext"',
                'version = "0.1.0"',
                "",
                "[tool.molt.extension]",
                'module = "pkg.demoext"',
                'capabilities = ["fs.read"]',
                'molt_c_api_version = "1"',
                'python_exports = ["pkg.demoext"]',
                "",
                "[tool.molt.extension.source_plan]",
                'kind = "meson-intro-targets"',
                'path = "build/meson-info/intro-targets.json"',
                'target = "pkg.demoext"',
                'source_root = "."',
                'build_root = "build"',
                "",
            ]
        ),
        encoding="utf-8",
    )
    return intro_path


def _write_extension_scan_project(project_root: Path) -> None:
    src_dir = project_root / "src"
    src_dir.mkdir(parents=True, exist_ok=True)
    (src_dir / "demoext.c").write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "",
                "static PyObject *scan_probe(PyObject *self, PyObject *args) {",
                "    PyObject *value = PyLong_FromLong(7);",
                "    (void)PyType_FromSpec;",
                "    (void)PyType_FromModuleAndSpec;",
                "    (void)PyType_GetModule;",
                "    (void)PyType_GetModuleState;",
                "    (void)PyType_GetModuleByDef;",
                "    (void)PyThreadState_Get;",
                "    (void)PyGILState_Ensure;",
                "    (void)PyGILState_Release;",
                "    (void)PyImport_ImportModule;",
                "    (void)PyCapsule_Import;",
                "    (void)PyArg_UnpackTuple;",
                "    (void)PyAnySet_Check;",
                "    (void)PyComplex_CheckExact;",
                "    (void)PyDate_Check;",
                "    (void)PyDateTime_Check;",
                "    (void)PyDelta_Check;",
                "    (void)PyDateTime_IMPORT;",
                "    (void)PyLong_AsLongLongAndOverflow;",
                "    (void)PyNumber_Long;",
                "    (void)PyIter_Check;",
                "    (void)PyIter_Next;",
                "    (void)PyObject_Next;",
                "    (void)PyOS_string_to_double;",
                "    (void)PyObject_Vectorcall;",
                "    (void)PyCode_NewWithPosOnlyArgs;",
                "    return value;",
                "}",
                "",
            ]
        )
        + "\n"
    )
    (project_root / "pyproject.toml").write_text(
        "\n".join(
            [
                "[project]",
                'name = "scan-ext"',
                'version = "0.1.0"',
                "",
                "[tool.molt.extension]",
                'module = "demoext"',
                'sources = ["src/demoext.c"]',
                'capabilities = ["fs.read"]',
                'molt_c_api_version = "1"',
                "",
            ]
        )
    )


def _write_extension_numpy_project(project_root: Path) -> None:
    src_dir = project_root / "src"
    src_dir.mkdir(parents=True, exist_ok=True)
    (src_dir / "demoext.c").write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "#include <numpy/arrayobject.h>",
                "#include <numpy/npy_math.h>",
                "",
                "static int numpy_probe(PyObject *obj) {",
                "    PyArrayObject *arr = (PyArrayObject *)obj;",
                "    PyArray_Descr *descr = PyArray_DescrFromType(NPY_INT);",
                "    PyArray_Descr *scalar_descr = PyArray_DescrFromScalar(obj);",
                "    npy_cdouble complex_value = {0};",
                "    npy_intp ndim = PyArray_NDIM(arr);",
                "    npy_intp size = PyArray_SIZE(arr);",
                "    int is_int = PyTypeNum_ISINTEGER(PyArray_TYPE(arr));",
                "    int scalar_check = PyArray_CheckScalar(obj);",
                "    int is_datetime = PyArray_ISDATETIME(arr);",
                "    enum NPY_TYPES array_type = NPY_DOUBLE;",
                "    int notype = NPY_NOTYPE;",
                "    int behaved = NPY_ARRAY_BEHAVED_NS;",
                "    int branch = NPY_UNLIKELY(size < 0);",
                "    unsigned int max_u8 = NPY_MAX_UBYTE;",
                "    int min_i8 = NPY_MIN_BYTE;",
                "    double real = npy_creal(complex_value);",
                "    double imag = npy_cimag(complex_value);",
                "    NPY_BEGIN_THREADS_DEF;",
                "    NPY_ALLOW_C_API_DEF;",
                "    NPY_BEGIN_THREADS;",
                "    NPY_END_THREADS;",
                "    NPY_BEGIN_THREADS_THRESHOLDED(size);",
                "    NPY_END_THREADS;",
                "    NPY_ALLOW_C_API;",
                "    NPY_DISABLE_C_API;",
                "    NPY_CSETREAL(&complex_value, real);",
                "    NPY_CSETIMAG(&complex_value, imag);",
                "    (void)PyArray_CastScalarToCtype;",
                "    if (descr != NULL) {",
                "        PyMem_Free(descr);",
                "    }",
                "    if (scalar_descr != NULL) {",
                "        PyMem_Free(scalar_descr);",
                "    }",
                "    return (int)(ndim + size + is_int + scalar_check + is_datetime + array_type + notype + behaved + branch + max_u8 + min_i8 + real + imag);",
                "}",
                "",
                "int demoext_numpy_ready(void) {",
                "    import_array1(-1);",
                "    return 0;",
                "}",
                "",
                "int demoext_numpy_touch(PyObject *obj) {",
                "    return numpy_probe(obj);",
                "}",
                "",
                "static PyModuleDef demoext_numpy_module = {",
                "    PyModuleDef_HEAD_INIT,",
                '    "demoext_numpy",',
                "    NULL,",
                "    -1,",
                "    NULL,",
                "};",
                "",
                "PyMODINIT_FUNC PyInit_demoext_numpy(void) {",
                "    return PyModule_Create(&demoext_numpy_module);",
                "}",
                "",
            ]
        )
        + "\n"
    )
    (project_root / "pyproject.toml").write_text(
        "\n".join(
            [
                "[project]",
                'name = "demo-numpy-ext"',
                'version = "0.1.0"',
                "",
                "[tool.molt.extension]",
                'module = "demoext_numpy"',
                'sources = ["src/demoext.c"]',
                'capabilities = ["fs.read"]',
                'molt_c_api_version = "1"',
                "",
            ]
        )
    )


def _write_extension_iterator_mapping_project(project_root: Path) -> None:
    src_dir = project_root / "src"
    src_dir.mkdir(parents=True, exist_ok=True)
    (src_dir / "demoext_iter.c").write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "",
                "int demoext_iter_mapping_touch(PyObject *seq, PyObject *dict) {",
                "    PyObject *iter = PyObject_GetIter(seq);",
                "    PyObject *first = NULL;",
                "    PyObject *second = NULL;",
                "    PyObject *borrowed = NULL;",
                "    PyObject *values = NULL;",
                "    int ok = 0;",
                "    if (iter == NULL) {",
                "        return -1;",
                "    }",
                "    first = PyIter_Next(iter);",
                "    second = PyObject_Next(iter);",
                "    if (first == NULL || second == NULL) {",
                "        goto done;",
                "    }",
                "    borrowed = PyDict_GetItemWithError(dict, first);",
                "    values = PyMapping_Values(dict);",
                "    if (borrowed != NULL && values != NULL) {",
                "        ok = 1;",
                "    }",
                "done:",
                "    Py_XDECREF(values);",
                "    Py_XDECREF(first);",
                "    Py_XDECREF(second);",
                "    Py_DECREF(iter);",
                "    return ok;",
                "}",
                "",
                "static PyModuleDef demoext_iter_module = {",
                "    PyModuleDef_HEAD_INIT,",
                '    "demoext_iter",',
                "    NULL,",
                "    -1,",
                "    NULL,",
                "};",
                "",
                "PyMODINIT_FUNC PyInit_demoext_iter(void) {",
                "    return PyModule_Create(&demoext_iter_module);",
                "}",
                "",
            ]
        )
        + "\n",
        encoding="utf-8",
    )
    (project_root / "pyproject.toml").write_text(
        "\n".join(
            [
                "[project]",
                'name = "demo-iter-mapping-ext"',
                'version = "0.1.0"',
                "",
                "[tool.molt.extension]",
                'module = "demoext_iter"',
                'sources = ["src/demoext_iter.c"]',
                'capabilities = ["fs.read"]',
                'molt_c_api_version = "1"',
                "",
            ]
        ),
        encoding="utf-8",
    )


def _write_extension_wheel(
    root: Path,
    *,
    capabilities: list[str] | None = None,
    include_checksums: bool,
) -> tuple[Path, Path]:
    wheel_name = "demo_ext-0.1.0-py3-molt_abi1-x86_64_unknown_linux_gnu.whl"
    wheel_path = root / wheel_name
    extension_entry = "demoext.so"
    extension_bytes = b"shared"
    with zipfile.ZipFile(wheel_path, "w") as zf:
        zf.writestr(extension_entry, extension_bytes)

    manifest = {
        "schema_version": 1,
        "name": "demo-ext",
        "version": "0.1.0",
        "module": "demoext",
        "molt_c_api_version": "1",
        "abi_tag": "molt_abi1",
        "python_tag": "py3",
        "target_python": "py312",
        "target_triple": "x86_64-unknown-linux-gnu",
        "platform_tag": "x86_64_unknown_linux_gnu",
        "capabilities": capabilities if capabilities is not None else ["fs.read"],
        "wheel": wheel_name,
        "extension": extension_entry,
        "deterministic": True,
    }
    if include_checksums:
        manifest["wheel_sha256"] = hashlib.sha256(wheel_path.read_bytes()).hexdigest()
        manifest["extension_sha256"] = hashlib.sha256(extension_bytes).hexdigest()
    manifest_path = root / "extension_manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
    return manifest_path, wheel_path


def test_extension_scan_reports_missing_symbols_without_gate(
    tmp_path: Path, capsys
) -> None:
    project_root = tmp_path / "scanproj"
    project_root.mkdir()
    _write_extension_scan_project(project_root)

    rc = cli.extension_scan(
        project=str(project_root),
        fail_on_missing=False,
        json_output=True,
        verbose=False,
    )
    assert rc == 0
    payload = json.loads(capsys.readouterr().out)
    assert payload["status"] == "ok"
    data = payload["data"]
    assert "PyType_FromSpec" in data["supported_symbols"]
    assert "PyType_FromModuleAndSpec" in data["supported_symbols"]
    assert "PyType_GetModule" in data["supported_symbols"]
    assert "PyType_GetModuleState" in data["supported_symbols"]
    assert "PyType_GetModuleByDef" in data["supported_symbols"]
    assert "PyThreadState_Get" in data["supported_symbols"]
    assert "PyGILState_Ensure" in data["supported_symbols"]
    assert "PyGILState_Release" in data["supported_symbols"]
    assert "PyImport_ImportModule" in data["supported_symbols"]
    assert "PyCapsule_Import" in data["supported_symbols"]
    assert "PyArg_UnpackTuple" in data["supported_symbols"]
    assert "PyAnySet_Check" in data["supported_symbols"]
    assert "PyComplex_CheckExact" in data["supported_symbols"]
    assert "PyDate_Check" in data["supported_symbols"]
    assert "PyDateTime_Check" in data["supported_symbols"]
    assert "PyDelta_Check" in data["supported_symbols"]
    assert "PyDateTime_IMPORT" in data["supported_symbols"]
    assert "PyLong_AsLongLongAndOverflow" in data["supported_symbols"]
    assert "PyNumber_Long" in data["supported_symbols"]
    assert "PyIter_Check" in data["supported_symbols"]
    assert "PyIter_Next" in data["supported_symbols"]
    assert "PyObject_Next" in data["supported_symbols"]
    assert "PyOS_string_to_double" in data["supported_symbols"]
    assert "PyObject_Vectorcall" in data["supported_symbols"]
    assert "PyLong_FromLong" in data["supported_symbols"]


def test_public_libmolt_header_declares_iterator_and_dict_view_surface() -> None:
    header = (ROOT / "include" / "molt" / "molt.h").read_text(encoding="utf-8")

    for declaration in [
        "MoltHandle molt_iter_next(MoltHandle iter_bits);",
        "MoltHandle molt_list_append(MoltHandle list_bits, MoltHandle val_bits);",
        "MoltHandle molt_dict_keys(MoltHandle dict_bits);",
        "MoltHandle molt_dict_values(MoltHandle dict_bits);",
        "MoltHandle molt_dict_items(MoltHandle dict_bits);",
        "MoltHandle molt_dict_getitem_borrowed(MoltHandle dict_bits, MoltHandle key_bits);",
    ]:
        assert declaration in header


def test_extension_scan_fail_on_missing_returns_error(tmp_path: Path, capsys) -> None:
    project_root = tmp_path / "scanproj"
    project_root.mkdir()
    _write_extension_scan_project(project_root)

    rc = cli.extension_scan(
        project=str(project_root),
        fail_on_missing=True,
        json_output=True,
        verbose=False,
    )
    assert rc == 1
    payload = json.loads(capsys.readouterr().out)
    assert payload["status"] == "error"
    assert "PyCode_NewWithPosOnlyArgs" in payload["data"]["missing_symbols"]


def test_extension_scan_accepts_source_directories_deterministically(
    tmp_path: Path, capsys
) -> None:
    project_root = tmp_path / "scan_dir_project"
    src = project_root / "src"
    nested = src / "nested"
    ignored = src / "build"
    nested.mkdir(parents=True)
    ignored.mkdir()
    (project_root / "pyproject.toml").write_text("[project]\nname = 'scan-dir'\n")
    (src / "a.c").write_text(
        "#include <Python.h>\nPyObject *a(void) { return PyLong_FromLong(1); }\n"
    )
    (nested / "b.h").write_text(
        "#include <Python.h>\nvoid *b(void) { return (void *)PyCode_NewWithPosOnlyArgs; }\n"
    )
    (ignored / "ignored.c").write_text(
        "#include <Python.h>\nvoid *ignored(void) { return (void *)PyObject_Str; }\n"
    )
    (src / "not_a_source.txt").write_text("PyObject_Repr should not be scanned\n")

    rc = cli.extension_scan(
        project=str(project_root),
        sources=[str(src)],
        fail_on_missing=False,
        json_output=True,
        verbose=False,
    )

    assert rc == 0
    payload = json.loads(capsys.readouterr().out)
    data = payload["data"]
    scanned = [
        Path(path).relative_to(project_root).as_posix()
        for path in data["required_by_file"]
    ]
    assert scanned == ["src/a.c", "src/nested/b.h"]
    assert data["source_count"] == 2
    assert "PyLong_FromLong" in data["supported_symbols"]
    assert data["symbol_status"]["PyLong_FromLong"] == "runtime_backed"
    assert "PyCode_NewWithPosOnlyArgs" in data["missing_symbols"]
    assert data["symbol_status"]["PyCode_NewWithPosOnlyArgs"] == "missing"


def test_extension_scan_excludes_non_build_source_directories(
    tmp_path: Path, capsys
) -> None:
    project_root = tmp_path / "scan_exclude_project"
    src = project_root / "src"
    tests_dir = src / "tests"
    tests_dir.mkdir(parents=True)
    (project_root / "pyproject.toml").write_text("[project]\nname = 'scan-exclude'\n")
    (src / "module.c").write_text(
        "#include <Python.h>\nPyObject *ok(void) { return PyLong_FromLong(1); }\n"
    )
    (tests_dir / "fixture.c").write_text(
        "#include <Python.h>\n"
        "void *fixture(void) { return (void *)PyCode_NewWithPosOnlyArgs; }\n"
    )

    rc = cli.extension_scan(
        project=str(project_root),
        sources=[str(src)],
        exclude_dirs=["tests"],
        fail_on_missing=True,
        json_output=True,
        verbose=False,
    )

    assert rc == 0
    payload = json.loads(capsys.readouterr().out)
    data = payload["data"]
    assert data["source_count"] == 1
    assert data["exclude_dirs"] == ["tests"]
    assert "PyCode_NewWithPosOnlyArgs" not in data["required_symbols"]


def test_extension_scan_reads_non_utf8_source_deterministically(
    tmp_path: Path, capsys
) -> None:
    project_root = tmp_path / "scan_non_utf8_project"
    src = project_root / "src"
    src.mkdir(parents=True)
    (project_root / "pyproject.toml").write_text("[project]\nname = 'scan-non-utf8'\n")
    (src / "module.c").write_bytes(
        b"#include <Python.h>\n"
        b"// non-utf8 byte: \x90\n"
        b"PyObject *ok(void) { return PyLong_FromLong(1); }\n"
    )

    rc = cli.extension_scan(
        project=str(project_root),
        sources=[str(src)],
        fail_on_missing=True,
        json_output=True,
        verbose=False,
    )

    assert rc == 0
    payload = json.loads(capsys.readouterr().out)
    assert payload["data"]["source_count"] == 1
    assert payload["data"]["symbol_status"]["PyLong_FromLong"] == "runtime_backed"


def test_extension_scan_resolves_package_defined_py_symbols(
    tmp_path: Path, capsys
) -> None:
    project_root = tmp_path / "scan_project_defined"
    src = project_root / "src"
    src.mkdir(parents=True)
    (project_root / "pyproject.toml").write_text("[project]\nname = 'scan-local'\n")
    (src / "defs.h").write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "#define PyLocalMacro(value) (value)",
                "typedef struct PyLocalStruct { int value; } PyLocalStruct;",
                "enum { NPY_LOCAL_ENUM = 3 };",
                "static const int NPY_LOCAL_STATIC_CONST = 5;",
                "",
            ]
        )
    )
    (src / "defs.c").write_text(
        "\n".join(
            [
                '#include "defs.h"',
                "",
                "PyObject *PyLocal_FromThing(PyObject *value) {",
                "    Py_INCREF(value);",
                "    return value;",
                "}",
                "",
                "static PyTypeObject PyLocal_Type = {0};",
                "PyObject *PyTentative_Global;",
                "",
                "PyObject *",
                "PyPlainSplit_FromThing(PyObject *PyParam)",
                "{",
                "    Py_INCREF(PyParam);",
                "    return PyParam;",
                "}",
                "",
            ]
        )
    )
    (src / "use.c").write_text(
        "\n".join(
            [
                '#include "defs.h"',
                "",
                "PyObject *use(PyObject *value) {",
                "    PyObject *PyLocalTemp = value;",
                "    PyLocalStruct local = {0};",
                "    (void)PyLocalTemp;",
                "    (void)local;",
                "    (void)PyLocalMacro(value);",
                "    (void)NPY_LOCAL_ENUM;",
                "    (void)NPY_LOCAL_STATIC_CONST;",
                "    (void)PyTentative_Global;",
                "    (void)PyPlainSplit_FromThing(value);",
                "    (void)value;  # PyTrailingCommentOnly should not be scanned",
                "#ifdef Py_LIMITED_API",
                "    (void)PyLong_FromLong(1);",
                "#endif",
                "# PyCythonCommentOnly should not be scanned",
                '"""PyTripleDocOnly should not be scanned"""',
                "    return PyLocal_FromThing(value);",
                "}",
                "",
            ]
        )
    )

    rc = cli.extension_scan(
        project=str(project_root),
        sources=[str(src)],
        fail_on_missing=True,
        json_output=True,
        verbose=False,
    )

    assert rc == 0
    payload = json.loads(capsys.readouterr().out)
    data = payload["data"]
    for symbol in [
        "PyLocalMacro",
        "PyLocalStruct",
        "PyLocal_FromThing",
        "PyTentative_Global",
        "PyPlainSplit_FromThing",
        "NPY_LOCAL_ENUM",
        "NPY_LOCAL_STATIC_CONST",
    ]:
        assert symbol in data["project_defined_symbols"]
        assert data["symbol_status"][symbol] == "project_defined"
    assert "PyParam" not in data["required_symbols"]
    assert "PyLocalTemp" not in data["required_symbols"]
    assert "PyTypeObject" not in data["required_symbols"]
    assert "Python" not in data["required_symbols"]
    assert "PyLocal_Type" not in data["project_defined_symbols"]
    assert "PyLong_FromLong" in data["required_symbols"]
    assert data["symbol_status"]["PyLong_FromLong"] == "runtime_backed"
    assert "PyCythonCommentOnly" not in data["required_symbols"]
    assert "PyTrailingCommentOnly" not in data["required_symbols"]
    assert "PyTripleDocOnly" not in data["required_symbols"]


def test_extension_scan_preserves_guarded_body_symbols(tmp_path: Path, capsys) -> None:
    project_root = tmp_path / "scan_guarded_body"
    src = project_root / "src"
    src.mkdir(parents=True)
    (project_root / "pyproject.toml").write_text("[project]\nname = 'scan-guard'\n")
    (src / "guarded.c").write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "PyObject *use(PyObject *value) {",
                "#ifdef Py_LIMITED_API",
                "    return PyCode_NewWithPosOnlyArgs(value);",
                "#endif",
                "    Py_RETURN_NONE;",
                "}",
                "",
            ]
        )
    )

    rc = cli.extension_scan(
        project=str(project_root),
        sources=[str(src)],
        fail_on_missing=False,
        json_output=True,
        verbose=False,
    )

    assert rc == 0
    payload = json.loads(capsys.readouterr().out)
    data = payload["data"]
    assert "PyCode_NewWithPosOnlyArgs" in data["required_symbols"]
    assert data["symbol_status"]["PyCode_NewWithPosOnlyArgs"] == "missing"


def test_extension_scan_macro_bodies_do_not_define_called_apis(
    tmp_path: Path, capsys
) -> None:
    project_root = tmp_path / "scan_macro_body"
    src = project_root / "src"
    src.mkdir(parents=True)
    (project_root / "pyproject.toml").write_text("[project]\nname = 'scan-macro'\n")
    (src / "macro.h").write_text(
        "\n".join(
            [
                "#define PyLocalMacro(npy_type) \\",
                "    (PyMacroMissingAPI((npy_type)))",
                "",
            ]
        )
    )
    (src / "use.c").write_text(
        "\n".join(
            [
                '#include "macro.h"',
                "PyObject *use(PyObject *value) {",
                "    return PyLocalMacro(value);",
                "}",
                "",
            ]
        )
    )

    rc = cli.extension_scan(
        project=str(project_root),
        sources=[str(src)],
        fail_on_missing=False,
        json_output=True,
        verbose=False,
    )

    assert rc == 0
    payload = json.loads(capsys.readouterr().out)
    data = payload["data"]
    assert data["symbol_status"]["PyLocalMacro"] == "project_defined"
    assert data["symbol_status"]["PyMacroMissingAPI"] == "missing"
    assert "npy_type" not in data["required_symbols"]


def test_extension_scan_classifies_project_generated_c_api_symbols(
    tmp_path: Path, capsys
) -> None:
    project_root = tmp_path / "scan_generated_api"
    src = project_root / "src"
    src.mkdir(parents=True)
    (project_root / "pyproject.toml").write_text("[project]\nname = 'scan-gen'\n")
    (src / "generated.c").write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "#define NPY_GENERATED_DECL(name) int npy_generated_ ## name(void)",
                "#define NPYV_TOO_BROAD(SFX) npyv_ ## SFX",
                "PyObject *use(PyObject *value) {",
                "    (void)npy_generated_int8;",
                "    (void)npyv_u8;",
                "    (void)npyv_lanetype_f32;",
                "    (void)npyv_loadable_stride_f32;",
                "    (void)npyv_storable_stride_f32;",
                "    (void)npy_missing_runtime;",
                "    return value;",
                "}",
                "",
            ]
        )
    )

    rc = cli.extension_scan(
        project=str(project_root),
        sources=[str(src)],
        fail_on_missing=False,
        json_output=True,
        verbose=False,
    )

    assert rc == 0
    payload = json.loads(capsys.readouterr().out)
    data = payload["data"]
    assert data["symbol_status"]["npy_generated_int8"] == "project_generated"
    assert data["project_generated_c_api_prefixes"] == ["npy_generated_"]
    assert data["project_generated_symbols"] == ["npy_generated_int8"]
    assert not is_c_api_external_requirement("npyv_u8")
    assert not is_c_api_external_requirement("npyv_lanetype")
    assert not is_c_api_external_requirement("npyv_loadable_stride")
    assert not is_c_api_external_requirement("npyv_storable_stride")
    for symbol in (
        "npyv_u8",
        "npyv_lanetype_f32",
        "npyv_loadable_stride_f32",
        "npyv_storable_stride_f32",
    ):
        assert symbol not in data["required_symbols"]
        assert symbol not in data["missing_symbols"]
    assert data["symbol_status"]["npy_missing_runtime"] == "missing"
    assert "npy_missing_runtime" in data["missing_symbols"]


def test_extension_scan_numpy_surface_fails_closed_without_package_headers(
    tmp_path: Path, capsys
) -> None:
    project_root = tmp_path / "numpy_scanproj"
    project_root.mkdir()
    _write_extension_numpy_project(project_root)

    rc = cli.extension_scan(
        project=str(project_root),
        fail_on_missing=True,
        json_output=True,
        verbose=False,
    )
    assert rc == 1
    payload = json.loads(capsys.readouterr().out)
    assert payload["status"] == "error"
    assert payload["errors"] == ["unsupported C-API symbols found"]
    data = payload["data"]
    assert data["fail_fast_symbols"] == []
    assert data["source_compile_only_symbols"] == []
    expected_missing = {
        "PyArray_CastScalarToCtype",
        "PyArray_CheckScalar",
        "PyArray_DescrFromScalar",
        "PyArray_DescrFromType",
        "PyArray_ISDATETIME",
        "PyArray_NDIM",
        "PyArray_SIZE",
        "PyArray_TYPE",
        "PyTypeNum_ISINTEGER",
        "NPY_ARRAY_BEHAVED_NS",
        "NPY_INT",
        "NPY_NOTYPE",
        "npy_cimag",
        "npy_creal",
    }
    assert expected_missing <= set(data["missing_symbols"])
    assert data["fail_fast_symbols"] == []
    for symbol in expected_missing:
        assert data["symbol_status"][symbol] == "missing"
        assert data["symbol_primitive_class"][symbol] == "numpy_c_api"
    assert data["primitive_class_counts"]["numpy_c_api"] >= 1
    assert "numpy_c_api" in data["symbols_by_primitive_class"]


def test_cpython_abi_variadic_shim_owns_variadic_exports() -> None:
    shim = (ROOT / "runtime/molt-cpython-abi/shims/pyarg_variadic.c").read_text()
    platform = (ROOT / "runtime/molt-cpython-abi/src/platform.rs").read_text()
    build_rs = (ROOT / "runtime/molt-cpython-abi/build.rs").read_text()
    runtime_anchor = (
        ROOT / "runtime/molt-runtime/src/c_api/cpython_abi_wasm_exports.rs"
    ).read_text()
    runtime_c_api_mod = (ROOT / "runtime/molt-runtime/src/c_api/mod.rs").read_text()
    runtime_build_rs = (ROOT / "runtime/molt-runtime/build.rs").read_text()
    variadic_exports = set(
        (ROOT / "runtime/molt-cpython-abi/shims/pyarg_variadic.exports")
        .read_text()
        .splitlines()
    )

    required_variadic_exports = {
        "PyArg_ParseTuple",
        "PyArg_ParseTupleAndKeywords",
        "PyArg_UnpackTuple",
        "Py_BuildValue",
        "PyErr_Format",
        "PyErr_FormatV",
        "PyErr_WarnFormat",
        "PyUnicode_FromFormat",
        "PyObject_CallFunction",
        "PyObject_CallFunctionObjArgs",
        "PyObject_CallMethod",
        "PyTuple_Pack",
    }
    assert "mod cpython_abi_wasm_exports;" in runtime_c_api_mod
    assert "MOLT_CPYTHON_ABI_VARIADIC_EXPORT_ANCHORS" in runtime_anchor
    assert "cargo:rustc-link-search=native=" in build_rs
    assert 'name = "molt_pyarg_shims"' not in runtime_anchor
    owns_archive_link = build_rs.split("let owns_archive_link =", 1)[1].split(";", 1)[0]
    assert 'target_arch == "wasm32"' not in owns_archive_link
    assert "molt_cpython_abi_wasm_export_anchor_count" in runtime_anchor
    assert "molt_cpython_abi_requested_exports.rs" in runtime_anchor
    assert "MOLT_WASM_CPYTHON_ABI_EXPORTS" in runtime_build_rs
    assert "MOLT_WASM_CPYTHON_ABI_DATA_EXPORTS" in runtime_build_rs
    assert "is_cpython_abi_data_symbol" not in runtime_build_rs
    assert "core::hint::black_box" in runtime_anchor
    for symbol in required_variadic_exports:
        assert f"{symbol}(" in shim
        assert symbol in variadic_exports
    assert "PyOS_snprintf(" in shim
    assert "PyOS_snprintf" in variadic_exports
    assert "vsnprintf(str, size, format, ap)" in shim
    assert "int molt_capi_write_string(const char *text, FILE *stream)" in shim
    assert "fwrite(text, 1, length, stream) == length ? 0 : EOF" in shim
    assert (
        "fn molt_capi_write_string(text: *const c_char, stream: *mut CFile)" in platform
    )
    assert "molt_capi_write_string(text, stream)" in platform
    for operation in ("malloc", "calloc", "realloc", "free"):
        assert f"molt_capi_{operation}" in shim
        assert f"molt_capi_{operation}" in platform
    assert "freestanding_alloc" not in platform
    assert "std::alloc::" not in platform
    assert 'freestanding_libc_dir = Some(provider.lib_dir("wasm32-wasip1"))' in build_rs
    assert 'println!("cargo:rustc-link-lib=static=c")' in build_rs
    assert "fputs(" not in shim
    assert "let _ = (text, stream)" not in platform


def test_cpython_abi_pyarg_format_parity_masks() -> None:
    shim = (ROOT / "runtime/molt-cpython-abi/shims/pyarg_variadic.c").read_text()
    parser = (ROOT / "runtime/molt-cpython-abi/src/api/errors.rs").read_text()

    assert "MOLT_PYARG_MAX_OUTS" not in shim
    assert "void **outs = n == 0 ? NULL : (void **)malloc" in shim
    assert "PyComplex_FromCComplex(*value)" in shim
    assert "PyUnicode_FromOrdinal(ordinal)" in shim
    for unit in ("'D'", "'Y'", "'w'", "'e'"):
        assert unit in parser
    assert "PyBUF_WRITABLE" in parser
    assert "PyObject_GetBuffer(py_ptr, view, PyBUF_WRITABLE)" in parser
    assert "PyUnicode_AsEncodedString" in parser
    assert "PyByteArray_Check" in parser


@pytest.mark.parametrize(
    ("inspection_failure", "json_output"),
    [
        (None, False),
        ("io", False),
        ("io", True),
        ("symbols", False),
        ("symbols", True),
        ("replacement", False),
        ("replacement", True),
    ],
)
def test_extension_build_emits_wheel_and_manifest(
    tmp_path: Path,
    monkeypatch,
    capsys,
    inspection_failure: str | None,
    json_output: bool,
) -> None:
    project_root = tmp_path / "extproj"
    project_root.mkdir()
    _write_extension_project(project_root)
    commands: list[list[str]] = []

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        commands.append(cmd)
        _materialize_fake_extension_command(cmd)
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli_commands, "_run_completed_command", fake_run)
    _install_extension_object_symbol_facts(
        monkeypatch,
        default_init_symbol="PyInit_demoext",
    )

    if inspection_failure is not None:
        from molt.cli.native_symbol_inspection import NativeSymbolInspectionError

        original_inspection = (
            cli_source_extensions._inspect_source_extension_artifact_symbols
        )

        def fail_artifact_inspection(
            path: Path, **kwargs: Any
        ) -> cli_source_extensions._SourceExtensionArtifactSymbolInspection | None:
            if not path.name.endswith(".molt.a"):
                return original_inspection(path, **kwargs)
            if inspection_failure == "replacement":
                inspected = original_inspection(path, **kwargs)
                path.write_bytes(static_archive_bytes() + b"replacement")
                return inspected
            if inspection_failure == "symbols":
                raise NativeSymbolInspectionError(path, ["symbol reader unavailable"])
            raise OSError("artifact read denied")

        monkeypatch.setattr(
            cli_source_extensions,
            "_inspect_source_extension_artifact_symbols",
            fail_artifact_inspection,
        )

    out_dir = project_root / "dist"
    rc = cli_commands.extension_build(
        project=str(project_root),
        out_dir=str(out_dir),
        deterministic=False,
        python_version="3.13",
        json_output=json_output,
        verbose=False,
    )
    if inspection_failure is not None:
        assert rc == 2
        captured = capsys.readouterr()
        expected = {
            "symbols": "symbol reader unavailable",
            "io": "artifact read denied",
            "replacement": "Extension artifact changed after symbol inspection",
        }[inspection_failure]
        if json_output:
            payload = json.loads(captured.out)
            assert payload["status"] == "error"
            assert expected in " ".join(payload["errors"])
        else:
            assert expected in captured.err
        assert not list(out_dir.glob("*.whl"))
        assert not (out_dir / "extension_manifest.json").exists()
        return
    assert rc == 0

    wheels = sorted(out_dir.glob("*.whl"))
    assert len(wheels) == 1
    wheel_path = wheels[0]

    manifest_path = out_dir / "extension_manifest.json"
    assert manifest_path.exists()
    manifest = json.loads(manifest_path.read_text())
    assert manifest["wheel"] == wheel_path.name
    assert manifest["molt_c_api_version"] == "2"
    assert manifest["capabilities"] == ["fs.read"]
    assert manifest["abi_tag"] == "molt_abi2"
    assert manifest["loader_kind"] == "libmolt_source"
    assert manifest["init_symbol"] == "PyInit_demoext"
    assert manifest["runtime_linkage"] == "static_link"
    assert manifest["artifact_kind"] == "static_archive"
    assert manifest["target_python"] == "py313"
    assert manifest["extension"].endswith(".molt.a")
    compile_command = next(
        command for command in commands if "-c" in command or "/c" in command
    )
    archive_command = next(command for command in commands if "rcsD" in command)
    compiler_command = manifest["build"]["compiler"]
    assert compile_command[: len(compiler_command)] == compiler_command
    assert archive_command[:1] == manifest["build"]["tool_commands"]["ar"]
    assert set(("c", "ar", "nm")) <= set(manifest["build"]["tool_commands"])

    with zipfile.ZipFile(wheel_path) as zf:
        names = set(zf.namelist())
        assert "extension_manifest.json" in names
        assert manifest["extension"] in names


def test_default_molt_c_api_version_fallback_tracks_current_contract(
    tmp_path: Path,
) -> None:
    assert (
        _default_molt_c_api_version(tmp_path / "missing-root")
        == _CURRENT_MOLT_C_API_VERSION
    )

    root = tmp_path / "bad-root"
    (root / "include" / "molt").mkdir(parents=True)
    (root / "include" / "molt" / "molt.h").write_text(
        "#define NOT_THE_VERSION 1\n",
        encoding="utf-8",
    )

    assert _default_molt_c_api_version(root) == _CURRENT_MOLT_C_API_VERSION


@pytest.mark.parametrize(
    ("config", "expected", "error"),
    [
        ({}, [], None),
        ({"python_exports": []}, [], None),
        ({"python_exports": ["pkg.z", "pkg.a", "pkg.z"]}, ["pkg.a", "pkg.z"], None),
        ({"python-exports": ["pkg.z", "pkg.a"]}, ["pkg.a", "pkg.z"], None),
        ({"python_exports": None}, [], "must be a list"),
        ({"python_exports": "pkg.a"}, [], "must be a list"),
        ({"python_exports": ["pkg..a"]}, [], "invalid Python module name"),
        ({"python_exports": [], "python-exports": ["pkg.a"]}, [], "both"),
    ],
)
def test_extension_export_configuration_is_encoded_at_producer_boundary(
    config: dict[str, object], expected: list[str], error: str | None
) -> None:
    from molt.cli.extension_commands import _extension_manifest_public_exports

    errors: list[str] = []
    exports, callables = _extension_manifest_public_exports(
        config, package="pkg", errors=errors
    )
    assert exports == expected
    assert callables == []
    if error is None:
        assert errors == []
    else:
        assert len(errors) == 1
        assert error in errors[0]


def test_extension_build_emits_public_exports_in_manifest(
    tmp_path: Path,
    monkeypatch,
) -> None:
    project_root = tmp_path / "extproj"
    project_root.mkdir()
    _write_extension_project(
        project_root,
        extension_extra_lines=[
            'python_exports = ["demoext.ndimage.distance_transform_edt"]',
            'support_files = ["demoext/ndimage/_morphology.py"]',
            "",
            "[[tool.molt.extension.callable_exports]]",
            'module = "demoext.ndimage"',
            'name = "distance_transform_edt"',
            'binding = "direct_symbol"',
            'abi = "molt.object_call_v1"',
            'symbol = "molt_demoext_ndimage_distance_transform_edt"',
            "arity = 1",
            'effects = ["read", "write"]',
            "deterministic = true",
        ],
    )
    support_source = project_root / "demoext" / "ndimage" / "_morphology.py"
    support_source.parent.mkdir(parents=True)
    support_source.write_text(
        "def distance_transform_edt(mask):\n    return mask\n",
        encoding="utf-8",
    )
    source_path = project_root / "src" / "demoext.c"
    source_path.write_text(
        source_path.read_text() + "\nstatic PyTypeObject PyLocal_Type = {0};\n"
    )

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        _materialize_fake_extension_command(cmd)
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli_commands, "_run_completed_command", fake_run)
    _install_extension_object_symbol_facts(
        monkeypatch,
        default_init_symbol="PyInit_demoext",
        by_stem={
            "demoext": (
                {
                    "PyInit_demoext",
                    "molt_demoext_ndimage_distance_transform_edt",
                },
                {"PyModule_Create2", "molt_c_api_version"},
            )
        },
    )

    out_dir = project_root / "dist"
    rc = cli_commands.extension_build(
        project=str(project_root),
        out_dir=str(out_dir),
        deterministic=False,
        json_output=False,
        verbose=False,
    )

    assert rc == 0
    manifest_path = out_dir / "extension_manifest.json"
    manifest = json.loads(manifest_path.read_text())
    assert manifest["python_exports"] == ["demoext.ndimage.distance_transform_edt"]
    expected_support_files = [
        {
            "path": "demoext/ndimage/_morphology.py",
            "sha256": hashlib.sha256(support_source.read_bytes()).hexdigest(),
        }
    ]
    assert manifest["support_files"] == expected_support_files
    expected_callable_exports = [
        {
            "module": "demoext.ndimage",
            "name": "distance_transform_edt",
            "binding": "direct_symbol",
            "abi": "molt.object_call_v1",
            "symbol": "molt_demoext_ndimage_distance_transform_edt",
            "arity": 1,
            "effects": ["read", "write"],
            "deterministic": True,
        }
    ]
    assert manifest["callable_exports"] == expected_callable_exports

    wheel_path = next(out_dir.glob("*.whl"))
    with zipfile.ZipFile(wheel_path) as zf:
        embedded = json.loads(zf.read("extension_manifest.json"))
        assert zf.read("demoext/ndimage/_morphology.py") == support_source.read_bytes()
    assert embedded["python_exports"] == manifest["python_exports"]
    assert embedded["support_files"] == expected_support_files
    assert embedded["callable_exports"] == expected_callable_exports


def test_extension_build_infers_module_attr_callable_exports_from_pymethoddef(
    tmp_path: Path,
    monkeypatch,
) -> None:
    project_root = tmp_path / "extproj"
    project_root.mkdir()
    _write_extension_project(
        project_root,
        extension_extra_lines=[
            'python_exports = ["demoext.ndimage.distance_transform_edt"]',
        ],
    )
    source_path = project_root / "src" / "demoext.c"
    source_path.write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "#include <molt/molt.h>",
                "int demoext_version(void) { return (int)molt_c_api_version(); }",
                "static PyObject *demo_distance_transform_edt(PyObject *self, PyObject *args) {",
                "    (void)self;",
                "    (void)args;",
                "    return PyLong_FromLong(1);",
                "}",
                "static PyMethodDef demoext_methods[] = {",
                '    {"distance_transform_edt", demo_distance_transform_edt, METH_VARARGS, "EDT"},',
                "    {NULL, NULL, 0, NULL},",
                "};",
                "static PyModuleDef demoext_module = {",
                "    PyModuleDef_HEAD_INIT,",
                '    "demoext",',
                "    NULL,",
                "    -1,",
                "    demoext_methods,",
                "};",
                "PyMODINIT_FUNC PyInit_demoext(void) {",
                "    return PyModule_Create(&demoext_module);",
                "}",
                "",
            ]
        ),
        encoding="utf-8",
    )

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        _materialize_fake_extension_command(cmd)
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli_commands, "_run_completed_command", fake_run)
    _install_extension_object_symbol_facts(
        monkeypatch,
        default_init_symbol="PyInit_demoext",
    )

    out_dir = project_root / "dist"
    rc = cli_commands.extension_build(
        project=str(project_root),
        out_dir=str(out_dir),
        deterministic=False,
        json_output=False,
        verbose=False,
    )

    assert rc == 0
    manifest = json.loads((out_dir / "extension_manifest.json").read_text())
    expected_callable_exports = [
        {
            "module": "demoext.ndimage",
            "name": "distance_transform_edt",
            "binding": "module_attr",
            "abi": "molt.object_callargs_v1",
            "effects": [],
            "deterministic": False,
        }
    ]
    assert manifest["python_exports"] == ["demoext.ndimage.distance_transform_edt"]
    assert manifest["callable_exports"] == expected_callable_exports
    wheel_path = next(out_dir.glob("*.whl"))
    with zipfile.ZipFile(wheel_path) as zf:
        embedded = json.loads(zf.read("extension_manifest.json"))
    assert embedded["callable_exports"] == expected_callable_exports


@pytest.mark.slow
def test_extension_build_compiles_iterator_mapping_surface_without_subprocess_mock(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    plan = cli_source_extension_target.resolve_source_extension_target_plan("native")
    if not cli_llvm_wasi_tools.llvm_tool_candidates(
        "cc", target_triple=plan.target_triple
    ):
        pytest.skip(
            f"native LLVM compiler for {plan.target_triple} is required for real extension smoke"
        )
    monkeypatch.setenv("CL", "/DUNRECORDED=1 /FIunowned-missing-header.h")
    monkeypatch.setenv("_CL_", "/Fo" + str(tmp_path / "unowned.obj"))
    monkeypatch.setenv("CCC_OVERRIDE_OPTIONS", "+-include;unowned-missing-header.h")
    project_root = tmp_path / "iter_mapping_ext"
    project_root.mkdir()
    _write_extension_iterator_mapping_project(project_root)

    out_dir = project_root / "dist"
    rc = cli_commands.extension_build(
        project=str(project_root),
        out_dir=str(out_dir),
        deterministic=False,
        json_output=False,
        verbose=False,
    )

    assert rc == 0
    wheels = sorted(out_dir.glob("*.whl"))
    assert len(wheels) == 1
    manifest = json.loads((out_dir / "extension_manifest.json").read_text())
    assert manifest["module"] == "demoext_iter"
    assert manifest["capabilities"] == ["fs.read"]
    with zipfile.ZipFile(wheels[0]) as zf:
        names = set(zf.namelist())
        assert "extension_manifest.json" in names
        assert manifest["extension"] in names
        assert zf.read(manifest["extension"])


def test_extension_build_cross_target_uses_target_compiler_and_manifest(
    tmp_path: Path, monkeypatch
) -> None:
    project_root = tmp_path / "extproj"
    project_root.mkdir()
    _write_extension_project(project_root)
    commands: list[list[str]] = []
    compiler = tmp_path / "toolchain" / "cross-clang"
    compiler.parent.mkdir()
    compiler.write_bytes(b"compiler")
    monkeypatch.setenv(
        "MOLT_CROSS_CC",
        f"{compiler} --driver-mode=gcc",
    )
    monkeypatch.delenv("MOLT_CROSS_CXX", raising=False)

    def resolve_family(
        *,
        target_family: cli_llvm_wasi_tools.LlvmTargetFamily,
        explicit_commands: dict[cli_llvm_wasi_tools.LlvmToolRole, tuple[str, ...]],
        sibling_directories: tuple[Path, ...],
        environment: object,
    ) -> cli_llvm_wasi_tools.LlvmWasiToolFamily:
        del sibling_directories, environment
        assert target_family == "native"
        return cli_llvm_wasi_tools.LlvmWasiToolFamily(
            cc=_resolved_llvm_tool("cc", explicit_commands["cc"]),
            cxx=None,
            wasm_ld=None,
            ar=_resolved_llvm_tool("ar", ("/tools/llvm-ar",)),
            ranlib=None,
            nm=_resolved_llvm_tool("nm", ("/tools/llvm-nm",)),
            strip=None,
        )

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        commands.append(cmd)
        _materialize_fake_extension_command(cmd)
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(
        cli_source_extension_toolchain,
        "resolve_llvm_wasi_tool_family",
        resolve_family,
    )
    monkeypatch.setattr(cli_commands, "_run_completed_command", fake_run)
    _install_extension_object_symbol_facts(
        monkeypatch,
        default_init_symbol="PyInit_demoext",
    )

    out_dir = project_root / "dist"
    target = "aarch64-unknown-linux-gnu"
    rc = cli_commands.extension_build(
        project=str(project_root),
        out_dir=str(out_dir),
        deterministic=False,
        target=target,
        json_output=False,
        verbose=False,
    )
    assert rc == 0
    compile_command = next(command for command in commands if "-c" in command)
    expected_compiler = [
        str(compiler.resolve()),
        "--driver-mode=gcc",
        "-target",
        target,
    ]
    assert compile_command[: len(expected_compiler)] == expected_compiler
    assert any("rcsD" in command for command in commands)
    manifest = json.loads((out_dir / "extension_manifest.json").read_text())
    assert manifest["target_triple"] == target
    assert manifest["runtime_linkage"] == "static_link"
    assert manifest["artifact_kind"] == "static_archive"
    assert manifest["build"]["compiler"] == expected_compiler


def test_extension_build_consumes_meson_source_plan_object_closure(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project_root = tmp_path / "meson_extproj"
    project_root.mkdir()
    intro_path = _write_meson_source_plan_project(project_root)
    commands: list[list[str]] = []

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        commands.append(cmd)
        _materialize_fake_extension_command(cmd)
        _write_fake_compiler_depfile(cmd, project_root / "pkg/include/demoext.h")
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli_commands, "_run_completed_command", fake_run)
    _install_extension_object_symbol_facts(
        monkeypatch,
        default_init_symbol="PyInit_demoext",
        by_stem={
            "demoext": (
                {"PyInit_demoext"},
                {"PyModule_Create2", "PyTuple_New", "helper_generated"},
            ),
            "helper_generated": (
                {"helper_generated"},
                {"PyLong_AsLong", "PyLong_FromLong"},
            ),
        },
    )
    out_dir = project_root / "dist"
    rc = cli_commands.extension_build(
        project=str(project_root),
        out_dir=str(out_dir),
        deterministic=False,
        json_output=False,
        verbose=False,
    )

    assert rc == 0
    compile_cmd = next(
        cmd
        for cmd in commands
        if ("-c" in cmd or "/c" in cmd) and any("demoext.c" in part for part in cmd)
    )
    include_dirs = [
        Path(compile_cmd[idx + 1]).resolve()
        for idx, token in enumerate(compile_cmd[:-1])
        if token in {"-I", "/I"}
    ]
    assert include_dirs.index(
        (ROOT / "include" / "molt").resolve()
    ) < include_dirs.index(project_root.resolve())
    assert include_dirs.index(project_root.resolve()) < include_dirs.index(
        (project_root / "pkg" / "include").resolve()
    )
    assert include_dirs.index(
        (project_root / "pkg" / "include").resolve()
    ) < include_dirs.index((ROOT / "include").resolve())
    archive_cmd = next(cmd for cmd in commands if "rcsD" in cmd)
    assert Path(archive_cmd[archive_cmd.index("rcsD") + 1]).name == "demoext.molt.a"
    object_suffix = ".obj" if cli_commands.sys.platform == "win32" else ".o"
    assert any(part.endswith("0_demoext" + object_suffix) for part in archive_cmd)
    assert any(
        part.endswith("1_helper_generated" + object_suffix) for part in archive_cmd
    )
    manifest = json.loads((out_dir / "extension_manifest.json").read_text())
    assert manifest["source_plan"]["kind"] == "meson-intro-targets"
    assert manifest["source_plan"]["plan"] == str(intro_path.resolve())
    assert manifest["source_plan"]["compile_commands"] == str(
        (project_root / "build" / "compile_commands.json").resolve()
    )
    assert manifest["source_plan"]["digest"]
    assert manifest["build"]["source_plan_digest"] == manifest["source_plan"]["digest"]
    assert manifest["source_plan"]["skipped_generated_sources"] == []
    assert manifest["build"]["source_plan_skipped_generated_source_count"] == 0
    assert manifest["build"]["object_count"] == 2
    assert manifest["build"]["linked_object_count"] == 2
    for obj in manifest["object_closure"]["objects"]:
        assert obj["language"] == "c"
        assert Path(obj["source"]).suffix == ""
        command = obj["compile_command"]
        if cli_commands.sys.platform == "win32":
            assert command[command.index("/c") - 1] == "/TC"
        else:
            language_index = command.index("-x")
            assert command[language_index + 1] == "c"
            assert language_index < command.index("-c")
    assert manifest["build"]["source_c_api_scan"][
        "project_generated_c_api_prefixes"
    ] == ["npy_generated_"]
    assert (
        manifest["build"]["source_c_api_scan"]["project_generated_c_api_symbols"] == []
    )
    assert manifest["object_closure"]["root_symbol"] == "PyInit_demoext"
    assert (
        manifest["object_closure"]["init_symbol_owner"] == "0_demoext" + object_suffix
    )
    assert manifest["object_closure"]["closure_sha256"]
    assert manifest["object_closure"]["required_capsules"] == []
    assert manifest["object_closure"]["project_generated_c_api_prefixes"] == [
        "npy_generated_"
    ]
    assert manifest["provided_capsules"] == []
    required_c_api_symbols = {
        symbol
        for obj in manifest["object_closure"]["objects"]
        for symbol in obj["required_c_api_symbols"]
    }
    project_generated_c_api_symbols = {
        symbol
        for obj in manifest["object_closure"]["objects"]
        for symbol in obj["project_generated_c_api_symbols"]
    }
    # This textual reference did not survive compilation; it is not a requirement.
    assert "npy_generated_int8" not in project_generated_c_api_symbols
    assert "npy_generated_int8" not in required_c_api_symbols
    assert "NPY_HEADER_ONLY_MACRO" not in required_c_api_symbols
    assert "NPY_DISABLED_Alias" not in required_c_api_symbols
    assert "NPY_DISABLED_SVE" not in required_c_api_symbols
    assert "PyCode_NewWithPosOnlyArgs" not in required_c_api_symbols
    assert "_PyLong_FormatAdvancedWriter" not in required_c_api_symbols
    assert "_PyFloat_FormatAdvancedWriter" not in required_c_api_symbols
    assert "PyUnicode_CopyCharacters" not in required_c_api_symbols
    assert manifest["build"]["source_c_api_scan"]["missing_symbols"] == []
    assert "PyTuple_New" in required_c_api_symbols
    assert (out_dir / "pkg" / "__init__.py").read_text(
        encoding="utf-8"
    ) == "VALUE = 1\n"
    artifact_path = out_dir / manifest["extension"]
    artifact_manifest = json.loads(
        artifact_path.with_name(
            artifact_path.name + ".extension_manifest.json"
        ).read_text(encoding="utf-8")
    )
    assert (
        artifact_manifest["source_plan"]["digest"] == manifest["source_plan"]["digest"]
    )
    assert artifact_manifest["python_exports"] == ["pkg.demoext"]


def test_direct_build_audits_and_reseals_extracted_wheel(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys
) -> None:
    from molt.cli import source_extension_producer as producer
    from molt.target_python import _parse_target_python_version

    project = tmp_path / "project"
    project.mkdir()
    _write_meson_source_plan_project(project)
    compile_database = project / "build" / "compile_commands.json"
    compile_rows = json.loads(compile_database.read_text())
    for row in compile_rows:
        row["arguments"].append("--target=wasm32-wasip1")
    compile_database.write_text(json.dumps(compile_rows), encoding="utf-8")
    wasm_bytes = _wasm_exporting_i64_unary_symbols(
        ("PyInit_demoext", "helper_generated")
    )

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        output_path = Path(cmd[cmd.index("-o") + 1])
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_bytes(wasm_bytes)
        _write_fake_compiler_depfile(cmd)
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli_commands, "_run_completed_command", fake_run)
    sysroot = _write_fake_wasi_sysroot(tmp_path)
    monkeypatch.setattr(cli_commands, "resolve_wasi_sysroot", lambda: sysroot)
    # This is a producer/wheel custody test, not a host compiler-discovery test.
    # Bind the same typed tool family the real producer consumes, then retain
    # the real target command construction and mocked compiler byte outputs.
    tools = cli_llvm_wasi_tools.LlvmWasiToolFamily(
        cc=_resolved_llvm_tool("cc", (str(tmp_path / "toolchain/bin/clang"),)),
        cxx=_resolved_llvm_tool("cxx", (str(tmp_path / "toolchain/bin/clang++"),)),
        wasm_ld=_resolved_llvm_tool(
            "wasm_ld", (str(tmp_path / "toolchain/bin/wasm-ld"),)
        ),
        ar=_resolved_llvm_tool("ar", (str(tmp_path / "toolchain/bin/llvm-ar"),)),
        ranlib=_resolved_llvm_tool(
            "ranlib", (str(tmp_path / "toolchain/bin/llvm-ranlib"),)
        ),
        nm=_resolved_llvm_tool("nm", (str(tmp_path / "toolchain/bin/llvm-nm"),)),
        strip=_resolved_llvm_tool(
            "strip", (str(tmp_path / "toolchain/bin/llvm-strip"),)
        ),
    )
    monkeypatch.setattr(
        cli_source_extension_toolchain,
        "_resolve_source_extension_wasm_toolchain",
        lambda _target, *, environment: (
            cli_source_extension_toolchain._SourceExtensionWasmToolchain(
                ok=True,
                compiler_kind="clang",
                tools=tools,
                wasi_sysroot=sysroot,
                detail="deterministic producer fixture tool family",
            )
        ),
    )
    _install_extension_object_symbol_facts(
        monkeypatch,
        default_init_symbol="PyInit_demoext",
        by_stem={
            "demoext": ({"PyInit_demoext"}, {"helper_generated"}),
            "helper_generated": ({"helper_generated"}, set()),
        },
    )
    output = project / "dist"
    assert (
        cli_commands.extension_build(
            project=str(project),
            out_dir=str(output),
            abi_tier="cpython-abi",
            target="wasm",
            molt_abi=_default_molt_c_api_version(ROOT),
            python_version="3.12",
            deterministic=True,
        )
        == 0
    )
    capsys.readouterr()
    manifest = json.loads((output / "extension_manifest.json").read_text())
    produced = producer._audit_extension_output(
        output_root=output,
        module="pkg.demoext",
        source_plan_target="pkg.demoext",
        expected_target_triple=manifest["target_triple"],
        expected_target_python=_parse_target_python_version("3.12"),
        expected_package_version="0.1.0",
        python_exports=["pkg.demoext"],
        capabilities=["fs.read"],
        provided_capsules=[],
    )
    # An earlier ancestor deliberately repeats the package name. Only the
    # explicit module-relative artifact placement owns the extracted root.
    extracted = tmp_path / "pkg" / "extracted"
    with zipfile.ZipFile(produced.wheel_path) as wheel:
        embedded = json.loads(wheel.read("extension_manifest.json"))
        assert embedded["extension_sha256"] == manifest["extension_sha256"]
        assert embedded["runtime_python_import_modules"] == []
        assert (
            wheel.read("pkg/__init__.py")
            == (project / "pkg" / "__init__.py").read_bytes()
        )
        wheel.extractall(extracted)
    # Invalidate every original input and both original output views; all
    # subsequent reads must come from the extracted wheel.
    for item in manifest["object_closure"]["objects"]:
        (output / item["source"]).unlink(missing_ok=True)
    for source in (
        project / "pkg" / "demoext.c",
        project / "build" / "generated" / "helper_generated.c",
    ):
        source.unlink(missing_ok=True)
    resealed = tmp_path / "resealed"
    assert (
        cli.extension_seal(
            path=str(extracted / "extension_manifest.json"),
            out_dir=str(resealed),
            json_output=True,
        )
        == 0
    )
    assert json.loads(capsys.readouterr().out)["status"] == "ok"
    sealed = json.loads((resealed / "extension_manifest.json").read_text())
    assert sealed["extension_sha256"] == embedded["extension_sha256"]
    assert (
        sealed["runtime_python_import_modules"]
        == embedded["runtime_python_import_modules"]
    )
    assert (resealed / "pkg" / "__init__.py").read_bytes() == (
        extracted / "pkg" / "__init__.py"
    ).read_bytes()


def test_extension_build_threads_source_plan_roots_to_cython_regeneration(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project_root = tmp_path / "cython_extproj"
    project_root.mkdir()
    pkg = project_root / "pkg"
    build_root = project_root / "build"
    meson_info = build_root / "meson-info"
    pkg.mkdir(parents=True)
    meson_info.mkdir(parents=True)
    (pkg / "__init__.py").write_text("", encoding="utf-8")
    (pkg / "_cyext.pyx").write_text("cdef int value = 1\n", encoding="utf-8")
    (pkg / "_cyext.c").write_text("/* upstream generated C */\n", encoding="utf-8")
    (meson_info / "intro-targets.json").write_text(
        json.dumps(
            [
                {
                    "id": "pkg._cyext",
                    "name": "_cyext",
                    "type": "shared module",
                    "filename": str(build_root / "pkg" / "_cyext.so"),
                    "target_sources": [
                        {
                            "language": "c",
                            "machine": "host",
                            "parameters": [],
                            "sources": ["pkg/_cyext.pyx", "pkg/_cyext.c"],
                            "generated_sources": [],
                        }
                    ],
                }
            ],
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    (build_root / "compile_commands.json").write_text(
        json.dumps(
            [
                {
                    "directory": str(project_root),
                    "file": "pkg/_cyext.c",
                    "arguments": [
                        "cc",
                        "-c",
                        "pkg/_cyext.c",
                        "-DPLAN_UNIT=1",
                        "-o",
                        "build/pkg/_cyext.so.p/_cyext.o",
                    ],
                }
            ],
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    (project_root / "pyproject.toml").write_text(
        "\n".join(
            [
                "[project]",
                'name = "cython-ext"',
                'version = "0.1.0"',
                "",
                "[tool.molt.extension]",
                'module = "pkg._cyext"',
                'capabilities = ["fs.read"]',
                'molt_c_api_version = "1"',
                'python_exports = ["pkg._cyext"]',
                "",
                "[tool.molt.extension.source_plan]",
                'kind = "meson-intro-targets"',
                'path = "build/meson-info/intro-targets.json"',
                'target = "pkg._cyext"',
                'source_root = "."',
                'build_root = "build"',
                "",
            ]
        ),
        encoding="utf-8",
    )
    commands: list[list[str]] = []
    observed_package_roots: list[tuple[Path, ...]] = []

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        commands.append(cmd)
        _materialize_fake_extension_command(cmd)
        return subprocess.CompletedProcess(cmd, 0, "", "")

    def fake_regenerate(
        *,
        pyx_path: Path,
        original_c: Path,
        language: cli_commands._source_extension_cython.SourceExtensionLanguage,
        out_dir: Path,
        include_dirs: object,
        cython_version: str,
        python_exe: str | None = None,
        package_roots: object = (),
        ninja_command: object = (),
    ) -> tuple[cli_commands._source_extension_cython.CythonRegeneration, None]:
        del include_dirs, cython_version, python_exe, ninja_command
        assert (
            language is cli_commands._source_extension_cython.SourceExtensionLanguage.C
        )
        observed_package_roots.append(
            tuple(Path(path).resolve() for path in package_roots)
        )
        regenerated_c = out_dir / original_c.name
        regenerated_c.parent.mkdir(parents=True, exist_ok=True)
        regenerated_c.write_text(
            "#include <Python.h>\n"
            'static PyModuleDef module = {PyModuleDef_HEAD_INIT, "_cyext", NULL, -1, NULL};\n'
            "PyMODINIT_FUNC PyInit__cyext(void) { return PyModule_Create(&module); }\n",
            encoding="utf-8",
        )
        return (
            cli_commands._source_extension_cython.CythonRegeneration(
                pyx_path=pyx_path.resolve(),
                original_c=original_c.resolve(),
                regenerated_c=regenerated_c.resolve(),
                cython_version="test",
                cython_argv=("python", "-m", "cython", "-3"),
                cimport_packages=("widget",),
                cimport_pxd_roots=(project_root.resolve(),),
                cimport_header_include_dirs=((project_root / "pkg").resolve(),),
            ),
            None,
        )

    monkeypatch.setattr(cli_commands, "_run_completed_command", fake_run)
    monkeypatch.setattr(
        cli_commands._source_extension_cython,
        "provision_cython",
        lambda *, python_exe, requirement: ("test", None),
    )
    monkeypatch.setattr(
        cli_commands._source_extension_cython,
        "regenerate_cython_c_standalone",
        fake_regenerate,
    )
    _install_extension_object_symbol_facts(
        monkeypatch,
        default_init_symbol="PyInit__cyext",
        by_stem={"_cyext": ({"PyInit__cyext"}, {"PyModule_Create2"})},
    )
    out_dir = project_root / "dist"
    rc = cli_commands.extension_build(
        project=str(project_root),
        out_dir=str(out_dir),
        abi_tier="cpython-abi",
        deterministic=False,
        json_output=False,
        verbose=False,
    )

    assert rc == 0
    assert observed_package_roots == [(project_root.resolve(), build_root.resolve())]
    regenerated_compile_command = next(
        cmd
        for cmd in commands
        if ("-c" in cmd or "/c" in cmd)
        and any("molt_cython_standalone" in part and "_cyext.c" in part for part in cmd)
    )
    profile_args = cli_commands._source_extension_cython.CYTHON_CPYTHON_ABI_COMPILE_ARGS
    frontend_args = [arg.removeprefix("/clang:") for arg in regenerated_compile_command]
    assert all(arg in frontend_args for arg in profile_args)
    assert frontend_args.index(profile_args[0]) < (frontend_args.index("-DPLAN_UNIT=1"))
    assert all(
        sum(arg in [token.removeprefix("/clang:") for token in cmd] for cmd in commands)
        == 1
        for arg in profile_args
    )
    manifest = json.loads((out_dir / "extension_manifest.json").read_text())
    assert manifest["cython_standalone"][0]["cimport_pxd_roots"] == [
        str(project_root.resolve())
    ]
    assert manifest["cython_standalone"][0]["cimport_header_include_dirs"] == [
        str((project_root / "pkg").resolve())
    ]
    assert manifest["cython_standalone"][0]["compile_profile"] == (
        "molt-cpython-abi-safe-v1"
    )
    assert manifest["cython_standalone"][0]["compile_args"] == list(profile_args)


def test_extension_build_derives_module_attr_support_source_closure(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project_root = tmp_path / "meson_extproj"
    project_root.mkdir()
    _write_meson_source_plan_project(project_root)
    ndimage_dir = project_root / "pkg" / "ndimage"
    ndimage_dir.mkdir()
    (ndimage_dir / "_filters.py").write_text(
        "from . import _nd_image\n"
        "from . import _ni_docstrings\n"
        "from . import _ni_support\n"
        # Runtime imports may mutate module globals, including TYPE_CHECKING.
        # This fixture needs a proven dead branch, not a spelling assumption.
        "if False:\n"
        "    from . import _dead_support\n"
        "def gaussian_filter(value):\n"
        "    _ni_docstrings.docfiller(gaussian_filter)\n"
        "    return _ni_support.normalize(value)\n",
        encoding="utf-8",
    )
    (ndimage_dir / "_ni_docstrings.py").write_text(
        "docfiller = lambda func: func\n",
        encoding="utf-8",
    )
    (ndimage_dir / "_ni_support.py").write_text(
        "def normalize(value):\n    return value\n",
        encoding="utf-8",
    )
    (ndimage_dir / "_dead_support.py").write_text(
        "def unused(value):\n    return value\n",
        encoding="utf-8",
    )
    commands: list[list[str]] = []

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        commands.append(cmd)
        _materialize_fake_extension_command(cmd)
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli_commands, "_run_completed_command", fake_run)
    _install_extension_object_symbol_facts(
        monkeypatch,
        default_init_symbol="PyInit_demoext",
        by_stem={
            "demoext": ({"PyInit_demoext"}, {"helper_generated"}),
            "helper_generated": ({"helper_generated"}, set()),
        },
        linked_stems=("demoext", "helper_generated"),
    )
    out_dir = project_root / "dist"
    rc = cli_commands.extension_build(
        project=str(project_root),
        out_dir=str(out_dir),
        callable_export_json=[
            json.dumps(
                {
                    "module": "pkg.ndimage",
                    "name": "gaussian_filter",
                    "binding": "module_attr",
                    "provider_module": "pkg.ndimage._filters",
                    "abi": "molt.object_callargs_v1",
                }
            )
        ],
        deterministic=False,
        json_output=False,
        verbose=False,
    )

    assert rc == 0
    manifest = json.loads((out_dir / "extension_manifest.json").read_text())
    assert manifest["support_files"] == [
        {
            "path": "pkg/ndimage/_filters.py",
            "sha256": hashlib.sha256(
                (ndimage_dir / "_filters.py").read_bytes()
            ).hexdigest(),
        },
        {
            "path": "pkg/ndimage/_ni_docstrings.py",
            "sha256": hashlib.sha256(
                (ndimage_dir / "_ni_docstrings.py").read_bytes()
            ).hexdigest(),
        },
        {
            "path": "pkg/ndimage/_ni_support.py",
            "sha256": hashlib.sha256(
                (ndimage_dir / "_ni_support.py").read_bytes()
            ).hexdigest(),
        },
    ]
    assert (out_dir / "pkg" / "ndimage" / "_filters.py").is_file()
    assert (out_dir / "pkg" / "ndimage" / "_ni_docstrings.py").is_file()
    assert (out_dir / "pkg" / "ndimage" / "_ni_support.py").is_file()
    assert not (out_dir / "pkg" / "ndimage" / "_dead_support.py").exists()


@pytest.mark.parametrize("static_library_suffix", [".a", ".lib"])
@pytest.mark.parametrize("nested_linker", [False, True])
def test_extension_build_follows_linked_static_library_source_closure(
    tmp_path: Path,
    static_library_suffix: str,
    nested_linker: bool,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project_root = tmp_path / "meson_extproj"
    project_root.mkdir()
    _write_meson_source_plan_project(
        project_root,
        linked_static_library=True,
        static_library_suffix=static_library_suffix,
        nested_linker=nested_linker,
    )
    commands: list[list[str]] = []

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        commands.append(cmd)
        _materialize_fake_extension_command(cmd)
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli_commands, "_run_completed_command", fake_run)
    _install_extension_object_symbol_facts(
        monkeypatch,
        default_init_symbol="PyInit_demoext",
        by_stem={
            "demoext": (
                {"PyInit_demoext"},
                {"array__unique_hash", "helper_generated"},
            ),
            "helper_generated": ({"helper_generated"}, set()),
            "unique": ({"array__unique_hash"}, set()),
        },
    )
    out_dir = project_root / "dist"
    rc = cli_commands.extension_build(
        project=str(project_root),
        out_dir=str(out_dir),
        deterministic=False,
        json_output=False,
        verbose=False,
    )
    captured = capsys.readouterr()

    assert rc == 0
    skipped_generated_source = (
        project_root / "build" / "generated" / "cleaned_unique.c"
    ).resolve()
    assert (
        "Warning: source_plan skipped 1 cleaned generated source absent from disk"
        in captured.err
    )
    assert str(skipped_generated_source) in captured.err
    assert any(
        ("-c" in cmd or "/c" in cmd) and any("unique.cpp" in part for part in cmd)
        for cmd in commands
    )
    archive_cmd = next(cmd for cmd in commands if "rcsD" in cmd)
    object_suffix = ".obj" if cli_commands.sys.platform == "win32" else ".o"
    assert any(part.endswith("2_unique" + object_suffix) for part in archive_cmd)
    manifest = json.loads((out_dir / "extension_manifest.json").read_text())
    assert manifest["build"]["object_count"] == 3
    assert manifest["build"]["linked_object_count"] == 3
    assert manifest["source_plan"]["skipped_generated_sources"] == [
        str(skipped_generated_source)
    ]
    assert manifest["build"]["source_plan_skipped_generated_source_count"] == 1
    assert (
        str((project_root / "pkg" / "unique.cpp").resolve())
        in (manifest["source_plan"]["sources"])
    )
    object_sources = {
        (out_dir / obj["source"]).read_bytes()
        for obj in manifest["object_closure"]["objects"]
    }
    assert (project_root / "pkg" / "unique.cpp").read_bytes() in object_sources
    defined_symbols = {
        symbol
        for obj in manifest["object_closure"]["objects"]
        for symbol in obj["defined_symbols"]
    }
    assert "array__unique_hash" in defined_symbols


@pytest.mark.parametrize("static_library_suffix", [".a", ".lib"])
@pytest.mark.parametrize("nested_linker", [False, True])
def test_extension_build_excludes_linked_static_library(
    tmp_path: Path,
    static_library_suffix: str,
    nested_linker: bool,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """``--source-plan-exclude-linked-static-library`` drops the archive's TUs.

    Mirrors the numpy multi-extension case: ``_umath_linalg`` links
    ``libnpymath.a``, which the PRIMARY ``_multiarray_umath`` extension already
    statically exports. Excluding the linked archive keeps its symbols undefined
    (resolved against the primary at final link) instead of compiling a second
    colliding copy into this extension's closure.
    """
    project_root = tmp_path / "meson_extproj"
    project_root.mkdir()
    _write_meson_source_plan_project(
        project_root,
        linked_static_library=True,
        static_library_suffix=static_library_suffix,
        nested_linker=nested_linker,
    )
    (project_root / "pkg" / ("libunique_hash" + static_library_suffix)).write_bytes(
        static_archive_bytes(b"unique-hash")
    )
    commands: list[list[str]] = []

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        commands.append(cmd)
        _materialize_fake_extension_command(cmd)
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli_commands, "_run_completed_command", fake_run)
    _install_extension_object_symbol_facts(
        monkeypatch,
        default_init_symbol="PyInit_demoext",
        by_stem={
            "demoext": (
                {"PyInit_demoext"},
                {"array__unique_hash", "helper_generated"},
            ),
            "helper_generated": ({"helper_generated"}, set()),
            "unique": ({"array__unique_hash"}, set()),
        },
        linked_stems=("demoext", "helper_generated"),
    )
    out_dir = project_root / "dist"
    rc = cli_commands.extension_build(
        project=str(project_root),
        out_dir=str(out_dir),
        source_plan_exclude_linked_static_libraries=[
            "libunique_hash" + static_library_suffix
        ],
        deterministic=False,
        json_output=False,
        verbose=False,
    )
    assert rc == 0

    # The excluded archive's translation unit is neither compiled nor linked.
    assert not any(
        ("-c" in cmd or "/c" in cmd) and any("unique.cpp" in part for part in cmd)
        for cmd in commands
    )
    manifest = json.loads((out_dir / "extension_manifest.json").read_text())
    object_sources = {
        (out_dir / obj["source"]).read_bytes()
        for obj in manifest["object_closure"]["objects"]
    }
    assert (project_root / "pkg" / "unique.cpp").read_bytes() not in object_sources
    assert (
        str((project_root / "pkg" / "unique.cpp").resolve())
        not in (manifest["source_plan"]["sources"])
    )
    # Its symbol is now undefined in this closure (resolved from the primary at
    # final link), not redundantly defined here.
    defined_symbols = {
        symbol
        for obj in manifest["object_closure"]["objects"]
        for symbol in obj["defined_symbols"]
    }
    assert "array__unique_hash" not in defined_symbols


@pytest.mark.parametrize("static_library_suffix", [".a", ".lib"])
@pytest.mark.parametrize("nested_linker", [False, True])
def test_extension_build_follows_meson_aggregate_static_library_members(
    tmp_path: Path,
    static_library_suffix: str,
    nested_linker: bool,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project_root = tmp_path / "meson_extproj"
    project_root.mkdir()
    _write_meson_source_plan_project(
        project_root,
        aggregate_static_library=True,
        static_library_suffix=static_library_suffix,
        nested_linker=nested_linker,
    )
    # This fixture supplies a synthetic Ninja archive graph, not a runnable
    # generator. Model its non-Cython generated-C recipe at the process boundary;
    # source folding, object closure and archive membership remain real.
    monkeypatch.setattr(
        cli_commands._source_extension_cython,
        "_query_ninja_generator_commands",
        lambda **_kwargs: ("python generate_sources.py", None),
    )
    commands: list[list[str]] = []

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        commands.append(cmd)
        _materialize_fake_extension_command(cmd)
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli_commands, "_run_completed_command", fake_run)
    _install_extension_object_symbol_facts(
        monkeypatch,
        default_init_symbol="PyInit_demoext",
        by_stem={
            "demoext": (
                {"PyInit_demoext"},
                {"FLOAT_add_indexed", "helper_generated"},
            ),
            "helper_generated": ({"helper_generated"}, set()),
            "loops_arithmetic.dispatch": ({"FLOAT_add_indexed"}, set()),
            "simd.dispatch": ({"SIMD_not_linked"}, set()),
        },
        linked_stems=(
            "demoext",
            "helper_generated",
            "loops_arithmetic.dispatch",
        ),
    )
    out_dir = project_root / "dist"
    rc = cli_commands.extension_build(
        project=str(project_root),
        out_dir=str(out_dir),
        deterministic=False,
        json_output=False,
        verbose=False,
    )

    assert rc == 0
    assert any(
        ("-c" in cmd or "/c" in cmd)
        and any("loops_arithmetic.dispatch.c" in part for part in cmd)
        for cmd in commands
    )
    assert not any(
        ("-c" in cmd or "/c" in cmd) and any("simd.dispatch.c" in part for part in cmd)
        for cmd in commands
    )
    manifest = json.loads((out_dir / "extension_manifest.json").read_text())
    assert manifest["build"]["object_count"] == 3
    assert manifest["build"]["linked_object_count"] == 3
    assert (
        str(
            (
                project_root / "build" / "generated" / "loops_arithmetic.dispatch.c"
            ).resolve()
        )
        in manifest["source_plan"]["generated_sources"]
    )
    assert (
        str((project_root / "build" / "generated" / "simd.dispatch.c").resolve())
        not in manifest["source_plan"]["generated_sources"]
    )
    object_sources = {
        (out_dir / obj["source"]).read_bytes()
        for obj in manifest["object_closure"]["objects"]
    }
    assert (
        project_root / "build" / "generated" / "loops_arithmetic.dispatch.c"
    ).read_bytes() in object_sources
    defined_symbols = {
        symbol
        for obj in manifest["object_closure"]["objects"]
        for symbol in obj["defined_symbols"]
    }
    assert "FLOAT_add_indexed" in defined_symbols
    assert "FLOAT_add_indexed" not in manifest["object_closure"]["runtime_symbols"]


def test_extension_build_rejects_parallel_sources_with_source_plan(
    tmp_path: Path,
    capsys,
) -> None:
    project_root = tmp_path / "meson_extproj"
    project_root.mkdir()
    _write_meson_source_plan_project(project_root)
    pyproject = project_root / "pyproject.toml"
    pyproject.write_text(
        pyproject.read_text(encoding="utf-8").replace(
            'molt_c_api_version = "1"',
            'molt_c_api_version = "1"\nsources = ["pkg/demoext.c"]',
        ),
        encoding="utf-8",
    )

    rc = cli_commands.extension_build(
        project=str(project_root),
        out_dir=str(project_root / "dist"),
        deterministic=False,
        json_output=False,
        verbose=False,
    )

    assert rc == 2
    assert "source_plan plus compile_commands.json is the source/arg authority" in (
        capsys.readouterr().err
    )


def test_extension_metadata_parser_surface() -> None:
    parser = cli_entrypoint_parser._build_entrypoint_parser()
    args = parser.parse_args(
        [
            "extension",
            "metadata",
            "--target",
            "wasm32-wasip1",
            "--out-dir",
            "dist/meta",
            "--python-version",
            "3.12",
            "--abi-tier",
            "cpython-abi",
            "--json",
        ]
    )
    assert args.command == "extension"
    assert args.extension_command == "metadata"
    assert args.target == "wasm32-wasip1"
    assert args.out_dir == "dist/meta"
    assert args.python_version == "3.12"
    assert args.abi_tier == "cpython-abi"
    assert args.json is True


def test_extension_build_export_custody_parser_surface() -> None:
    parser = cli_entrypoint_parser._build_entrypoint_parser()
    args = parser.parse_args(
        [
            "extension",
            "build",
            "--module",
            "numpy._core._multiarray_umath",
            "--source-plan",
            "build/meson-info/intro-targets.json",
            "--python-export",
            "numpy",
            "--provided-capsules",
            "numpy._core._multiarray_umath._ARRAY_API",
            "--callable-export-json",
            '{"module":"numpy._core","name":"probe","binding":"module_attr","abi":"molt.object_callargs_v1"}',
            "--support-file",
            "numpy/_core/_multiarray_umath.py",
        ]
    )
    assert args.command == "extension"
    assert args.extension_command == "build"
    assert args.module == "numpy._core._multiarray_umath"
    assert args.source_plan == "build/meson-info/intro-targets.json"
    assert args.python_export == ["numpy"]
    assert args.provided_capsules == ["numpy._core._multiarray_umath._ARRAY_API"]
    assert args.callable_export_json == [
        '{"module":"numpy._core","name":"probe","binding":"module_attr","abi":"molt.object_callargs_v1"}'
    ]
    assert args.support_file == ["numpy/_core/_multiarray_umath.py"]


def test_extension_seal_parser_surface() -> None:
    parser = cli_entrypoint_parser._build_entrypoint_parser()
    args = parser.parse_args(
        [
            "extension",
            "seal",
            "--path",
            "dist/extension_manifest.json",
            "--out-dir",
            "dist/sealed",
            "--python-export",
            "numpy",
            "--callable-export-json",
            '{"module":"numpy._core","name":"probe","binding":"module_attr","abi":"molt.object_callargs_v1"}',
            "--support-file",
            "numpy/_core/_multiarray_umath.py",
            "--json",
        ]
    )
    assert args.command == "extension"
    assert args.extension_command == "seal"
    assert args.path == "dist/extension_manifest.json"
    assert args.out_dir == "dist/sealed"
    assert args.python_export == ["numpy"]
    assert args.callable_export_json == [
        '{"module":"numpy._core","name":"probe","binding":"module_attr","abi":"molt.object_callargs_v1"}'
    ]
    assert args.support_file == ["numpy/_core/_multiarray_umath.py"]
    assert args.json is True


def test_extension_metadata_materializes_meson_cross_and_python_pc(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys,
) -> None:
    host = _stub_metadata_build_machine(monkeypatch)

    def tool(
        role: cli_llvm_wasi_tools.LlvmToolRole,
        command: tuple[str, ...],
    ) -> cli_llvm_wasi_tools.ResolvedLlvmTool:
        return cli_llvm_wasi_tools.ResolvedLlvmTool(
            role=role,
            command=command,
            path=Path(command[0]),
            version="22.1.0",
            sha256="a" * 64,
        )

    zig_tools = cli_llvm_wasi_tools.LlvmWasiToolFamily(
        cc=tool("cc", ("/usr/bin/zig", "cc")),
        cxx=tool("cxx", ("/usr/bin/zig", "c++")),
        wasm_ld=tool("wasm_ld", ("/usr/bin/wasm-ld",)),
        ar=tool("ar", ("/usr/bin/zig", "ar")),
        ranlib=tool("ranlib", ("/usr/bin/zig", "ranlib")),
        nm=tool("nm", ("/usr/bin/llvm-nm",)),
        strip=tool("strip", ("/usr/bin/zig", "strip")),
    )
    monkeypatch.setattr(
        cli_source_extension_toolchain,
        "_resolve_source_extension_wasm_toolchain",
        lambda target_plan, *, environment: (
            cli_source_extension_toolchain._SourceExtensionWasmToolchain(
                ok=True,
                compiler_kind="zig",
                tools=zig_tools,
                wasi_sysroot=None,
                detail="wasm-ld=/usr/bin/wasm-ld; zig=/usr/bin/zig",
            )
        ),
    )
    compiler_builtins = tmp_path / "libclang_rt.builtins-wasm32.a"
    compiler_builtins.write_bytes(b"compiler-builtins")
    monkeypatch.setattr(
        cli_source_extension_link_inputs.wasm_link_inputs,
        "wasm_compiler_builtins_archive",
        lambda _target, *, environment: compiler_builtins,
    )
    out_dir = tmp_path / "metadata"
    rc = cli_commands.extension_metadata(
        target="wasm",
        out_dir=str(out_dir),
        python_version="3.12",
        abi_tier="cpython-abi",
        json_output=True,
        verbose=False,
    )

    assert rc == 0
    payload = json.loads(capsys.readouterr().out)
    assert payload["data"]["target_triple"] == "wasm32-wasip1"
    assert payload["data"]["target"] == {
        "requested": "wasm",
        "compiler_target_triple": "wasm32-wasip1",
        "artifact_kind": "wasm_relocatable_object",
    }
    assert payload["data"]["abi"]["tier"] == "cpython-abi"
    assert payload["data"]["python"] == {
        "implementation": "cpython",
        "version": "3.12",
    }
    assert payload["data"]["schema_version"] == 4
    assert payload["data"]["build_toolchain"]["commands"] == {
        role: list(command) for role, command in host.commands.items()
    }
    assert payload["data"]["paths"]["meson_native"] == str(out_dir / "meson.native")
    native_text = (out_dir / "meson.native").read_text(encoding="utf-8")
    assert "'/host/clang'" in native_text
    for option in ("c_args", "cpp_args", "c_link_args", "cpp_link_args"):
        assert f"{option} = []" in native_text
    assert payload["data"]["toolchain"]["tools"]["nm"] == {
        "command": ["/usr/bin/llvm-nm"],
        "path": str(Path("/usr/bin/llvm-nm")),
        "sha256": "a" * 64,
        "version": "22.1.0",
    }
    assert payload["data"]["toolchain"]["commands"]["c"] == [
        "/usr/bin/zig",
        "cc",
        "-target",
        "wasm32-wasi",
    ]
    assert payload["data"]["toolchain"]["commands"]["cpp"] == [
        "/usr/bin/zig",
        "c++",
        "-target",
        "wasm32-wasi",
    ]
    assert payload["data"]["paths"]["python_pc"] == str(
        out_dir / "pkgconfig" / "python3.pc"
    )
    assert "Version: 3.12" in (out_dir / "pkgconfig" / "python3.pc").read_text(
        encoding="utf-8"
    )
    assert "wasm32" in (out_dir / "meson.cross").read_text(encoding="utf-8")
    assert (out_dir / "source-extension-target-metadata.json").is_file()


def test_source_extension_metadata_materializes_host_native_tool_family(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _stub_metadata_build_machine(monkeypatch)
    target_plan = _source_extension_target_plan("native")
    tools = cli_llvm_wasi_tools.LlvmWasiToolFamily(
        cc=_resolved_llvm_tool("cc", ("/tools/clang",)),
        cxx=_resolved_llvm_tool("cxx", ("/tools/clang++",)),
        wasm_ld=None,
        ar=_resolved_llvm_tool("ar", ("/tools/llvm-ar",)),
        ranlib=None,
        nm=_resolved_llvm_tool("nm", ("/tools/llvm-nm",)),
        strip=None,
    )
    resolved = cli_source_extension_toolchain._ResolvedSourceExtensionToolchain(
        target_plan=target_plan,
        compiler_kind="host",
        tools=tools,
        commands={
            "c": ("/tools/clang",),
            "cpp": ("/tools/clang++",),
            "ar": ("/tools/llvm-ar",),
            "nm": ("/tools/llvm-nm",),
        },
        wasi_sysroot=None,
        link_inputs=SourceExtensionLinkInputs(
            target_plan.target_triple, None, None, None
        ),
        detail="host LLVM family",
    )
    seen_plans: list[cli_source_extension_target.SourceExtensionTargetPlan] = []

    def resolve(
        plan: cli_source_extension_target.SourceExtensionTargetPlan,
        *,
        environment: object,
    ) -> cli_source_extension_toolchain._ResolvedSourceExtensionToolchain:
        seen_plans.append(plan)
        return resolved

    monkeypatch.setattr(
        cli_source_extension_toolchain,
        "_resolve_source_extension_toolchain",
        resolve,
    )

    metadata, errors = (
        cli_source_extension_toolchain._materialize_source_extension_target_metadata(
            molt_root=ROOT,
            out_dir=tmp_path / "native-metadata",
            target_plan=target_plan,
            python_version="3.12",
            abi_tier="cpython-abi",
        )
    )

    assert errors == []
    assert metadata is not None
    assert seen_plans == [target_plan]
    assert (
        metadata.payload["build_toolchain"]["commands"]
        == metadata.payload["toolchain"]["commands"]
    )
    assert (
        metadata.payload["build_toolchain"]["tools"]
        == metadata.payload["toolchain"]["tools"]
    )
    assert metadata.payload["target"] == {
        "requested": "native",
        "compiler_target_triple": None,
        "artifact_kind": "static_archive",
    }
    assert metadata.payload["toolchain"]["commands"] == {
        "ar": ["/tools/llvm-ar"],
        "c": ["/tools/clang"],
        "cpp": ["/tools/clang++"],
        "nm": ["/tools/llvm-nm"],
    }
    assert metadata.payload["toolchain"]["link_probe_archives"] == {}
    assert metadata.payload["meson_cross_properties"] == {
        "needs_exe_wrapper": False,
        "skip_sanity_check": False,
    }
    meson_cross = metadata.meson_cross.read_text(encoding="utf-8")
    assert "system = 'linux'" in meson_cross
    assert "cpu_family = 'x86_64'" in meson_cross
    assert "c_link_args = []" in meson_cross
    assert "cpp_link_args = []" in meson_cross

    invalid_metadata, invalid_errors = (
        cli_source_extension_toolchain._materialize_source_extension_target_metadata(
            molt_root=ROOT,
            out_dir=tmp_path / "invalid-native-metadata",
            target_plan=target_plan,
            python_version="3.12",
            abi_tier="unknown-abi",
        )
    )
    assert invalid_metadata is None
    assert len(invalid_errors) == 1
    assert "validation/materialization failed" in invalid_errors[0]
    assert "ABI tier must be source-compat or cpython-abi" in invalid_errors[0]


def test_native_target_metadata_commands_drive_real_extension_build(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    target_plan = cli_source_extension_target.resolve_source_extension_target_plan(
        "native",
        host_platform=cli_commands.sys.platform,
        host_arch=cli_commands.platform.machine(),
    )
    compiler = (
        "/tools/clang-cl"
        if target_plan.target_triple.endswith("-windows-msvc")
        else "/tools/clang"
    )
    cxx = compiler if compiler.endswith("clang-cl") else "/tools/clang++"
    tools = cli_llvm_wasi_tools.LlvmWasiToolFamily(
        cc=_resolved_llvm_tool("cc", (compiler,)),
        cxx=_resolved_llvm_tool("cxx", (cxx,)),
        wasm_ld=None,
        ar=_resolved_llvm_tool("ar", ("/tools/llvm-ar",)),
        ranlib=None,
        nm=_resolved_llvm_tool("nm", ("/tools/llvm-nm",)),
        strip=None,
    )
    resolved = cli_source_extension_toolchain._ResolvedSourceExtensionToolchain(
        target_plan=target_plan,
        compiler_kind="host",
        tools=tools,
        commands={
            "c": (compiler,),
            "cpp": (cxx,),
            "ar": ("/tools/llvm-ar",),
            "nm": ("/tools/llvm-nm",),
        },
        wasi_sysroot=None,
        link_inputs=SourceExtensionLinkInputs(
            target_plan.target_triple, None, None, None
        ),
        detail="host LLVM family",
    )
    monkeypatch.setattr(
        cli_source_extension_toolchain,
        "_resolve_source_extension_toolchain",
        lambda plan, **kwargs: (
            resolved if plan == target_plan else pytest.fail("target drift")
        ),
    )
    metadata, errors = (
        cli_source_extension_toolchain._materialize_source_extension_target_metadata(
            molt_root=ROOT,
            out_dir=tmp_path / "native-metadata",
            target_plan=target_plan,
            python_version="3.12",
            abi_tier="cpython-abi",
        )
    )
    assert errors == []
    assert metadata is not None
    raw_commands = metadata.payload["toolchain"]["commands"]
    assert isinstance(raw_commands, dict)
    tool_commands = {role: tuple(command) for role, command in raw_commands.items()}

    project_root = tmp_path / "native-project"
    project_root.mkdir()
    _write_extension_project(project_root)
    executed: list[list[str]] = []

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        executed.append(cmd)
        _materialize_fake_extension_command(cmd)
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli_commands, "_run_completed_command", fake_run)
    _install_extension_object_symbol_facts(
        monkeypatch,
        default_init_symbol="PyInit_demoext",
    )

    out_dir = project_root / "dist"
    rc = cli_commands.extension_build(
        project=str(project_root),
        out_dir=str(out_dir),
        target="native",
        deterministic=False,
        json_output=False,
        verbose=False,
        tool_commands=tool_commands,
    )

    assert rc == 0
    compile_command = next(
        command for command in executed if "-c" in command or "/c" in command
    )
    archive_command = next(command for command in executed if "rcsD" in command)
    assert compile_command[: len(tool_commands["c"])] == list(tool_commands["c"])
    assert archive_command[: len(tool_commands["ar"])] == list(tool_commands["ar"])
    manifest = json.loads((out_dir / "extension_manifest.json").read_text())
    assert manifest["target_triple"] == target_plan.target_triple
    assert manifest["artifact_kind"] == "static_archive"
    assert manifest["build"]["tool_commands"] == {
        role: list(command) for role, command in sorted(tool_commands.items())
    }


def test_source_extension_native_cross_toolchain_preserves_compiler_and_target(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    target_plan = _source_extension_target_plan("aarch64-apple-darwin")
    compiler = tmp_path / "cross-clang"
    compiler.write_bytes(b"compiler")
    configured_command = (str(compiler), "--driver-mode=gcc")
    monkeypatch.setenv("MOLT_CROSS_CC", " ".join(configured_command))
    monkeypatch.delenv("MOLT_CROSS_CXX", raising=False)
    seen_explicit: list[dict[cli_llvm_wasi_tools.LlvmToolRole, tuple[str, ...]]] = []

    def resolve_family(
        *,
        target_family: cli_llvm_wasi_tools.LlvmTargetFamily,
        explicit_commands: dict[cli_llvm_wasi_tools.LlvmToolRole, tuple[str, ...]],
        sibling_directories: tuple[Path, ...],
        environment: object,
    ) -> cli_llvm_wasi_tools.LlvmWasiToolFamily:
        del sibling_directories, environment
        assert target_family == "native"
        seen_explicit.append(dict(explicit_commands))
        return cli_llvm_wasi_tools.LlvmWasiToolFamily(
            cc=_resolved_llvm_tool("cc", explicit_commands["cc"]),
            cxx=_resolved_llvm_tool("cxx", ("/tools/clang++",)),
            wasm_ld=None,
            ar=_resolved_llvm_tool("ar", ("/tools/llvm-ar",)),
            ranlib=None,
            nm=_resolved_llvm_tool("nm", ("/tools/llvm-nm",)),
            strip=None,
        )

    monkeypatch.setattr(
        cli_source_extension_toolchain,
        "resolve_llvm_wasi_tool_family",
        resolve_family,
    )

    resolved = (
        cli_source_extension_toolchain._resolve_source_extension_native_toolchain(
            target_plan
        )
    )

    expected_c = (
        *configured_command,
        "-target",
        "aarch64-apple-darwin",
    )
    assert seen_explicit == [{"cc": expected_c}]
    assert resolved.compiler_kind == "molt_cross_cc"
    assert resolved.commands["c"] == expected_c
    assert resolved.commands["cpp"] == (
        "/tools/clang++",
        "--driver-mode=gcc",
        "-target",
        "aarch64-apple-darwin",
    )
    assert {"c", "ar", "nm"} <= resolved.commands.keys()


def test_source_extension_freestanding_metadata_needs_no_wasi_or_libc(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _stub_metadata_build_machine(monkeypatch)
    target_plan = _source_extension_target_plan("wasm-freestanding")
    tools = cli_llvm_wasi_tools.LlvmWasiToolFamily(
        cc=_resolved_llvm_tool("cc", ("/tools/clang",)),
        cxx=_resolved_llvm_tool("cxx", ("/tools/clang++",)),
        wasm_ld=_resolved_llvm_tool("wasm_ld", ("/tools/wasm-ld",)),
        ar=_resolved_llvm_tool("ar", ("/tools/llvm-ar",)),
        ranlib=_resolved_llvm_tool("ranlib", ("/tools/llvm-ranlib",)),
        nm=_resolved_llvm_tool("nm", ("/tools/llvm-nm",)),
        strip=_resolved_llvm_tool("strip", ("/tools/llvm-strip",)),
    )
    monkeypatch.delenv("MOLT_WASM_CC", raising=False)
    monkeypatch.delenv("MOLT_CROSS_CC", raising=False)
    monkeypatch.setattr(
        cli_source_extension_toolchain,
        "resolve_llvm_wasi_tool_family",
        lambda **_kwargs: tools,
    )
    monkeypatch.setattr(
        cli_source_extension_toolchain,
        "_resolve_wasi_sysroot",
        lambda *, env: pytest.fail("freestanding resolution must not probe WASI"),
    )
    probe_sources: list[str] = []
    probe_commands: list[list[str]] = []

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        probe_commands.append(cmd)
        source = Path(cmd[cmd.index("-c") + 1])
        probe_sources.append(source.read_text(encoding="ascii"))
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(
        cli_source_extension_toolchain.subprocess,
        "run",
        fake_run,
    )
    resolved = cli_source_extension_toolchain._resolve_source_extension_toolchain(
        target_plan
    )
    monkeypatch.setattr(
        cli_source_extension_toolchain,
        "_resolve_source_extension_toolchain",
        lambda plan, **kwargs: (
            resolved if plan == target_plan else pytest.fail("target drift")
        ),
    )
    monkeypatch.setattr(
        cli_source_extension_link_inputs.wasm_link_inputs,
        "wasm_compiler_builtins_archive",
        lambda _target, *, environment: pytest.fail(
            "freestanding metadata must not require compiler-builtins"
        ),
    )

    metadata, errors = (
        cli_source_extension_toolchain._materialize_source_extension_target_metadata(
            molt_root=ROOT,
            out_dir=tmp_path / "freestanding-metadata",
            target_plan=target_plan,
            python_version="3.12",
            abi_tier="cpython-abi",
        )
    )

    assert errors == []
    assert metadata is not None
    assert resolved.wasi_sysroot is None
    assert probe_sources == ["int molt_probe(void) { return 0; }\n"]
    assert probe_commands[0][:3] == [
        "/tools/clang",
        "-target",
        "wasm32-unknown-unknown",
    ]
    assert metadata.payload["target"] == {
        "requested": "wasm-freestanding",
        "compiler_target_triple": "wasm32-unknown-unknown",
        "artifact_kind": "wasm_relocatable_object",
    }
    assert metadata.payload["toolchain"]["link_probe_archives"] == {}
    meson_cross = metadata.meson_cross.read_text(encoding="utf-8")
    assert "system = 'none'" in meson_cross
    assert "c_link_args = ['-nostdlib']" in meson_cross
    assert "'-lc'" not in meson_cross


def test_freestanding_metadata_commands_drive_compile_and_relocatable_link(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _stub_metadata_build_machine(monkeypatch)
    target_plan = _source_extension_target_plan("wasm-freestanding")
    tools = cli_llvm_wasi_tools.LlvmWasiToolFamily(
        cc=_resolved_llvm_tool("cc", ("/tools/clang",)),
        cxx=_resolved_llvm_tool("cxx", ("/tools/clang++",)),
        wasm_ld=_resolved_llvm_tool("wasm_ld", ("/tools/wasm-ld",)),
        ar=_resolved_llvm_tool("ar", ("/tools/llvm-ar",)),
        ranlib=_resolved_llvm_tool("ranlib", ("/tools/llvm-ranlib",)),
        nm=_resolved_llvm_tool("nm", ("/tools/llvm-nm",)),
        strip=_resolved_llvm_tool("strip", ("/tools/llvm-strip",)),
    )
    resolved = cli_source_extension_toolchain._ResolvedSourceExtensionToolchain(
        target_plan=target_plan,
        compiler_kind="clang",
        tools=tools,
        commands={
            "c": ("/tools/clang", "-target", "wasm32-unknown-unknown"),
            "cpp": ("/tools/clang++", "-target", "wasm32-unknown-unknown"),
            "ld": ("/tools/wasm-ld",),
            "ar": ("/tools/llvm-ar",),
            "ranlib": ("/tools/llvm-ranlib",),
            "nm": ("/tools/llvm-nm",),
            "strip": ("/tools/llvm-strip",),
        },
        wasi_sysroot=None,
        link_inputs=SourceExtensionLinkInputs(
            target_plan.target_triple, None, None, None
        ),
        detail="freestanding LLVM family",
    )
    monkeypatch.setattr(
        cli_source_extension_toolchain,
        "_resolve_source_extension_toolchain",
        lambda plan, **kwargs: (
            resolved if plan == target_plan else pytest.fail("target drift")
        ),
    )
    monkeypatch.setattr(
        cli_source_extension_link_inputs.wasm_link_inputs,
        "wasm_compiler_builtins_archive",
        lambda _target, *, environment: pytest.fail(
            "freestanding metadata needs no builtins"
        ),
    )
    metadata, errors = (
        cli_source_extension_toolchain._materialize_source_extension_target_metadata(
            molt_root=ROOT,
            out_dir=tmp_path / "freestanding-metadata",
            target_plan=target_plan,
            python_version="3.12",
            abi_tier="cpython-abi",
        )
    )
    assert errors == []
    assert metadata is not None
    raw_commands = metadata.payload["toolchain"]["commands"]
    assert isinstance(raw_commands, dict)
    tool_commands = {role: tuple(command) for role, command in raw_commands.items()}

    project_root = tmp_path / "freestanding-project"
    project_root.mkdir()
    _write_extension_project(project_root)
    helper_source = project_root / "src/helper.c"
    helper_source.write_text("int molt_helper(void) { return 1; }\n", encoding="utf-8")
    pyproject = project_root / "pyproject.toml"
    pyproject.write_text(
        pyproject.read_text(encoding="utf-8").replace(
            'sources = ["src/demoext.c"]',
            'sources = ["src/demoext.c", "src/helper.c"]',
        ),
        encoding="utf-8",
    )
    wasm_bytes = _wasm_exporting_i64_unary_symbol(
        "PyInit_demoext",
        imports=("PyModule_Create2", "molt_c_api_version"),
    )
    executed: list[list[str]] = []

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        executed.append(cmd)
        output = Path(cmd[cmd.index("-o") + 1])
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_bytes(wasm_bytes)
        _write_fake_compiler_depfile(cmd)
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli_commands, "_run_completed_command", fake_run)
    monkeypatch.setattr(
        cli_commands,
        "resolve_wasi_sysroot",
        lambda: pytest.fail("freestanding build must not probe WASI"),
    )
    _install_extension_object_symbol_facts(
        monkeypatch,
        default_init_symbol="PyInit_demoext",
        by_stem={
            "demoext": (
                {"PyInit_demoext"},
                {"PyModule_Create2", "molt_c_api_version", "molt_helper"},
            ),
            "helper": ({"molt_helper"}, set()),
        },
    )

    out_dir = project_root / "dist"
    rc = cli_commands.extension_build(
        project=str(project_root),
        out_dir=str(out_dir),
        target="wasm-freestanding",
        deterministic=False,
        json_output=False,
        verbose=False,
        tool_commands=tool_commands,
    )

    assert rc == 0
    compile_commands = [command for command in executed if "-c" in command]
    link_command = next(command for command in executed if "-r" in command)
    assert len(compile_commands) == 2
    assert all(
        command[: len(tool_commands["c"])] == list(tool_commands["c"])
        for command in compile_commands
    )
    assert link_command[: len(tool_commands["ld"])] == list(tool_commands["ld"])
    manifest = json.loads((out_dir / "extension_manifest.json").read_text())
    assert manifest["target_triple"] == "wasm32-unknown-unknown"
    assert manifest["artifact_kind"] == "wasm_relocatable_object"
    assert manifest["build"]["wasi_sysroot"] is None


def test_source_extension_toolchain_rejects_wasm_cc_without_wasi_headers(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(cli_llvm_wasi_tools, "_sha256_file", lambda _path: "a" * 64)
    monkeypatch.setenv("MOLT_WASM_CC", "clang")
    monkeypatch.delenv("MOLT_CROSS_CC", raising=False)
    monkeypatch.setattr(
        cli_llvm_wasi_tools,
        "find_executable",
        lambda tool, **_kwargs: {
            "clang": "/tools/clang",
            "clang++": "/tools/clang++",
            "wasm-ld": "/tools/wasm-ld",
            "llvm-ar": "/tools/llvm-ar",
            "llvm-ranlib": "/tools/llvm-ranlib",
            "llvm-nm": "/tools/llvm-nm",
            "llvm-strip": "/tools/llvm-strip",
        }.get(tool),
    )

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        return subprocess.CompletedProcess(
            cmd,
            1,
            "",
            "fatal error: 'errno.h' file not found\n",
        )

    monkeypatch.setattr(
        cli_source_extension_toolchain.subprocess,
        "run",
        fake_run,
    )

    toolchain = cli_source_extension_toolchain._resolve_source_extension_wasm_toolchain(
        _source_extension_target_plan("wasm")
    )

    assert toolchain.ok is False
    assert toolchain.compiler_kind == "molt_wasm_cc"
    assert "MOLT_WASM_CC cannot compile the WASI source-extension probe" in (
        toolchain.detail
    )
    assert "errno.h" in toolchain.detail
    assert "WASI_SYSROOT" in toolchain.detail


def test_source_extension_toolchain_prefers_wasm_cc_and_probes_target(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(cli_llvm_wasi_tools, "_sha256_file", lambda _path: "a" * 64)
    seen_commands: list[list[str]] = []
    monkeypatch.setenv("MOLT_WASM_CC", "clang-wasm")
    monkeypatch.setenv("MOLT_CROSS_CC", "wrong-cross")
    tool_root = tmp_path / "tools"
    tool_root.mkdir()
    tool_paths = {
        name: tool_root / name
        for name in (
            "clang-wasm",
            "wrong-cross",
            "clang++",
            "wasm-ld",
            "llvm-ar",
            "llvm-ranlib",
            "llvm-nm",
            "llvm-strip",
        )
    }
    for path in tool_paths.values():
        path.write_bytes(b"tool")
        path.chmod(0o755)
    monkeypatch.setenv("MOLT_WASM_CC", '"' + str(tool_paths["clang-wasm"]) + '"')
    monkeypatch.setattr(
        cli_llvm_wasi_tools,
        "find_executable",
        lambda tool, **_kwargs: str(tool_paths[tool]) if tool in tool_paths else None,
    )

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        seen_commands.append(cmd)
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(
        cli_source_extension_toolchain.subprocess,
        "run",
        fake_run,
    )

    toolchain = cli_source_extension_toolchain._resolve_source_extension_wasm_toolchain(
        _source_extension_target_plan("wasm")
    )

    assert toolchain.ok is True
    assert toolchain.compiler_kind == "molt_wasm_cc"
    assert toolchain.tools.cc is not None
    assert toolchain.tools.cc.command == (str(tool_paths["clang-wasm"].resolve()),)
    assert "MOLT_WASM_CC=" in toolchain.detail
    assert str(tool_paths["clang-wasm"].resolve()) in toolchain.detail
    assert "wrong-cross" not in toolchain.detail
    compile_commands = [command for command in seen_commands if "-c" in command]
    assert compile_commands
    assert compile_commands[0][:3] == [
        str(tool_paths["clang-wasm"].resolve()),
        "-target",
        "wasm32-wasip1",
    ]


def test_source_extension_toolchain_accepts_target_specific_wasi_sysroot_layout(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(cli_llvm_wasi_tools, "_sha256_file", lambda _path: "a" * 64)
    sysroot = tmp_path / "wasi-sysroot-33.0+m"
    include_dir = sysroot / "include" / "wasm32-wasip1"
    include_dir.mkdir(parents=True)
    (include_dir / "errno.h").write_text("#define EINVAL 28\n")
    wasm_link_inputs._resolve_wasi_sysroot_cached.cache_clear()
    monkeypatch.setenv("WASI_SYSROOT", str(sysroot))
    monkeypatch.delenv("MOLT_WASM_CC", raising=False)
    monkeypatch.delenv("MOLT_CROSS_CC", raising=False)
    tool_root = tmp_path / "tools"
    tool_root.mkdir()
    tool_paths = {
        name: tool_root / name
        for name in (
            "clang",
            "clang++",
            "zig",
            "wasm-ld",
            "llvm-ar",
            "llvm-ranlib",
            "llvm-nm",
            "llvm-strip",
        )
    }
    for path in tool_paths.values():
        path.write_bytes(b"tool")
    monkeypatch.setattr(
        cli_llvm_wasi_tools,
        "find_executable",
        lambda tool, **_kwargs: str(tool_paths[tool]) if tool in tool_paths else None,
    )
    seen_commands: list[list[str]] = []

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        seen_commands.append(cmd)
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(
        cli_source_extension_toolchain.subprocess,
        "run",
        fake_run,
    )

    toolchain = cli_source_extension_toolchain._resolve_source_extension_wasm_toolchain(
        _source_extension_target_plan("wasm")
    )

    assert toolchain.ok is True
    assert toolchain.compiler_kind == "clang"
    assert toolchain.tools.cc is not None
    assert toolchain.tools.cc.command[-2:] == (
        "--sysroot",
        str(sysroot.resolve(strict=False)),
    )
    assert toolchain.wasi_sysroot == sysroot.resolve(strict=False)
    compile_commands = [command for command in seen_commands if "-c" in command]
    assert compile_commands
    sysroot_index = compile_commands[0].index("--sysroot")
    assert compile_commands[0][sysroot_index : sysroot_index + 5] == [
        "--sysroot",
        str(sysroot.resolve(strict=False)),
        "-target",
        "wasm32-wasip1",
        "-c",
    ]


def test_wasm_cxx_runtime_archives_resolve_matching_exception_variant(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    sysroot = tmp_path / "wasi-sysroot"
    library_root = sysroot / "lib" / "wasm32-wasip1" / "eh"
    library_root.mkdir(parents=True)
    libcxx = library_root / "libc++.a"
    libcxxabi = library_root / "libc++abi.a"
    libunwind = library_root / "libunwind.a"
    libcxx.write_bytes(b"!<arch>\nlibcxx")
    libcxxabi.write_bytes(b"!<arch>\nlibcxxabi")
    libunwind.write_bytes(b"!<arch>\nlibunwind")
    monkeypatch.setattr(
        wasm_link_inputs,
        "resolve_wasi_sysroot",
        lambda: sysroot,
    )

    assert wasm_link_inputs.wasm_cxx_runtime_archives() == (
        libcxx.resolve(strict=False),
        libcxxabi.resolve(strict=False),
        libunwind.resolve(strict=False),
    )


def test_wasm_cxx_runtime_archives_resolve_matching_no_exception_variant(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    sysroot = tmp_path / "wasi-sysroot"
    library_root = sysroot / "lib" / "wasm32-wasip1" / "noeh"
    library_root.mkdir(parents=True)
    libcxx = library_root / "libc++.a"
    libcxxabi = library_root / "libc++abi.a"
    libcxx.write_bytes(b"!<arch>\nlibcxx")
    libcxxabi.write_bytes(b"!<arch>\nlibcxxabi")
    monkeypatch.setattr(
        wasm_link_inputs,
        "resolve_wasi_sysroot",
        lambda: sysroot,
    )

    assert wasm_link_inputs.wasm_cxx_runtime_archives(exceptions=False) == (
        libcxx.resolve(strict=False),
        libcxxabi.resolve(strict=False),
    )


@pytest.mark.parametrize(
    ("abi_tier", "c_api_import", "supported"),
    [
        ("source-compat", "PyLong_FromLong", True),
        ("cpython-abi", "PyOS_strtol", True),
        ("source-compat", "PyOS_strtol", False),
    ],
)
def test_extension_build_wasm_target_emits_static_link_artifact_and_manifest(
    tmp_path: Path,
    monkeypatch,
    capsys,
    abi_tier: str,
    c_api_import: str,
    supported: bool,
) -> None:
    project_root = tmp_path / "extproj"
    project_root.mkdir()
    native_symbol = "molt_demoext_ndimage_distance_transform_edt"
    _write_extension_project(
        project_root,
        extension_extra_lines=[
            'python_exports = ["demoext.ndimage.distance_transform_edt"]',
            "",
            "[[tool.molt.extension.callable_exports]]",
            'module = "demoext.ndimage"',
            'name = "distance_transform_edt"',
            'binding = "direct_symbol"',
            'abi = "molt.object_call_v1"',
            f'symbol = "{native_symbol}"',
            "arity = 1",
            'effects = ["read"]',
            "deterministic = true",
        ],
    )
    wasm_function_imports = (
        "molt_alloc",
        "molt_cpython_abi_date_from_date",
        c_api_import,
        "malloc",
    )
    raw_data_relocations = ("PyExc_RuntimeError", "Py_None")
    monkeypatch.setattr(
        cli_source_extensions,
        "wasm_external_link_provider_symbol_classes",
        lambda _target: {"malloc": "wasm_libc_link_import"},
    )
    wasm_bytes = _wasm_exporting_i64_unary_symbols(
        ("PyInit_demoext", native_symbol),
        imports=wasm_function_imports,
        memory_imports=("__linear_memory",),
    )
    commands: list[list[str]] = []

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        commands.append(cmd)
        out_index = cmd.index("-o")
        out_path = Path(cmd[out_index + 1])
        out_path.parent.mkdir(parents=True, exist_ok=True)
        out_path.write_bytes(wasm_bytes)
        _write_fake_compiler_depfile(cmd)
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli_commands, "_run_completed_command", fake_run)
    _install_extension_object_symbol_facts(
        monkeypatch,
        default_init_symbol="PyInit_demoext",
        by_stem={
            "demoext": (
                {"PyInit_demoext", native_symbol},
                set((*wasm_function_imports, *raw_data_relocations)),
            ),
        },
    )
    wasi_sysroot = _write_fake_wasi_sysroot(tmp_path)
    monkeypatch.setattr(
        cli_commands,
        "resolve_wasi_sysroot",
        lambda: wasi_sysroot,
        raising=True,
    )

    out_dir = project_root / "dist"
    rc = cli_commands.extension_build(
        project=str(project_root),
        out_dir=str(out_dir),
        target="wasm",
        abi_tier=abi_tier,
        deterministic=False,
        json_output=True,
        verbose=False,
    )
    payload = json.loads(capsys.readouterr().out)
    if not supported:
        assert rc == 2
        assert f"missing: {c_api_import}" in " ".join(payload["errors"])
        assert not (out_dir / "extension_manifest.json").exists()
        assert not (out_dir / "demoext.molt.wasm").exists()
        return
    assert rc == 0
    assert payload["data"]["target_triple"] == "wasm32-wasip1"
    assert payload["data"]["runtime_linkage"] == "static_link"
    assert payload["data"]["artifact_kind"] == "wasm_relocatable_object"
    assert any(
        "-target" in cmd and cmd[cmd.index("-target") + 1] == "wasm32-wasip1"
        for cmd in commands
    )
    selected_sysroot = next(
        Path(cmd[cmd.index("--sysroot") + 1]).resolve()
        for cmd in commands
        if "--sysroot" in cmd
    )
    artifact_path = out_dir / "demoext.molt.wasm"
    assert artifact_path.exists()
    assert artifact_path.read_bytes() == wasm_bytes
    assert [
        export.name
        for export in wasm_artifact.read_wasm_function_exports(artifact_path)
    ] == ["PyInit_demoext", native_symbol]

    manifest = json.loads((out_dir / "extension_manifest.json").read_text())
    assert manifest["target_triple"] == "wasm32-wasip1"
    assert manifest["runtime_linkage"] == "static_link"
    assert manifest["artifact_kind"] == "wasm_relocatable_object"
    assert manifest["build"]["wasi_sysroot"] == str(selected_sysroot)
    assert manifest["extension"] == "demoext.molt.wasm"
    assert manifest["extension_sha256"] == hashlib.sha256(wasm_bytes).hexdigest()
    object_closure = manifest["object_closure"]
    assert object_closure["defined_symbols"] == ["PyInit_demoext", native_symbol]
    raw_undefined = (*wasm_function_imports, *raw_data_relocations)
    assert object_closure["undefined_symbols"] == sorted(raw_undefined)
    assert object_closure["wasm_imports"] == sorted(
        (
            *(
                {"module": "env", "name": name, "kind": "function"}
                for name in wasm_function_imports
            ),
            {"module": "env", "name": "__linear_memory", "kind": "memory"},
        ),
        key=lambda item: (item["module"], item["name"], item["kind"]),
    )
    assert object_closure["runtime_symbols"] == sorted(
        wasm_static_link_runtime_symbols_for_imports(
            (*raw_undefined, "__linear_memory")
        )
    )
    assert "malloc" not in object_closure["runtime_symbols"]
    assert object_closure["objects"][0]["undefined_symbols"] == sorted(raw_undefined)
    assert "__linear_memory" not in object_closure["objects"][0]["undefined_symbols"]
    assert object_closure["closure_sha256"] == source_extension_object_closure_digest(
        object_closure
    )
    required_c_api_symbols = {
        symbol
        for item in object_closure["objects"]
        for symbol in item["required_c_api_symbols"]
    }
    assert c_api_import in required_c_api_symbols
    assert "PyInit_demoext" not in required_c_api_symbols
    assert "PyMODINIT_FUNC" not in required_c_api_symbols
    assert "PyTypeObject" not in required_c_api_symbols
    assert "Python" not in required_c_api_symbols
    assert manifest["callable_exports"] == [
        {
            "module": "demoext.ndimage",
            "name": "distance_transform_edt",
            "binding": "direct_symbol",
            "abi": "molt.object_call_v1",
            "symbol": native_symbol,
            "arity": 1,
            "effects": ["read"],
            "deterministic": True,
        }
    ]

    wheel_path = next(out_dir.glob("*.whl"))
    with zipfile.ZipFile(wheel_path) as zf:
        assert manifest["extension"] in set(zf.namelist())
        embedded = json.loads(zf.read("extension_manifest.json"))
    assert embedded["runtime_linkage"] == "static_link"
    assert embedded["artifact_kind"] == "wasm_relocatable_object"
    embedded_closure = embedded["object_closure"]
    for field in (
        "defined_symbols",
        "undefined_symbols",
        "runtime_symbols",
        "required_capsules",
        "project_generated_c_api_prefixes",
    ):
        assert embedded_closure[field] == object_closure[field]
    assert embedded_closure["objects"][0]["required_c_api_symbols"] == sorted(
        required_c_api_symbols
    )
    assert embedded_closure["objects"][0]["compile_command"][0].startswith(
        "@toolchain/"
    )
    assert "@wasi-sysroot" in embedded_closure["objects"][0]["compile_command"]


@pytest.mark.slow
def test_extension_build_real_wasm_object_separates_import_and_data_relocation_closure(
    tmp_path: Path,
) -> None:
    target_plan = _source_extension_target_plan("wasm")
    try:
        cli_source_extension_toolchain._resolve_source_extension_toolchain(target_plan)
    except (OSError, ValueError) as exc:
        pytest.skip(f"LLVM/WASI source-extension toolchain is unavailable: {exc}")

    out_dir = tmp_path / "pending-call-probe"
    rc = cli_commands.extension_build(
        project=str(ROOT / "tests/fixtures/pending_call_probe"),
        out_dir=str(out_dir),
        target="wasm",
        deterministic=True,
        json_output=False,
        verbose=False,
    )

    assert rc == 0
    manifest = json.loads((out_dir / "extension_manifest.json").read_text())
    object_closure = manifest["object_closure"]
    raw_undefined = object_closure["objects"][0]["undefined_symbols"]
    wasm_import_names = {item["name"] for item in object_closure["wasm_imports"]}
    assert {"PyExc_RuntimeError", "Py_None"} <= set(raw_undefined)
    assert {"PyExc_RuntimeError", "Py_None"} <= set(object_closure["undefined_symbols"])
    assert {"PyExc_RuntimeError", "Py_None"}.isdisjoint(wasm_import_names)
    assert "__linear_memory" in wasm_import_names
    assert "__linear_memory" not in raw_undefined
    assert {"PyExc_RuntimeError", "Py_None"} <= set(object_closure["runtime_symbols"])
    assert object_closure["closure_sha256"] == source_extension_object_closure_digest(
        object_closure
    )


def test_extension_build_wasm_source_recompiled_package_requires_export_custody(
    tmp_path: Path,
    monkeypatch,
    capsys,
) -> None:
    project_root = tmp_path / "numpy_extproj"
    project_root.mkdir()
    _write_extension_project(project_root)
    pyproject = project_root / "pyproject.toml"
    pyproject.write_text(
        pyproject.read_text().replace(
            'module = "demoext"',
            'module = "numpy._core._multiarray_umath"',
        ),
        encoding="utf-8",
    )
    wasi_sysroot = _write_fake_wasi_sysroot(tmp_path)
    monkeypatch.setattr(
        cli_commands,
        "resolve_wasi_sysroot",
        lambda: wasi_sysroot,
        raising=True,
    )

    rc = cli_commands.extension_build(
        project=str(project_root),
        out_dir=str(project_root / "dist"),
        target="wasm",
        deterministic=False,
        json_output=False,
        verbose=False,
    )

    assert rc == 2
    stderr = capsys.readouterr().err
    assert "Source-recompiled extension builds for 'numpy'" in stderr
    assert "tool.molt.extension.python_exports" in stderr
    assert "tool.molt.extension.callable_exports" in stderr
    assert "not package directory ancestry" in stderr


def test_extension_build_wasm_source_recompiled_package_accepts_cli_python_export(
    tmp_path: Path,
    monkeypatch,
) -> None:
    project_root = tmp_path / "numpy_extproj"
    project_root.mkdir()
    _write_extension_project(project_root)
    pyproject = project_root / "pyproject.toml"
    pyproject.write_text(
        pyproject.read_text().replace(
            'module = "demoext"',
            'module = "numpy._core._multiarray_umath"',
        ),
        encoding="utf-8",
    )
    init_symbol = "PyInit__multiarray_umath"
    wasm_bytes = _wasm_exporting_i64_unary_symbol(init_symbol)

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        out_index = cmd.index("-o")
        out_path = Path(cmd[out_index + 1])
        out_path.parent.mkdir(parents=True, exist_ok=True)
        out_path.write_bytes(wasm_bytes)
        _write_fake_compiler_depfile(cmd)
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli_commands, "_run_completed_command", fake_run)
    _install_extension_object_symbol_facts(
        monkeypatch,
        default_init_symbol=init_symbol,
    )
    wasi_sysroot = _write_fake_wasi_sysroot(tmp_path)
    monkeypatch.setattr(
        cli_commands,
        "resolve_wasi_sysroot",
        lambda: wasi_sysroot,
        raising=True,
    )

    out_dir = project_root / "dist"
    rc = cli_commands.extension_build(
        project=str(project_root),
        out_dir=str(out_dir),
        target="wasm",
        deterministic=False,
        python_export=["numpy"],
        json_output=False,
        verbose=False,
    )

    assert rc == 0
    manifest = json.loads((out_dir / "extension_manifest.json").read_text())
    assert manifest["module"] == "numpy._core._multiarray_umath"
    assert manifest["python_exports"] == ["numpy"]
    assert manifest["runtime_linkage"] == "static_link"
    assert (out_dir / "numpy" / "_core" / "_multiarray_umath.molt.wasm").exists()
    artifact_manifest = json.loads(
        (
            out_dir
            / "numpy"
            / "_core"
            / "_multiarray_umath.molt.wasm.extension_manifest.json"
        ).read_text(encoding="utf-8")
    )
    assert artifact_manifest["python_exports"] == ["numpy"]


def test_extension_build_wasm_target_rejects_missing_direct_symbol(
    tmp_path: Path,
    monkeypatch,
) -> None:
    project_root = tmp_path / "extproj"
    project_root.mkdir()
    native_symbol = "molt_demoext_ndimage_distance_transform_edt"
    _write_extension_project(
        project_root,
        extension_extra_lines=[
            'python_exports = ["demoext.ndimage.distance_transform_edt"]',
            "",
            "[[tool.molt.extension.callable_exports]]",
            'module = "demoext.ndimage"',
            'name = "distance_transform_edt"',
            'binding = "direct_symbol"',
            'abi = "molt.object_call_v1"',
            f'symbol = "{native_symbol}"',
            "arity = 1",
            "deterministic = true",
        ],
    )
    wasm_bytes = _wasm_exporting_i64_unary_symbol("molt_wrong_symbol")

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        out_index = cmd.index("-o")
        out_path = Path(cmd[out_index + 1])
        out_path.parent.mkdir(parents=True, exist_ok=True)
        out_path.write_bytes(wasm_bytes)
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli_commands, "_run_completed_command", fake_run)
    wasi_sysroot = _write_fake_wasi_sysroot(tmp_path)
    monkeypatch.setattr(
        cli_commands,
        "resolve_wasi_sysroot",
        lambda: wasi_sysroot,
        raising=True,
    )

    out_dir = project_root / "dist"
    rc = cli_commands.extension_build(
        project=str(project_root),
        out_dir=str(out_dir),
        target="wasm",
        deterministic=False,
        json_output=False,
        verbose=False,
    )

    assert rc != 0
    assert not (out_dir / "extension_manifest.json").exists()
    assert not (out_dir / "demoext.molt.wasm").exists()


def test_extension_build_wasm_target_requires_wasi_sysroot(
    tmp_path: Path,
    monkeypatch,
) -> None:
    project_root = tmp_path / "extproj"
    project_root.mkdir()
    _write_extension_project(project_root)
    commands: list[list[str]] = []

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        commands.append(cmd)
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli_commands, "_run_completed_command", fake_run)
    monkeypatch.setattr(
        cli_commands,
        "resolve_wasi_sysroot",
        lambda: None,
        raising=True,
    )
    monkeypatch.setattr(
        cli_source_extension_toolchain,
        "_resolve_wasi_sysroot",
        lambda *, env: None,
        raising=True,
    )

    out_dir = project_root / "dist"
    rc = cli_commands.extension_build(
        project=str(project_root),
        out_dir=str(out_dir),
        target="wasm",
        deterministic=False,
        json_output=False,
        verbose=False,
    )

    assert rc != 0
    assert commands == []
    assert not (out_dir / "extension_manifest.json").exists()


def test_wasi_sysroot_resolver_accepts_target_specific_include_layout(
    tmp_path: Path,
) -> None:
    sysroot = tmp_path / "wasi-sysroot-33.0+m"
    include_dir = sysroot / "include" / "wasm32-wasip1"
    include_dir.mkdir(parents=True)
    (include_dir / "errno.h").write_text("#define EINVAL 28\n")

    assert wasm_link_inputs.normalize_wasi_sysroot(sysroot) == sysroot.resolve(
        strict=False
    )
    assert wasm_link_inputs.normalize_wasi_sysroot(include_dir) == sysroot.resolve(
        strict=False
    )


@pytest.mark.parametrize(
    "target",
    [None, "aarch64-unknown-linux-gnu"],
    ids=["native", "cross-aarch64-gnu"],
)
def test_extension_numpy_build_uses_compiled_link_closure_matrix(
    tmp_path: Path,
    monkeypatch,
    target: str | None,
) -> None:
    project_root = tmp_path / "numpy_extproj"
    project_root.mkdir()
    _write_extension_numpy_project(project_root)

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        _materialize_fake_extension_command(cmd)
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli_commands, "_run_completed_command", fake_run)
    _install_extension_object_symbol_facts(
        monkeypatch,
        default_init_symbol="PyInit_demoext_numpy",
    )

    if target is not None:
        target_plan = _source_extension_target_plan(target)
        tools = cli_llvm_wasi_tools.LlvmWasiToolFamily(
            cc=_resolved_llvm_tool("cc", ("/usr/bin/zig", "cc")),
            cxx=None,
            wasm_ld=None,
            ar=_resolved_llvm_tool("ar", ("/usr/bin/zig", "ar")),
            ranlib=None,
            nm=_resolved_llvm_tool("nm", ("/usr/bin/llvm-nm",)),
            strip=None,
        )
        resolved = cli_source_extension_toolchain._ResolvedSourceExtensionToolchain(
            target_plan=target_plan,
            compiler_kind="zig",
            tools=tools,
            commands={
                "c": ("/usr/bin/zig", "cc"),
                "ar": ("/usr/bin/zig", "ar"),
                "nm": ("/usr/bin/llvm-nm",),
            },
            wasi_sysroot=None,
            link_inputs=SourceExtensionLinkInputs(
                target_plan.target_triple, None, None, None
            ),
            detail="fixture cross compiler family",
        )
        monkeypatch.setattr(
            cli_commands,
            "_resolve_source_extension_toolchain",
            lambda plan: (
                resolved if plan == target_plan else pytest.fail("target drift")
            ),
        )

    out_dir = project_root / ("dist-" + (target or "native"))
    rc = cli_commands.extension_build(
        project=str(project_root),
        out_dir=str(out_dir),
        deterministic=False,
        target=target,
        json_output=False,
        verbose=False,
    )
    assert rc == 0
    assert list(out_dir.glob("*.whl"))
    manifest = json.loads((out_dir / "extension_manifest.json").read_text())
    assert manifest["runtime_linkage"] == "static_link"
    assert manifest["artifact_kind"] == "static_archive"
    required_symbols = {
        symbol
        for item in manifest["object_closure"]["objects"]
        for symbol in item["required_c_api_symbols"]
    }
    assert required_symbols == {"PyModule_Create2"}
    assert "PyArray_NDIM" not in required_symbols
    assert "NPY_ARRAY_BEHAVED_NS" not in required_symbols


def test_extension_audit_reports_abi_mismatch(tmp_path: Path) -> None:
    out_dir = tmp_path / "dist"
    out_dir.mkdir()

    wheel_name = "demo_ext-0.1.0-py3-molt_abi1-x86_64_unknown_linux_gnu.whl"
    wheel_path = out_dir / wheel_name
    with zipfile.ZipFile(wheel_path, "w") as zf:
        zf.writestr("demoext.so", b"shared")

    manifest = {
        "schema_version": 1,
        "name": "demo-ext",
        "version": "0.1.0",
        "module": "demoext",
        "molt_c_api_version": "1",
        "abi_tag": "molt_abi1",
        "python_tag": "py3",
        "target_python": "py312",
        "target_triple": "x86_64-unknown-linux-gnu",
        "platform_tag": "x86_64_unknown_linux_gnu",
        "capabilities": ["fs.read"],
        "wheel": wheel_name,
        "extension": "demoext.so",
    }
    (out_dir / "extension_manifest.json").write_text(
        json.dumps(manifest, indent=2) + "\n"
    )

    rc = cli.extension_audit(
        path=str(out_dir),
        require_capabilities=True,
        require_abi="2",
        json_output=False,
        verbose=False,
    )
    assert rc == 1


def test_extension_audit_accepts_embedded_manifest(tmp_path: Path) -> None:
    wheel_name = "demo_ext-0.1.0-py3-molt_abi1-x86_64_unknown_linux_gnu.whl"
    wheel_path = tmp_path / wheel_name
    manifest = {
        "schema_version": 1,
        "name": "demo-ext",
        "version": "0.1.0",
        "module": "demoext",
        "molt_c_api_version": "1",
        "abi_tag": "molt_abi1",
        "python_tag": "py3",
        "target_python": "py312",
        "target_triple": "x86_64-unknown-linux-gnu",
        "platform_tag": "x86_64_unknown_linux_gnu",
        "capabilities": ["fs.read"],
        "wheel": wheel_name,
        "extension": "demoext.so",
    }

    with zipfile.ZipFile(wheel_path, "w") as zf:
        zf.writestr("demoext.so", b"shared")
        zf.writestr("extension_manifest.json", json.dumps(manifest))

    rc = cli.extension_audit(
        path=str(wheel_path),
        require_capabilities=True,
        require_abi="1",
        json_output=False,
        verbose=False,
    )
    assert rc == 0


def test_extension_audit_requires_checksums_when_requested(tmp_path: Path) -> None:
    _manifest_path, wheel_path = _write_extension_wheel(
        tmp_path, include_checksums=False
    )
    rc = cli.extension_audit(
        path=str(wheel_path),
        require_capabilities=True,
        require_abi="1",
        require_checksum=True,
        json_output=False,
        verbose=False,
    )
    assert rc == 1


def test_extension_audit_requires_manifest_python_export(
    tmp_path: Path,
    capsys,
) -> None:
    out_dir = tmp_path / "dist"
    out_dir.mkdir()
    manifest = {
        "schema_version": 1,
        "name": "numpy-probe",
        "version": "0.1.0",
        "module": "numpy._core._multiarray_umath",
        "molt_c_api_version": "1",
        "abi_tag": "molt_abi1",
        "python_tag": "py3",
        "target_python": "py312",
        "target_triple": "wasm32-wasip1",
        "platform_tag": "wasm32_wasip1",
        "runtime_linkage": "static_link",
        "artifact_kind": "wasm_relocatable_object",
        "capabilities": ["ffi.unsafe"],
        "extension": "_multiarray_umath.molt.wasm",
    }
    (out_dir / "extension_manifest.json").write_text(
        json.dumps(manifest, indent=2) + "\n",
        encoding="utf-8",
    )

    rc = cli.extension_audit(
        path=str(out_dir),
        require_python_export=["numpy"],
        json_output=False,
        verbose=False,
    )

    assert rc == 1
    out = capsys.readouterr().out
    assert "Missing required python export 'numpy'" in out
    assert "molt extension build --python-export numpy" in out


def test_extension_audit_reports_required_callable_exports_json(
    tmp_path: Path,
    capsys,
) -> None:
    out_dir = tmp_path / "dist"
    out_dir.mkdir()
    manifest = {
        "schema_version": 1,
        "name": "scipy-ndimage-probe",
        "version": "0.1.0",
        "module": "scipy.ndimage._nd_image",
        "molt_c_api_version": "1",
        "abi_tag": "molt_abi1",
        "python_tag": "py3",
        "target_python": "py312",
        "target_triple": "wasm32-wasip1",
        "platform_tag": "wasm32_wasip1",
        "runtime_linkage": "static_link",
        "artifact_kind": "wasm_relocatable_object",
        "capabilities": ["ffi.unsafe"],
        "extension": "_nd_image.molt.wasm",
        "python_exports": ["scipy.ndimage.distance_transform_edt"],
        "callable_exports": [
            {
                "module": "scipy.ndimage",
                "name": "distance_transform_edt",
                "binding": "module_attr",
                "abi": "molt.object_call_v1",
                "deterministic": True,
            }
        ],
    }
    (out_dir / "extension_manifest.json").write_text(
        json.dumps(manifest, indent=2) + "\n",
        encoding="utf-8",
    )

    rc = cli.extension_audit(
        path=str(out_dir),
        require_python_export=["scipy.ndimage.distance_transform_edt"],
        require_callable_export=["scipy.ndimage.distance_transform_edt"],
        json_output=True,
        verbose=False,
    )

    assert rc == 0
    payload = json.loads(capsys.readouterr().out)
    assert payload["status"] == "ok"
    assert payload["data"]["python_exports"] == ["scipy.ndimage.distance_transform_edt"]
    assert payload["data"]["callable_exports"] == [
        "scipy.ndimage.distance_transform_edt"
    ]
    assert payload["data"]["missing_python_exports"] == []
    assert payload["data"]["missing_callable_exports"] == []


def test_extension_seal_publishes_package_root_export_for_existing_static_artifact(
    tmp_path: Path,
    capsys,
) -> None:
    source_root = tmp_path / "source"
    package_dir = source_root / "numpy"
    artifact_dir = package_dir / "_core"
    artifact_dir.mkdir(parents=True)
    (package_dir / "__init__.py").write_text("VALUE = 1\n", encoding="utf-8")
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
        "molt_c_api_version": "1",
        "abi_tag": "molt_abi1",
        "python_tag": "py3",
        "target_python": "py312",
        "target_triple": "wasm32-wasip1",
        "platform_tag": "wasm32_wasip1",
        "loader_kind": "libmolt_source",
        "init_symbol": "PyInit__multiarray_umath",
        "runtime_linkage": "static_link",
        "artifact_kind": "wasm_relocatable_object",
        "link_requirements": {
            "target_triple": "wasm32-wasip1",
            "items": [],
            "retained_symbols": [],
        },
        "capabilities": ["module.extension.exec"],
        "extension": "numpy/_core/_multiarray_umath.molt.wasm",
        "extension_sha256": extension_sha256,
        "provided_capsules": [],
        "object_closure": {
            "schema_version": SOURCE_EXTENSION_OBJECT_CLOSURE_SCHEMA_VERSION,
            "root_symbol": "PyInit__multiarray_umath",
            "init_symbol_owner": "0_multiarray.o",
            "closure_sha256": extension_sha256,
            "runtime_symbols": [],
            "required_capsules": [],
            "objects": [
                {
                    "object": "0_multiarray.o",
                    "language": "c",
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
    _finalize_test_extension_object_closure(manifest, artifact_path=artifact_path)
    manifest_path = source_root / "extension_manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    sealed_root = tmp_path / "sealed"

    rc = cli.extension_seal(
        path=str(manifest_path),
        out_dir=str(sealed_root),
        python_export=["numpy"],
        json_output=True,
        verbose=False,
    )

    assert rc == 0
    payload = json.loads(capsys.readouterr().out)
    assert payload["data"]["python_exports"] == ["numpy"]
    sealed_manifest = json.loads(
        (
            sealed_root
            / "numpy"
            / "_core"
            / "_multiarray_umath.molt.wasm.extension_manifest.json"
        ).read_text(encoding="utf-8")
    )
    assert sealed_manifest["extension"] == "_multiarray_umath.molt.wasm"
    assert sealed_manifest["python_exports"] == ["numpy"]
    assert (sealed_root / "numpy" / "__init__.py").exists()
    assert (sealed_root / "numpy" / "_core" / "__init__.py").exists()

    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(sealed_root,),
        admitted_packages={"numpy"},
        target="wasm",
        required_modules={"numpy"},
    )

    assert errors == []
    assert plan is not None
    assert plan.native_python_export_names() == frozenset({"numpy"})
    assert plan.native_module_names() >= frozenset(
        {
            "numpy",
            "numpy._core",
            "numpy._core._multiarray_umath",
        }
    )


def _minimal_static_extension_manifest(
    *,
    artifact_dir: Path,
    molt_c_api_version: str,
    abi_tag: str,
) -> tuple[dict, Path]:
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
        "target_python": "py312",
        "target_triple": "wasm32-wasip1",
        "platform_tag": "wasm32_wasip1",
        "loader_kind": "libmolt_source",
        "init_symbol": "PyInit__multiarray_umath",
        "runtime_linkage": "static_link",
        "artifact_kind": "wasm_relocatable_object",
        "link_requirements": {
            "target_triple": "wasm32-wasip1",
            "items": [],
            "retained_symbols": [],
        },
        "capabilities": ["module.extension.exec"],
        "extension": "numpy/_core/_multiarray_umath.molt.wasm",
        "extension_sha256": extension_sha256,
        "provided_capsules": [],
        "object_closure": {
            "schema_version": SOURCE_EXTENSION_OBJECT_CLOSURE_SCHEMA_VERSION,
            "root_symbol": "PyInit__multiarray_umath",
            "init_symbol_owner": "0_multiarray.o",
            "closure_sha256": extension_sha256,
            "runtime_symbols": [],
            "required_capsules": [],
            "objects": [
                {
                    "object": "0_multiarray.o",
                    "language": "c",
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
    _finalize_test_extension_object_closure(manifest, artifact_path=artifact_path)
    return manifest, artifact_path


def test_extension_seal_restamps_stale_abi_to_current_runtime(
    tmp_path: Path,
    capsys,
) -> None:
    # A root sealed against an older runtime must not propagate its stale ABI
    # label. Seal is the custody boundary that admits a recompiled artifact into
    # the current runtime, so it re-stamps molt_c_api_version / abi_tag from the
    # runtime header authority rather than copying the build-time label through.
    source_root = tmp_path / "source"
    artifact_dir = source_root / "numpy" / "_core"
    artifact_dir.mkdir(parents=True)
    (source_root / "numpy" / "__init__.py").write_text("V = 1\n", encoding="utf-8")
    (artifact_dir / "__init__.py").write_text("", encoding="utf-8")
    manifest, _artifact = _minimal_static_extension_manifest(
        artifact_dir=artifact_dir,
        molt_c_api_version="1",
        abi_tag="molt_abi1",
    )
    manifest_path = source_root / "extension_manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    sealed_root = tmp_path / "sealed"

    rc = cli.extension_seal(
        path=str(manifest_path),
        out_dir=str(sealed_root),
        python_export=["numpy"],
        json_output=True,
        verbose=False,
    )
    assert rc == 0
    capsys.readouterr()

    expected_major = _default_molt_c_api_version(ROOT).split(".", 1)[0]
    for manifest_rel in (
        "extension_manifest.json",
        "numpy/_core/_multiarray_umath.molt.wasm.extension_manifest.json",
    ):
        sealed_manifest = json.loads(
            (sealed_root / manifest_rel).read_text(encoding="utf-8")
        )
        assert sealed_manifest["molt_c_api_version"] == _CURRENT_MOLT_C_API_VERSION
        assert sealed_manifest["abi_tag"] == f"molt_abi{expected_major}"


def test_extension_seal_fails_closed_on_future_abi(
    tmp_path: Path,
    capsys,
) -> None:
    # An artifact recorded at a newer major ABI than the current runtime is a
    # genuine mismatch: seal must refuse to re-stamp it downward and fail closed.
    source_root = tmp_path / "source"
    artifact_dir = source_root / "numpy" / "_core"
    artifact_dir.mkdir(parents=True)
    (source_root / "numpy" / "__init__.py").write_text("V = 1\n", encoding="utf-8")
    (artifact_dir / "__init__.py").write_text("", encoding="utf-8")
    future_major = int(_default_molt_c_api_version(ROOT).split(".", 1)[0]) + 1
    manifest, _artifact = _minimal_static_extension_manifest(
        artifact_dir=artifact_dir,
        molt_c_api_version=str(future_major),
        abi_tag=f"molt_abi{future_major}",
    )
    manifest_path = source_root / "extension_manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")

    rc = cli.extension_seal(
        path=str(manifest_path),
        out_dir=str(tmp_path / "sealed"),
        python_export=["numpy"],
        json_output=False,
        verbose=False,
    )
    assert rc == 2
    captured = capsys.readouterr()
    assert "newer than the current runtime" in captured.err


def test_extension_seal_derives_source_capsule_requirements_for_static_artifact(
    tmp_path: Path,
    capsys,
) -> None:
    source_root = tmp_path / "source"
    artifact_dir = source_root / "scipy" / "ndimage"
    source_dir = artifact_dir / "src"
    source_dir.mkdir(parents=True)
    (source_root / "scipy" / "__init__.py").write_text("", encoding="utf-8")
    (artifact_dir / "__init__.py").write_text("", encoding="utf-8")
    source_path = source_dir / "nd_image.c"
    source_path.write_text(
        "int PyInit__nd_image(void) {\n"
        "    if (_import_array() < 0) { return -1; }\n"
        "    return 0;\n"
        "}\n",
        encoding="utf-8",
    )
    artifact_bytes = _wasm_exporting_i64_unary_symbol("PyInit__nd_image")
    artifact_path = artifact_dir / "_nd_image.molt.wasm"
    artifact_path.write_bytes(artifact_bytes)
    extension_sha256 = hashlib.sha256(artifact_bytes).hexdigest()
    source_sha256 = hashlib.sha256(source_path.read_bytes()).hexdigest()
    capsule = "numpy.core._multiarray_umath._ARRAY_API"
    manifest = {
        "schema_version": 1,
        "name": "scipy-ndimage-probe",
        "version": "0.1.0",
        "module": "scipy.ndimage._nd_image",
        "molt_c_api_version": "1",
        "abi_tag": "molt_abi1",
        "python_tag": "py3",
        "target_python": "py312",
        "target_triple": "wasm32-wasip1",
        "platform_tag": "wasm32_wasip1",
        "loader_kind": "libmolt_source",
        "init_symbol": "PyInit__nd_image",
        "runtime_linkage": "static_link",
        "artifact_kind": "wasm_relocatable_object",
        "link_requirements": {
            "target_triple": "wasm32-wasip1",
            "items": [],
            "retained_symbols": [],
        },
        "capabilities": ["module.extension.exec"],
        "extension": "scipy/ndimage/_nd_image.molt.wasm",
        "extension_sha256": extension_sha256,
        "sources": [str(source_path)],
        "provided_capsules": [],
        "object_closure": {
            "schema_version": SOURCE_EXTENSION_OBJECT_CLOSURE_SCHEMA_VERSION,
            "root_symbol": "PyInit__nd_image",
            "init_symbol_owner": "0_nd_image.o",
            "closure_sha256": extension_sha256,
            "runtime_symbols": [],
            "required_capsules": [],
            "objects": [
                {
                    "source": str(source_path),
                    "object": "0_nd_image.o",
                    "language": "c",
                    "source_sha256": source_sha256,
                    "object_sha256": extension_sha256,
                    "defined_symbols": ["PyInit__nd_image"],
                    "undefined_symbols": [],
                    "required_c_api_symbols": [],
                    "required_capsules": [],
                }
            ],
        },
    }
    _finalize_test_extension_object_closure(manifest, artifact_path=artifact_path)
    manifest_path = source_root / "extension_manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    sealed_root = tmp_path / "sealed"

    rc = cli.extension_seal(
        path=str(manifest_path),
        out_dir=str(sealed_root),
        python_export=["scipy.ndimage._nd_image"],
        json_output=True,
        verbose=False,
    )

    assert rc == 0
    capsys.readouterr()
    root_manifest = json.loads(
        (sealed_root / "extension_manifest.json").read_text(encoding="utf-8")
    )
    artifact_manifest = json.loads(
        (
            sealed_root
            / "scipy"
            / "ndimage"
            / "_nd_image.molt.wasm.extension_manifest.json"
        ).read_text(encoding="utf-8")
    )
    for sealed_manifest in (root_manifest, artifact_manifest):
        assert sealed_manifest["object_closure"]["required_capsules"] == [capsule]
        assert _manifest_sequence(
            sealed_manifest,
            sealed_manifest["object_closure"]["objects"][0],
            "required_capsules",
        ) == [capsule]


def test_extension_seal_persists_runtime_python_import_modules_for_static_artifact(
    tmp_path: Path,
    capsys,
) -> None:
    source_root = tmp_path / "source"
    artifact_dir = source_root / "numpy" / "_core"
    source_dir = artifact_dir / "src"
    source_dir.mkdir(parents=True)
    (source_root / "numpy" / "__init__.py").write_text("V = 1\n", encoding="utf-8")
    (artifact_dir / "__init__.py").write_text("", encoding="utf-8")
    source_path = source_dir / "npy_static_data.c"
    source_path.write_text(
        "static int PyInit__multiarray_umath(PyObject *module) {\n"
        '    IMPORT_GLOBAL("numpy._core._exceptions", ComplexWarning, warning_obj);\n'
        "    return 0;\n"
        "}\n",
        encoding="utf-8",
    )
    artifact_bytes = _wasm_exporting_i64_unary_symbol("PyInit__multiarray_umath")
    artifact_path = artifact_dir / "_multiarray_umath.molt.wasm"
    artifact_path.write_bytes(artifact_bytes)
    extension_sha256 = hashlib.sha256(artifact_bytes).hexdigest()
    source_sha256 = hashlib.sha256(source_path.read_bytes()).hexdigest()
    manifest = {
        "schema_version": 1,
        "name": "numpy-probe",
        "version": "0.1.0",
        "module": "numpy._core._multiarray_umath",
        "molt_c_api_version": "1",
        "abi_tag": "molt_abi1",
        "python_tag": "py3",
        "target_python": "py312",
        "target_triple": "wasm32-wasip1",
        "platform_tag": "wasm32_wasip1",
        "loader_kind": "libmolt_source",
        "init_symbol": "PyInit__multiarray_umath",
        "runtime_linkage": "static_link",
        "artifact_kind": "wasm_relocatable_object",
        "link_requirements": {
            "target_triple": "wasm32-wasip1",
            "items": [],
            "retained_symbols": [],
        },
        "capabilities": ["module.extension.exec"],
        "extension": "numpy/_core/_multiarray_umath.molt.wasm",
        "extension_sha256": extension_sha256,
        "provided_capsules": [],
        "object_closure": {
            "schema_version": SOURCE_EXTENSION_OBJECT_CLOSURE_SCHEMA_VERSION,
            "root_symbol": "PyInit__multiarray_umath",
            "init_symbol_owner": "0_multiarray.o",
            "closure_sha256": extension_sha256,
            "runtime_symbols": [],
            "required_capsules": [],
            "objects": [
                {
                    "source": str(source_path),
                    "object": "0_multiarray.o",
                    "language": "c",
                    "source_sha256": source_sha256,
                    "object_sha256": extension_sha256,
                    "defined_symbols": ["PyInit__multiarray_umath"],
                    "undefined_symbols": [],
                    "required_c_api_symbols": [],
                    "required_capsules": [],
                }
            ],
        },
    }
    _finalize_test_extension_object_closure(manifest, artifact_path=artifact_path)
    manifest_path = source_root / "extension_manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    sealed_root = tmp_path / "sealed"

    rc = cli.extension_seal(
        path=str(manifest_path),
        out_dir=str(sealed_root),
        python_export=["numpy"],
        json_output=True,
        verbose=False,
    )

    assert rc == 0
    capsys.readouterr()
    for manifest_rel in (
        "extension_manifest.json",
        "numpy/_core/_multiarray_umath.molt.wasm.extension_manifest.json",
    ):
        sealed_manifest = json.loads(
            (sealed_root / manifest_rel).read_text(encoding="utf-8")
        )
        assert sealed_manifest["runtime_python_import_modules"] == [
            "numpy._core._exceptions"
        ]


def test_extension_seal_retains_all_inputs_for_reseal_after_source_deletion(
    tmp_path: Path,
    capsys,
) -> None:
    source_root = tmp_path / "source"
    build_root = tmp_path / "build"
    artifact_dir = source_root / "numpy" / "_core"
    source_dir = artifact_dir / "src"
    generated_dir = build_root / "numpy" / "_core" / "libloops.a.p"
    source_dir.mkdir(parents=True)
    generated_dir.mkdir(parents=True)
    (source_root / "numpy" / "__init__.py").write_text("V = 1\n", encoding="utf-8")
    (artifact_dir / "__init__.py").write_text("", encoding="utf-8")
    (build_root / "intro-targets.json").write_text("[]\n", encoding="utf-8")
    (build_root / "compile_commands.json").write_text("[]\n", encoding="utf-8")
    source_path = source_dir / "npy_static_data.c"
    source_path.write_text(
        "static int PyInit__multiarray_umath(PyObject *module) { return 0; }\n",
        encoding="utf-8",
    )
    generated_source_path = generated_dir / "loops.dispatch.c"
    generated_source_path.write_text(
        "int npy_generated_loop(void) { return 1; }\n",
        encoding="utf-8",
    )
    artifact_bytes = _wasm_exporting_i64_unary_symbol("PyInit__multiarray_umath")
    artifact_path = artifact_dir / "_multiarray_umath.molt.wasm"
    artifact_path.write_bytes(artifact_bytes)
    extension_sha256 = hashlib.sha256(artifact_bytes).hexdigest()
    source_sha256 = hashlib.sha256(source_path.read_bytes()).hexdigest()
    generated_source_sha256 = hashlib.sha256(
        generated_source_path.read_bytes()
    ).hexdigest()
    manifest = {
        "schema_version": 1,
        "name": "numpy-probe",
        "version": "0.1.0",
        "module": "numpy._core._multiarray_umath",
        "molt_c_api_version": "1",
        "abi_tag": "molt_abi1",
        "python_tag": "py3",
        "target_python": "py312",
        "target_triple": "wasm32-wasip1",
        "platform_tag": "wasm32_wasip1",
        "loader_kind": "libmolt_source",
        "init_symbol": "PyInit__multiarray_umath",
        "runtime_linkage": "static_link",
        "artifact_kind": "wasm_relocatable_object",
        "link_requirements": {
            "target_triple": "wasm32-wasip1",
            "items": [],
            "retained_symbols": [],
        },
        "capabilities": ["module.extension.exec"],
        "extension": "numpy/_core/_multiarray_umath.molt.wasm",
        "extension_sha256": extension_sha256,
        "provided_capsules": [],
        "source_plan": {
            "kind": "meson-intro-targets",
            "plan": str((build_root / "intro-targets.json").resolve()),
            "source_root": str(source_root.resolve()),
            "build_root": str(build_root.resolve()),
            "compile_commands": str((build_root / "compile_commands.json").resolve()),
            "digest": "source-plan-digest",
        },
        "object_closure": {
            "schema_version": SOURCE_EXTENSION_OBJECT_CLOSURE_SCHEMA_VERSION,
            "root_symbol": "PyInit__multiarray_umath",
            "init_symbol_owner": "0_multiarray.o",
            "closure_sha256": extension_sha256,
            "runtime_symbols": [],
            "required_capsules": [],
            "objects": [
                {
                    "source": str(source_path),
                    "object": "0_multiarray.o",
                    "language": "c",
                    "source_sha256": source_sha256,
                    "object_sha256": extension_sha256,
                    "defined_symbols": ["PyInit__multiarray_umath"],
                    "undefined_symbols": [],
                    "required_c_api_symbols": [],
                    "required_capsules": [],
                    "producer_unit": {
                        "target_id": "_multiarray_umath",
                        "object": "_multiarray_umath.so.p/0.o",
                    },
                },
                {
                    "source": str(generated_source_path),
                    "object": "1_loops.o",
                    "producer_unit": {"target_id": "loops", "object": "loops.a.p/1.o"},
                    "language": "c",
                    "source_sha256": generated_source_sha256,
                    "object_sha256": extension_sha256,
                    "defined_symbols": ["npy_generated_loop"],
                    "undefined_symbols": [],
                    "required_c_api_symbols": [],
                    "required_capsules": [],
                },
            ],
        },
    }
    _finalize_test_extension_object_closure(manifest, artifact_path=artifact_path)
    manifest_path = source_root / "extension_manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    sealed_root = tmp_path / "sealed"

    rc = cli.extension_seal(
        path=str(manifest_path),
        out_dir=str(sealed_root),
        python_export=["numpy"],
        json_output=True,
        verbose=False,
    )

    assert rc == 0
    capsys.readouterr()
    expected_sources = {
        (
            sealed_root
            / "provenance"
            / "compiled-inputs"
            / "sha256"
            / digest[:2]
            / digest
        ).resolve()
        for digest in (source_sha256, generated_source_sha256)
    }
    source_path.unlink()
    generated_source_path.unlink()
    for manifest_rel in (
        "extension_manifest.json",
        "numpy/_core/_multiarray_umath.molt.wasm.extension_manifest.json",
    ):
        sealed_manifest_path = sealed_root / manifest_rel
        sealed_manifest = json.loads(sealed_manifest_path.read_text(encoding="utf-8"))
        sealed_sources = {
            item["source"] for item in sealed_manifest["object_closure"]["objects"]
        }
        assert {
            (sealed_manifest_path.parent / source).resolve()
            for source in sealed_sources
        } == expected_sources
        assert all(not Path(source).is_absolute() for source in sealed_sources)
        for item in sealed_manifest["object_closure"]["objects"]:
            resolved, errors = resolve_source_extension_manifest_input(
                item["source"],
                manifest_path=sealed_manifest_path,
                expected_sha256=item["source_sha256"],
            )
            assert errors == []
            assert resolved is not None
            assert resolved.is_file()
        resealed_root = tmp_path / (
            "resealed-root"
            if manifest_rel == "extension_manifest.json"
            else "resealed-artifact"
        )
        assert (
            cli.extension_seal(
                path=str(sealed_manifest_path),
                out_dir=str(resealed_root),
                python_export=["numpy"],
                json_output=True,
            )
            == 0
        )
        resealed = json.loads((resealed_root / "extension_manifest.json").read_text())
        assert resealed["runtime_python_import_modules"] == []
        assert set(resealed["sources"]) == {
            path.relative_to(sealed_root).as_posix() for path in expected_sources
        }


def test_extension_seal_rejects_stale_sealed_sources_without_runtime_import_custody(
    tmp_path: Path,
    capsys,
) -> None:
    source_root = tmp_path / "source"
    artifact_dir = source_root / "numpy" / "_core"
    artifact_dir.mkdir(parents=True)
    (source_root / "numpy" / "__init__.py").write_text("V = 1\n", encoding="utf-8")
    (artifact_dir / "__init__.py").write_text("", encoding="utf-8")
    artifact_bytes = _wasm_exporting_i64_unary_symbol("PyInit__multiarray_umath")
    artifact_path = artifact_dir / "_multiarray_umath.molt.wasm"
    artifact_path.write_bytes(artifact_bytes)
    extension_sha256 = hashlib.sha256(artifact_bytes).hexdigest()
    stale_source = tmp_path / "deleted" / "npy_static_data.c"
    manifest = {
        "schema_version": 1,
        "name": "numpy-probe",
        "version": "0.1.0",
        "module": "numpy._core._multiarray_umath",
        "molt_c_api_version": "1",
        "abi_tag": "molt_abi1",
        "python_tag": "py3",
        "target_python": "py312",
        "target_triple": "wasm32-wasip1",
        "platform_tag": "wasm32_wasip1",
        "loader_kind": "libmolt_source",
        "init_symbol": "PyInit__multiarray_umath",
        "runtime_linkage": "static_link",
        "artifact_kind": "wasm_relocatable_object",
        "link_requirements": {
            "target_triple": "wasm32-wasip1",
            "items": [],
            "retained_symbols": [],
        },
        "capabilities": ["module.extension.exec"],
        "extension": "numpy/_core/_multiarray_umath.molt.wasm",
        "extension_sha256": extension_sha256,
        "sealed_from_manifest_sha256": "0" * 64,
        "sealed_from_extension_sha256": extension_sha256,
        "provided_capsules": [],
        "object_closure": {
            "schema_version": SOURCE_EXTENSION_OBJECT_CLOSURE_SCHEMA_VERSION,
            "root_symbol": "PyInit__multiarray_umath",
            "init_symbol_owner": "0_multiarray.o",
            "closure_sha256": extension_sha256,
            "runtime_symbols": [],
            "required_capsules": [],
            "objects": [
                {
                    "source": str(stale_source),
                    "object": "0_multiarray.o",
                    "language": "c",
                    "source_sha256": "1" * 64,
                    "object_sha256": extension_sha256,
                    "defined_symbols": ["PyInit__multiarray_umath"],
                    "undefined_symbols": [],
                    "required_c_api_symbols": [],
                    "required_capsules": [],
                }
            ],
        },
    }
    _finalize_test_extension_object_closure(manifest, artifact_path=artifact_path)
    manifest_path = source_root / "extension_manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")

    rc = cli.extension_seal(
        path=str(manifest_path),
        out_dir=str(tmp_path / "sealed"),
        python_export=["numpy"],
        json_output=False,
        verbose=False,
    )

    assert rc == 2
    captured = capsys.readouterr()
    assert "source missing" in captured.err
    assert "object_closure.objects[0].source" in captured.err
    assert not (tmp_path / "sealed").exists()


def test_extension_seal_rejects_fake_module_attr_callable_export(
    tmp_path: Path,
    capsys,
) -> None:
    source_root = tmp_path / "source"
    artifact_dir = source_root / "scipy" / "ndimage"
    source_dir = artifact_dir / "src"
    source_dir.mkdir(parents=True)
    (source_root / "scipy" / "__init__.py").write_text("VALUE = 1\n", encoding="utf-8")
    (artifact_dir / "__init__.py").write_text("", encoding="utf-8")
    source_path = source_dir / "nd_image.c"
    source_path.write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "static PyObject *native_min_or_max_filter(PyObject *self, PyObject *args) {",
                "    return PyLong_FromLong(1);",
                "}",
                "static PyMethodDef ndimage_methods[] = {",
                '    {"min_or_max_filter", native_min_or_max_filter, METH_VARARGS, ""},',
                "    {NULL, NULL, 0, NULL},",
                "};",
                "",
            ]
        ),
        encoding="utf-8",
    )
    artifact_bytes = _wasm_exporting_i64_unary_symbol("PyInit__nd_image")
    artifact_path = artifact_dir / "_nd_image.molt.wasm"
    artifact_path.write_bytes(artifact_bytes)
    extension_sha256 = hashlib.sha256(artifact_bytes).hexdigest()
    manifest = {
        "schema_version": 1,
        "name": "scipy-ndimage-probe",
        "version": "0.1.0",
        "module": "scipy.ndimage._nd_image",
        "molt_c_api_version": "1",
        "abi_tag": "molt_abi1",
        "python_tag": "py3",
        "target_python": "py312",
        "target_triple": "wasm32-wasip1",
        "platform_tag": "wasm32_wasip1",
        "loader_kind": "libmolt_source",
        "init_symbol": "PyInit__nd_image",
        "runtime_linkage": "static_link",
        "artifact_kind": "wasm_relocatable_object",
        "capabilities": ["module.extension.exec"],
        "extension": "scipy/ndimage/_nd_image.molt.wasm",
        "extension_sha256": extension_sha256,
        "sources": [str(source_path)],
        "provided_capsules": [],
        "object_closure": {
            "schema_version": SOURCE_EXTENSION_OBJECT_CLOSURE_SCHEMA_VERSION,
            "root_symbol": "PyInit__nd_image",
            "init_symbol_owner": "0_nd_image.o",
            "closure_sha256": extension_sha256,
            "runtime_symbols": [],
            "required_capsules": [],
            "objects": [
                {
                    "object": "0_nd_image.o",
                    "language": "c",
                    "source_sha256": hashlib.sha256(
                        source_path.read_bytes()
                    ).hexdigest(),
                    "object_sha256": extension_sha256,
                    "defined_symbols": ["PyInit__nd_image"],
                    "undefined_symbols": [],
                    "required_c_api_symbols": [],
                    "required_capsules": [],
                }
            ],
        },
    }
    _finalize_test_extension_object_closure(manifest, artifact_path=artifact_path)
    manifest_path = source_root / "extension_manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")

    rc = cli.extension_seal(
        path=str(manifest_path),
        out_dir=str(tmp_path / "sealed"),
        python_export=["scipy.ndimage.distance_transform_edt"],
        callable_export_json=[
            json.dumps(
                {
                    "module": "scipy.ndimage",
                    "name": "distance_transform_edt",
                    "binding": "module_attr",
                    "abi": "molt.object_call_v1",
                }
            )
        ],
        json_output=False,
        verbose=False,
    )

    assert rc == 2
    captured = capsys.readouterr()
    assert "not declared by a PyMethodDef entry" in captured.err
    assert "scipy.ndimage.distance_transform_edt" in captured.err


def test_extension_seal_publishes_provider_module_support_source(
    tmp_path: Path,
    capsys,
) -> None:
    source_root = tmp_path / "source"
    artifact_dir = source_root / "scipy" / "ndimage"
    source_dir = artifact_dir / "src"
    source_dir.mkdir(parents=True)
    (source_root / "scipy" / "__init__.py").write_text("VALUE = 1\n", encoding="utf-8")
    (artifact_dir / "__init__.py").write_text("", encoding="utf-8")
    provider_source = artifact_dir / "_morphology.py"
    provider_source.write_text(
        "from . import _nd_image\n"
        "from . import _ni_docstrings\n"
        "def distance_transform_edt(mask):\n"
        "    _ni_docstrings.docfiller(distance_transform_edt)\n"
        "    return _nd_image.euclidean_feature_transform(mask)\n",
        encoding="utf-8",
    )
    docstrings_source = artifact_dir / "_ni_docstrings.py"
    docstrings_source.write_text(
        "docfiller = lambda func: func\n",
        encoding="utf-8",
    )
    stale_provider_source = artifact_dir / "_stale.py"
    stale_provider_source.write_text(
        "def stale_distance_transform(mask):\n    return mask\n",
        encoding="utf-8",
    )
    helper_source = tmp_path / "upstream_numpy" / "numpy" / "exceptions.py"
    helper_source.parent.mkdir(parents=True)
    helper_source.write_text(
        "class AxisError(ValueError, IndexError):\n    pass\n",
        encoding="utf-8",
    )
    source_path = source_dir / "nd_image.c"
    source_path.write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "static PyObject *native_euclidean_feature_transform(PyObject *self, PyObject *args) {",
                "    return PyLong_FromLong(1);",
                "}",
                "static PyMethodDef ndimage_methods[] = {",
                '    {"euclidean_feature_transform", native_euclidean_feature_transform, METH_VARARGS, ""},',
                "    {NULL, NULL, 0, NULL},",
                "};",
                "",
            ]
        ),
        encoding="utf-8",
    )
    artifact_bytes = _wasm_exporting_i64_unary_symbol("PyInit__nd_image")
    artifact_path = artifact_dir / "_nd_image.molt.wasm"
    artifact_path.write_bytes(artifact_bytes)
    extension_sha256 = hashlib.sha256(artifact_bytes).hexdigest()
    manifest = {
        "schema_version": 1,
        "name": "scipy-ndimage-probe",
        "version": "0.1.0",
        "module": "scipy.ndimage._nd_image",
        "molt_c_api_version": "1",
        "abi_tag": "molt_abi1",
        "python_tag": "py3",
        "target_python": "py312",
        "target_triple": "wasm32-wasip1",
        "platform_tag": "wasm32_wasip1",
        "loader_kind": "libmolt_source",
        "init_symbol": "PyInit__nd_image",
        "runtime_linkage": "static_link",
        "artifact_kind": "wasm_relocatable_object",
        "link_requirements": {
            "target_triple": "wasm32-wasip1",
            "items": [],
            "retained_symbols": [],
        },
        "capabilities": ["module.extension.exec"],
        "extension": "scipy/ndimage/_nd_image.molt.wasm",
        "extension_sha256": extension_sha256,
        "python_exports": ["scipy.ndimage.stale_distance_transform"],
        "support_files": [
            {
                "path": "scipy/ndimage/_stale.py",
                "sha256": hashlib.sha256(
                    stale_provider_source.read_bytes()
                ).hexdigest(),
            }
        ],
        "callable_exports": [
            {
                "module": "scipy.ndimage",
                "name": "stale_distance_transform",
                "binding": "module_attr",
                "provider_module": "scipy.ndimage._stale",
                "abi": "molt.object_call_v1",
            }
        ],
        "sources": [str(source_path)],
        "provided_capsules": [],
        "object_closure": {
            "schema_version": SOURCE_EXTENSION_OBJECT_CLOSURE_SCHEMA_VERSION,
            "root_symbol": "PyInit__nd_image",
            "init_symbol_owner": "0_nd_image.o",
            "closure_sha256": extension_sha256,
            "runtime_symbols": [],
            "required_capsules": [],
            "objects": [
                {
                    "object": "0_nd_image.o",
                    "language": "c",
                    "source_sha256": hashlib.sha256(
                        source_path.read_bytes()
                    ).hexdigest(),
                    "object_sha256": extension_sha256,
                    "defined_symbols": ["PyInit__nd_image"],
                    "undefined_symbols": [],
                    "required_c_api_symbols": [],
                    "required_capsules": [],
                }
            ],
        },
    }
    _finalize_test_extension_object_closure(manifest, artifact_path=artifact_path)
    manifest_path = source_root / "extension_manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    sealed_root = tmp_path / "sealed"

    rc = cli.extension_seal(
        path=str(manifest_path),
        out_dir=str(sealed_root),
        python_export=["scipy.ndimage.distance_transform_edt"],
        callable_export_json=[
            json.dumps(
                {
                    "module": "scipy.ndimage",
                    "name": "distance_transform_edt",
                    "binding": "module_attr",
                    "provider_module": "scipy.ndimage._morphology",
                    "abi": "molt.object_call_v1",
                }
            )
        ],
        json_output=True,
        verbose=False,
    )

    assert rc == 0
    payload = json.loads(capsys.readouterr().out)
    assert payload["data"]["python_exports"] == ["scipy.ndimage.distance_transform_edt"]
    assert payload["data"]["callable_exports"] == [
        "scipy.ndimage.distance_transform_edt"
    ]
    assert payload["data"]["copied_support_files"] == [
        str(sealed_root / "scipy" / "ndimage" / "_morphology.py"),
        str(sealed_root / "scipy" / "ndimage" / "_ni_docstrings.py"),
    ]
    sealed_manifest = json.loads(
        (
            sealed_root
            / "scipy"
            / "ndimage"
            / "_nd_image.molt.wasm.extension_manifest.json"
        ).read_text(encoding="utf-8")
    )
    assert sealed_manifest["support_files"] == [
        {
            "path": "scipy/ndimage/_morphology.py",
            "sha256": hashlib.sha256(provider_source.read_bytes()).hexdigest(),
        },
        {
            "path": "scipy/ndimage/_ni_docstrings.py",
            "sha256": hashlib.sha256(docstrings_source.read_bytes()).hexdigest(),
        },
    ]
    assert sealed_manifest["python_exports"] == ["scipy.ndimage.distance_transform_edt"]
    assert sealed_manifest["callable_exports"] == [
        {
            "module": "scipy.ndimage",
            "name": "distance_transform_edt",
            "binding": "module_attr",
            "abi": "molt.object_call_v1",
            "provider_module": "scipy.ndimage._morphology",
            "effects": [],
            "deterministic": False,
        }
    ]
    plan, errors = cli._resolve_external_package_native_artifact_plan(
        external_module_roots=(sealed_root,),
        admitted_packages={"scipy"},
        target="wasm",
        required_modules={"scipy.ndimage.distance_transform_edt"},
    )
    assert errors == []
    assert plan is not None
    # The callable-host package init (scipy.ndimage) is part of the native
    # package-init support closure: its __init__.py is compiled so the sealed
    # provider callable can be published on the package. See the closure arc in
    # test_entry_native_package_import_compiles_package_init_closure.
    assert plan.support_source_module_names() == frozenset(
        {"scipy.ndimage._morphology", "scipy.ndimage._ni_docstrings", "scipy.ndimage"}
    )

    alias_root = tmp_path / "sealed_alias"
    rc = cli.extension_seal(
        path=str(manifest_path),
        out_dir=str(alias_root),
        python_export=["scipy.ndimage.distance_transform_edt"],
        callable_export_json=[
            json.dumps(
                {
                    "module": "scipy.ndimage",
                    "name": "distance_transform_edt",
                    "binding": "module_attr",
                    "provider_module": "scipy.ndimage._morphology",
                    "abi": "molt.object_call_v1",
                }
            )
        ],
        support_file=[
            json.dumps(
                {
                    "path": "numpy/exceptions.py",
                    "source": str(helper_source),
                }
            ),
        ],
        json_output=True,
        verbose=False,
    )

    assert rc == 0
    alias_payload = json.loads(capsys.readouterr().out)
    assert alias_payload["data"]["copied_support_files"] == [
        str(alias_root / "numpy" / "exceptions.py"),
        str(alias_root / "scipy" / "ndimage" / "_morphology.py"),
        str(alias_root / "scipy" / "ndimage" / "_ni_docstrings.py"),
    ]
    alias_manifest = json.loads(
        (
            alias_root
            / "scipy"
            / "ndimage"
            / "_nd_image.molt.wasm.extension_manifest.json"
        ).read_text(encoding="utf-8")
    )
    assert alias_manifest["support_files"] == [
        {
            "path": "numpy/exceptions.py",
            "sha256": hashlib.sha256(helper_source.read_bytes()).hexdigest(),
        },
        {
            "path": "scipy/ndimage/_morphology.py",
            "sha256": hashlib.sha256(provider_source.read_bytes()).hexdigest(),
        },
        {
            "path": "scipy/ndimage/_ni_docstrings.py",
            "sha256": hashlib.sha256(docstrings_source.read_bytes()).hexdigest(),
        },
    ]


def _write_auditable_static_link_manifest(
    tmp_path: Path,
) -> tuple[Path, Path, Path, dict[str, object]]:
    out_dir = tmp_path / "dist"
    artifact_dir = out_dir / "nativepkg"
    artifact_dir.mkdir(parents=True)
    artifact_bytes = _wasm_exporting_i64_unary_symbol(
        "PyInit__native", imports=("molt_alloc",)
    )
    artifact_path = artifact_dir / "_native.molt.wasm"
    artifact_path.write_bytes(artifact_bytes)
    manifest = {
        "schema_version": 1,
        "name": "nativepkg-probe",
        "version": "0.1.0",
        "module": "nativepkg._native",
        "molt_c_api_version": "1",
        "abi_tag": "molt_abi1",
        "python_tag": "py3",
        "target_python": "py312",
        "target_triple": "wasm32-wasip1",
        "platform_tag": "wasm32_wasip1",
        "loader_kind": "libmolt_source",
        "runtime_linkage": "static_link",
        "artifact_kind": "wasm_relocatable_object",
        "link_requirements": {
            "target_triple": "wasm32-wasip1",
            "items": [],
            "retained_symbols": [],
        },
        "capabilities": ["ffi.unsafe"],
        "init_symbol": "PyInit__native",
        "extension": "nativepkg/_native.molt.wasm",
        "extension_sha256": hashlib.sha256(artifact_bytes).hexdigest(),
        "object_closure": {
            "schema_version": SOURCE_EXTENSION_OBJECT_CLOSURE_SCHEMA_VERSION,
            "root_symbol": "PyInit__native",
            "init_symbol_owner": "0_native.o",
            "project_generated_c_api_prefixes": ["npy_generated_"],
            "objects": [
                {
                    "object": "0_native.o",
                    "defined_symbols": ["PyInit__native"],
                    "undefined_symbols": ["molt_alloc"],
                    "required_c_api_symbols": ["PyLong_FromLong"],
                    "required_capsules": [
                        "numpy.core._multiarray_umath._ARRAY_API",
                    ],
                    "project_generated_c_api_symbols": ["npy_generated_int8"],
                }
            ],
        },
    }
    _finalize_test_extension_object_closure(manifest, artifact_path=artifact_path)
    manifest_path = out_dir / "extension_manifest.json"
    manifest_path.write_text(
        json.dumps(manifest, indent=2) + "\n",
        encoding="utf-8",
    )
    return out_dir, artifact_path, manifest_path, manifest


def _audit_static_link_manifest(
    out_dir: Path,
    capsys,
    *,
    require_object_closure: bool,
) -> tuple[int, dict[str, object]]:
    rc = cli.extension_audit(
        path=str(out_dir),
        require_loader_kind="libmolt_source",
        require_runtime_linkage="static_link",
        require_artifact_kind="wasm_relocatable_object",
        require_artifact_file=True,
        require_object_closure=require_object_closure,
        require_checksum=True,
        json_output=True,
        verbose=False,
    )
    payload = json.loads(capsys.readouterr().out)
    assert isinstance(payload, dict)
    return rc, payload


def test_extension_audit_requires_static_link_artifact_custody(
    tmp_path: Path,
    capsys,
) -> None:
    out_dir, _artifact_path, manifest_path, manifest = (
        _write_auditable_static_link_manifest(tmp_path)
    )

    rc, payload = _audit_static_link_manifest(
        out_dir,
        capsys,
        require_object_closure=True,
    )

    assert rc == 0
    assert payload["status"] == "ok"
    assert payload["data"]["extension_file_status"] == "ok"
    assert payload["data"]["object_closure"]["present"] is True
    assert payload["data"]["object_closure"]["has_closure_sha256"] is True
    assert payload["data"]["object_closure"]["object_count"] == 1
    assert payload["data"]["object_closure"]["runtime_symbol_count"] == 1
    assert payload["data"]["object_closure"]["defined_symbol_count"] == 1
    assert payload["data"]["object_closure"]["undefined_symbol_count"] == 1
    assert payload["data"]["object_closure"]["required_c_api_symbol_count"] == 1
    assert payload["data"]["object_closure"]["required_capsule_count"] == 1
    assert (
        payload["data"]["object_closure"]["project_generated_c_api_symbol_count"] == 1
    )
    assert (
        payload["data"]["object_closure"]["project_generated_c_api_prefix_count"] == 1
    )

    manifest["artifact_kind"] = "static_archive"
    manifest_path.write_text(
        json.dumps(manifest, indent=2) + "\n",
        encoding="utf-8",
    )
    rc = cli.extension_audit(
        path=str(out_dir),
        json_output=True,
        verbose=False,
    )
    assert rc == 1
    mismatch = json.loads(capsys.readouterr().out)
    assert any(
        "target/artifact mismatch" in error and "wasm_relocatable_object" in error
        for error in mismatch["errors"]
    )


def test_extension_audit_summarizes_validated_optional_closure_fields(
    tmp_path: Path,
    capsys,
) -> None:
    out_dir, _artifact_path, manifest_path, manifest = (
        _write_auditable_static_link_manifest(tmp_path)
    )
    closure = manifest["object_closure"]
    assert isinstance(closure, dict)
    objects = closure["objects"]
    assert isinstance(objects, list)
    for item in objects:
        assert isinstance(item, dict)
        item["required_c_api_symbols"] = []
        item["required_capsules"] = []
        item["project_generated_c_api_symbols"] = []
    optional_fields = (
        "required_c_api_symbols",
        "required_capsules",
        "project_generated_c_api_symbols",
        "project_generated_c_api_prefixes",
    )
    for field in optional_fields:
        closure.pop(field)
    finalize_source_extension_object_closure(manifest)
    manifest_path.write_text(
        json.dumps(manifest, indent=2) + "\n",
        encoding="utf-8",
    )

    rc, payload = _audit_static_link_manifest(
        out_dir,
        capsys,
        require_object_closure=False,
    )

    assert rc == 0
    summary = payload["data"]["object_closure"]
    assert summary["required_c_api_symbol_count"] == 0
    assert summary["required_capsule_count"] == 0
    assert summary["project_generated_c_api_symbol_count"] == 0
    assert summary["project_generated_c_api_prefix_count"] == 0
    assert set(optional_fields).isdisjoint(summary["keys"])


@pytest.mark.parametrize(
    "invalid_closure",
    [
        pytest.param([], id="non-mapping"),
        pytest.param({"schema_version": 2}, id="malformed-mapping"),
    ],
)
def test_extension_audit_rejects_present_invalid_object_closure_without_requirement(
    tmp_path: Path,
    capsys,
    invalid_closure: object,
) -> None:
    out_dir, _artifact_path, manifest_path, manifest = (
        _write_auditable_static_link_manifest(tmp_path)
    )
    manifest["object_closure"] = invalid_closure
    manifest_path.write_text(
        json.dumps(manifest, indent=2) + "\n",
        encoding="utf-8",
    )

    rc, payload = _audit_static_link_manifest(
        out_dir,
        capsys,
        require_object_closure=False,
    )

    assert rc == 1
    assert any("object_closure" in str(error) for error in payload["errors"])


@pytest.mark.parametrize("corruption", ["schema-v1", "closure-hash", "build-hash"])
def test_extension_audit_rejects_false_object_closure_identity(
    tmp_path: Path,
    capsys,
    corruption: str,
) -> None:
    out_dir, _artifact_path, manifest_path, manifest = (
        _write_auditable_static_link_manifest(tmp_path)
    )
    closure = manifest["object_closure"]
    assert isinstance(closure, dict)
    if corruption == "schema-v1":
        closure["schema_version"] = 1
    elif corruption == "closure-hash":
        closure["closure_sha256"] = "f" * 64
    else:
        build = manifest["build"]
        assert isinstance(build, dict)
        build["object_closure_sha256"] = "f" * 64
    manifest_path.write_text(
        json.dumps(manifest, indent=2) + "\n",
        encoding="utf-8",
    )

    rc, payload = _audit_static_link_manifest(
        out_dir,
        capsys,
        require_object_closure=False,
    )

    assert rc == 1
    assert any("object_closure" in str(error) for error in payload["errors"])


def test_extension_audit_rejects_false_compact_object_unit_identity(
    tmp_path: Path,
    capsys,
) -> None:
    out_dir, _artifact_path, manifest_path, manifest = (
        _write_auditable_static_link_manifest(tmp_path)
    )
    compact = _compact_source_extension_manifest(manifest)
    closure = compact["object_closure"]
    assert isinstance(closure, dict)
    objects = closure["objects"]
    assert isinstance(objects, list) and isinstance(objects[0], dict)
    objects[0]["unit_sha256"] = "f" * 64
    manifest_path.write_text(
        json.dumps(compact, indent=2) + "\n",
        encoding="utf-8",
    )

    rc, payload = _audit_static_link_manifest(
        out_dir,
        capsys,
        require_object_closure=False,
    )

    assert rc == 1
    assert any("unit identity is false" in str(error) for error in payload["errors"])


def test_extension_audit_compares_required_artifact_symbol_closure(
    tmp_path: Path,
    capsys,
) -> None:
    out_dir, artifact_path, manifest_path, manifest = (
        _write_auditable_static_link_manifest(tmp_path)
    )
    replacement = _wasm_exporting_i64_unary_symbol(
        "PyInit_other",
        imports=("molt_alloc",),
    )
    artifact_path.write_bytes(replacement)
    manifest["extension_sha256"] = hashlib.sha256(replacement).hexdigest()
    manifest_path.write_text(
        json.dumps(manifest, indent=2) + "\n",
        encoding="utf-8",
    )

    rc, payload = _audit_static_link_manifest(
        out_dir,
        capsys,
        require_object_closure=True,
    )

    assert rc == 1
    assert any(
        "defined-symbol closure differs" in str(error) for error in payload["errors"]
    )


def test_extension_audit_rejects_static_link_artifact_hash_mismatch(
    tmp_path: Path,
    capsys,
) -> None:
    out_dir = tmp_path / "dist"
    out_dir.mkdir()
    artifact_path = out_dir / "_native.molt.wasm"
    artifact_path.write_bytes(b"actual-wasm-bytes")
    manifest = {
        "schema_version": 1,
        "name": "nativepkg-probe",
        "version": "0.1.0",
        "module": "nativepkg._native",
        "molt_c_api_version": "1",
        "abi_tag": "molt_abi1",
        "python_tag": "py3",
        "target_python": "py312",
        "target_triple": "wasm32-wasip1",
        "platform_tag": "wasm32_wasip1",
        "loader_kind": "libmolt_source",
        "runtime_linkage": "static_link",
        "artifact_kind": "wasm_relocatable_object",
        "capabilities": ["ffi.unsafe"],
        "init_symbol": "PyInit__native",
        "extension": "_native.molt.wasm",
        "extension_sha256": hashlib.sha256(b"different-bytes").hexdigest(),
        "object_closure": {
            "runtime_symbols": ["molt_add"],
            "undefined_symbols": ["molt_add"],
        },
    }
    (out_dir / "extension_manifest.json").write_text(
        json.dumps(manifest, indent=2) + "\n",
        encoding="utf-8",
    )

    rc = cli.extension_audit(
        path=str(out_dir),
        require_loader_kind="libmolt_source",
        require_runtime_linkage="static_link",
        require_artifact_kind="wasm_relocatable_object",
        require_artifact_file=True,
        require_object_closure=True,
        json_output=False,
        verbose=False,
    )

    assert rc == 1
    assert (
        "extension_sha256 does not match extension artifact" in capsys.readouterr().out
    )


def test_verify_extension_manifest_requires_checksums(tmp_path: Path) -> None:
    manifest_path, wheel_path = _write_extension_wheel(
        tmp_path, capabilities=[], include_checksums=False
    )
    rc = cli.verify(
        package_path=None,
        manifest_path=str(manifest_path),
        artifact_path=str(wheel_path),
        require_checksum=True,
        json_output=False,
    )
    assert rc == 1


def test_verify_extension_manifest_json_payload(tmp_path: Path, capsys) -> None:
    manifest_path, wheel_path = _write_extension_wheel(
        tmp_path, capabilities=[], include_checksums=True
    )
    rc = cli.verify(
        package_path=None,
        manifest_path=str(manifest_path),
        artifact_path=str(wheel_path),
        require_checksum=True,
        json_output=True,
        require_extension_abi="1",
        extension_metadata=True,
    )
    assert rc == 0
    payload = json.loads(capsys.readouterr().out)
    assert payload["status"] == "ok"
    assert payload["data"]["extension_metadata"] is True
    assert payload["data"]["extension_abi"] == "1"


def test_publish_extension_wheel_requires_checksum_verification(tmp_path: Path) -> None:
    _manifest_path, wheel_path = _write_extension_wheel(
        tmp_path, include_checksums=False
    )
    registry = tmp_path / "registry"
    registry.mkdir()
    rc = cli.publish(
        package_path=str(wheel_path),
        registry=str(registry),
        dry_run=False,
        json_output=False,
        verbose=False,
        deterministic=False,
        capabilities="fs.read",
    )
    assert rc != 0
    assert not (registry / wheel_path.name).exists()


def test_publish_extension_wheel_succeeds_with_checksums(tmp_path: Path) -> None:
    _manifest_path, wheel_path = _write_extension_wheel(
        tmp_path, include_checksums=True
    )
    registry = tmp_path / "registry"
    registry.mkdir()
    rc = cli.publish(
        package_path=str(wheel_path),
        registry=str(registry),
        dry_run=False,
        json_output=False,
        verbose=False,
        deterministic=False,
        capabilities="fs.read",
    )
    assert rc == 0
    assert (registry / wheel_path.name).exists()


def test_python_header_parse_tuple_and_keywords_smoke(tmp_path: Path) -> None:
    clang = shutil.which("clang")
    if clang is None:
        pytest.skip("clang is required for Python.h compatibility smoke test")
    source = tmp_path / "python_h_parse_kw_smoke.c"
    source.write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "",
                "static int parse_pair(PyObject *args, PyObject *kwargs) {",
                '    static char *kwlist[] = {"left", "right", NULL};',
                "    int left = 0;",
                "    int right = 0;",
                '    if (!PyArg_ParseTupleAndKeywords(args, kwargs, "i|i", kwlist, &left, &right)) {',
                "        return -1;",
                "    }",
                "    return left + right;",
                "}",
                "",
                "int parse_positional_only(PyObject *args) {",
                "    int value = 0;",
                '    if (!PyArg_ParseTuple(args, "i", &value)) {',
                "        return -1;",
                "    }",
                "    return value;",
                "}",
                "",
                "int main(void) {",
                "    (void)parse_pair;",
                "    (void)parse_positional_only;",
                "    return 0;",
                "}",
                "",
            ]
        )
    )
    result = run_cli_test_process(
        [
            clang,
            "-std=c11",
            "-Wall",
            "-Wextra",
            "-Werror",
            f"-I{ROOT / 'include'}",
            "-fsyntax-only",
            str(source),
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr


def test_python_header_buffer_descriptor_smoke(tmp_path: Path) -> None:
    clang = shutil.which("clang")
    if clang is None:
        pytest.skip("clang is required for Python.h compatibility smoke test")
    source = tmp_path / "python_h_buffer_smoke.c"
    source.write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "#include <stdint.h>",
                "",
                "static int fillinfo_descriptor(char *data) {",
                "    Py_buffer view;",
                "    if (PyBuffer_FillInfo(&view, NULL, data, 4, 1, PyBUF_SIMPLE) != 0) {",
                "        return -1;",
                "    }",
                "    if (view.buf != (void *)data || view.len != 4 || view.itemsize != 1) {",
                "        return -2;",
                "    }",
                "    if (view.readonly != 1 || view.ndim != 1 || view.internal != NULL) {",
                "        return -3;",
                "    }",
                "    if (view._molt_view.data != (uint8_t *)data || view._molt_view.len != 4) {",
                "        return -4;",
                "    }",
                "    if (view._molt_view.shape[0] != 4 || view._molt_view.strides[0] != 1) {",
                "        return -5;",
                "    }",
                "    PyBuffer_Release(&view);",
                "    if (view.buf != NULL || view._molt_view.data != NULL) {",
                "        return -6;",
                "    }",
                "    return 0;",
                "}",
                "",
                "static int getbuffer_descriptor(PyObject *obj) {",
                "    Py_buffer view;",
                "    int rc = PyObject_GetBuffer(obj, &view, PyBUF_FORMAT | PyBUF_STRIDES);",
                "    if (rc == 0) {",
                "        PyBuffer_Release(&view);",
                "    }",
                "    return rc;",
                "}",
                "",
                "static int full_buffer_flag_descriptor(PyObject *obj) {",
                "    Py_buffer view;",
                "    int rc = PyObject_GetBuffer(obj, &view, PyBUF_FULL_RO);",
                "    if (rc == 0) {",
                "        PyBuffer_Release(&view);",
                "    }",
                "    rc = PyObject_GetBuffer(obj, &view, PyBUF_RECORDS_RO);",
                "    if (rc == 0) {",
                "        PyBuffer_Release(&view);",
                "    }",
                "    rc = PyObject_GetBuffer(obj, &view, PyBUF_CONTIG_RO);",
                "    if (rc == 0) {",
                "        PyBuffer_Release(&view);",
                "    }",
                "    return 0;",
                "}",
                "",
                "static int memoryview_descriptor(PyObject *obj, char *data) {",
                "    Py_buffer view;",
                "    PyObject *from_object;",
                "    PyObject *from_memory;",
                "    PyObject *readonly_memory;",
                "    PyObject *from_buffer;",
                "    Py_buffer *exported;",
                "    PyObject *base;",
                "    if (PyBuffer_FillInfo(&view, obj, data, 4, 0, PyBUF_FORMAT | PyBUF_STRIDES) != 0) {",
                "        return -1;",
                "    }",
                "    from_object = PyMemoryView_FromObject(obj);",
                "    from_memory = PyMemoryView_FromMemory(data, 4, PyBUF_WRITE);",
                "    readonly_memory = PyMemoryView_FromMemory(data, 4, PyBUF_READ);",
                "    from_buffer = PyMemoryView_FromBuffer(&view);",
                "    if (from_object == NULL || from_memory == NULL || readonly_memory == NULL || from_buffer == NULL) {",
                "        return -2;",
                "    }",
                "    exported = PyMemoryView_GET_BUFFER(from_buffer);",
                "    base = PyMemoryView_GET_BASE(from_buffer);",
                "    if (exported == NULL) {",
                "        return -3;",
                "    }",
                "    (void)PyMemoryView_Check(from_object);",
                "    (void)from_memory;",
                "    (void)readonly_memory;",
                "    (void)exported;",
                "    (void)base;",
                "    PyBuffer_Release(&view);",
                "    return 0;",
                "}",
                "",
                "static int memoryview_2d_compact_descriptor(char *data) {",
                "    Py_buffer view;",
                "    Py_ssize_t shape[2] = {2, 2};",
                "    PyObject *from_buffer;",
                "    memset(&view, 0, sizeof(view));",
                "    view.buf = data;",
                "    view.len = 4;",
                "    view.itemsize = 1;",
                "    view.readonly = 1;",
                "    view.ndim = 2;",
                "    view.shape = shape;",
                "    view.strides = NULL;",
                '    view.format = "B";',
                "    from_buffer = PyMemoryView_FromBuffer(&view);",
                "    if (from_buffer == NULL) {",
                "        return -1;",
                "    }",
                "    return 0;",
                "}",
                "",
                "static int memoryview_scalar_descriptor(char *data) {",
                "    Py_buffer view;",
                "    PyObject *from_buffer;",
                "    Py_buffer *exported;",
                "    memset(&view, 0, sizeof(view));",
                "    view.buf = data;",
                "    view.len = 8;",
                "    view.itemsize = 8;",
                "    view.readonly = 1;",
                "    view.ndim = 0;",
                "    view.shape = NULL;",
                "    view.strides = NULL;",
                '    view.format = "d";',
                "    from_buffer = PyMemoryView_FromBuffer(&view);",
                "    if (from_buffer == NULL) {",
                "        return -1;",
                "    }",
                "    exported = PyMemoryView_GET_BUFFER(from_buffer);",
                "    if (exported == NULL || exported->ndim != 0) {",
                "        return -2;",
                "    }",
                "    return 0;",
                "}",
                "",
                "int main(void) {",
                "    char data[4] = {0, 1, 2, 3};",
                "    if (getbuffer_descriptor(NULL) != 0) {",
                "        return -1;",
                "    }",
                "    if (memoryview_descriptor(NULL, data) != 0) {",
                "        return -2;",
                "    }",
                "    if (PyObject_CheckBuffer(NULL) != 0) {",
                "        return -3;",
                "    }",
                "    if (memoryview_2d_compact_descriptor(data) != 0) {",
                "        return -4;",
                "    }",
                "    if (full_buffer_flag_descriptor(NULL) != 0) {",
                "        return -5;",
                "    }",
                "    if (memoryview_scalar_descriptor(data) != 0) {",
                "        return -6;",
                "    }",
                "    return fillinfo_descriptor(data);",
                "}",
                "",
            ]
        )
    )
    result = run_cli_test_process(
        [
            clang,
            "-std=c11",
            "-Wall",
            "-Wextra",
            "-Werror",
            f"-I{ROOT / 'include'}",
            "-fsyntax-only",
            str(source),
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr


def test_python_header_type_module_wrappers_smoke(tmp_path: Path) -> None:
    clang = shutil.which("clang")
    if clang is None:
        pytest.skip("clang is required for Python.h compatibility smoke test")
    source = tmp_path / "python_h_type_module_smoke.c"
    source.write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "#include <structmember.h>",
                "",
                "static PyObject *demo_ping(PyObject *self, PyObject *args) {",
                "    (void)self;",
                "    (void)args;",
                "    return PyLong_FromLong(1);",
                "}",
                "",
                "static PyObject *demo_get(PyObject *self, void *closure) {",
                "    (void)self;",
                "    (void)closure;",
                "    return PyLong_FromLong(2);",
                "}",
                "",
                "static PyMethodDef demo_methods[] = {",
                '    {"static_ping", (void *)demo_ping, METH_STATIC | METH_VARARGS, "static ping"},',
                '    {"cls_ping", (void *)demo_ping, METH_CLASS | METH_VARARGS, "class ping"},',
                "    {NULL, NULL, 0, NULL},",
                "};",
                "",
                "static PyGetSetDef demo_getset[] = {",
                '    {"value", (getter)demo_get, NULL, "value getter", NULL},',
                "    {NULL, NULL, NULL, NULL, NULL},",
                "};",
                "",
                "static PyMemberDef demo_members[] = {",
                '    {"member_value", T_OBJECT, 0, READONLY, "member field"},',
                "    {NULL, 0, 0, 0, NULL},",
                "};",
                "",
                "static PyType_Slot demo_slots[] = {",
                "    {Py_tp_methods, (void *)demo_methods},",
                "    {Py_tp_getset, (void *)demo_getset},",
                "    {Py_tp_members, (void *)demo_members},",
                "    {Py_tp_call, (void *)demo_ping},",
                "    {Py_tp_repr, (void *)demo_ping},",
                "    {Py_tp_str, (void *)demo_ping},",
                "    {Py_nb_add, (void *)demo_ping},",
                "    {Py_nb_subtract, (void *)demo_ping},",
                "    {Py_nb_multiply, (void *)demo_ping},",
                "    {Py_sq_concat, (void *)demo_ping},",
                "    {0, NULL},",
                "};",
                "",
                "static PyType_Spec demo_spec = {",
                '    "demo.TypeSmoke",',
                "    0,",
                "    0,",
                "    Py_TPFLAGS_DEFAULT,",
                "    demo_slots,",
                "};",
                "",
                "int main(void) {",
                '    PyObject *module = PyModule_New("demo");',
                "    PyObject *type_obj = PyType_FromModuleAndSpec(module, &demo_spec, NULL);",
                "    PyObject *module_owner = PyType_GetModule((PyTypeObject *)type_obj);",
                "    void *module_state = PyType_GetModuleState((PyTypeObject *)type_obj);",
                "    PyModuleDef *module_def = PyModule_GetDef(module);",
                "    PyObject *module_by_def = PyType_GetModuleByDef((PyTypeObject *)type_obj, module_def);",
                "    PyTypeObject *owner_type = Py_TYPE(type_obj);",
                "    PyGILState_STATE gil = PyGILState_Ensure();",
                "    PyThreadState *ts = PyThreadState_Get();",
                "    void *mem = PyMem_Malloc(16);",
                "    PyObject *dict_obj = PyDict_New();",
                "    PyObject *tmp_tuple = PyTuple_New(1);",
                "    PyObject *tmp_value = PyLong_FromLong(3);",
                "    int cmp = PyObject_RichCompareBool(type_obj, type_obj, Py_EQ);",
                "    int member_code = Py_T_ULONGLONG + T_ULONGLONG + Py_READONLY + Py_AUDIT_READ + _Py_WRITE_RESTRICTED;",
                "    (void)PyErr_NoMemory;",
                "    (void)PyObject_CallFunctionObjArgs;",
                "    (void)Py_BuildValue;",
                "    (void)PyCapsule_New;",
                "    (void)PyCapsule_GetPointer;",
                "    PyTuple_SET_ITEM(tmp_tuple, 0, tmp_value);",
                "    tmp_value = PyTuple_GET_ITEM(tmp_tuple, 0);",
                "    (void)PyTuple_GET_SIZE(tmp_tuple);",
                "    (void)module_owner;",
                "    (void)module_state;",
                "    (void)module_by_def;",
                "    (void)owner_type;",
                "    (void)ts;",
                "    (void)cmp;",
                "    (void)member_code;",
                "    (void)dict_obj;",
                "    (void)tmp_tuple;",
                "    (void)tmp_value;",
                "    PyMem_Free(mem);",
                "    PyGILState_Release(gil);",
                "    (void)type_obj;",
                "    (void)module;",
                "    return 0;",
                "}",
                "",
            ]
        )
    )
    result = run_cli_test_process(
        [
            clang,
            "-std=c11",
            "-Wall",
            "-Wextra",
            "-Werror",
            f"-I{ROOT / 'include'}",
            "-fsyntax-only",
            str(source),
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr


def test_python_header_source_compat_descriptors_fail_closed() -> None:
    header = (ROOT / "include" / "molt" / "Python.h").read_text(encoding="utf-8")

    assert "requires --abi-tier cpython-abi" in header
    assert (
        '_molt_type_wrap_single_arg_builtin("property", getter_callable)' not in header
    )
    assert "Py_INCREF(Py_None);\n    return Py_None;" not in header


def test_source_compat_tier_does_not_ship_numpy_overlay() -> None:
    source_compat_dirs = tuple(
        path.resolve()
        for path in (
            cli_source_extension_toolchain._source_extension_include_dirs_for_abi_tier(
                molt_root=ROOT,
                abi_tier="source-compat",
            )
        )
    )

    assert source_compat_dirs == ((ROOT / "include").resolve(),)
    forbidden_overlay_paths = (
        ROOT / "include" / "numpy",
        ROOT / "include" / "_numpyconfig.h",
        ROOT / "include" / "arrayobject.h",
        ROOT / "include" / "arraytypes.h",
        ROOT / "include" / "config.h",
        ROOT / "include" / "dispatching.h",
        ROOT / "include" / "extobj.h",
        ROOT / "include" / "npy_cpu_dispatch_config.h",
        ROOT / "include" / "npy_sort.h",
        ROOT / "include" / "templ_common.h",
        ROOT / "include" / "ufunc_object.h",
        ROOT / "include" / "ufunc_type_resolution.h",
        ROOT / "include" / "__multiarray_api.c",
        ROOT / "include" / "__ufunc_api.c",
    )
    for path in forbidden_overlay_paths:
        assert not path.exists(), (
            f"Molt must not ship package-owned NumPy overlay: {path}"
        )


def test_source_compat_tier_does_not_shadow_package_numpy_headers(
    tmp_path: Path,
) -> None:
    clang = shutil.which("clang")
    if clang is None:
        pytest.skip("clang is required for the source-compat numpy custody test")

    package_include = tmp_path / "pkg_numpy_include"
    (package_include / "numpy").mkdir(parents=True)
    (package_include / "numpy" / "arrayobject.h").write_text(
        "\n".join(
            [
                "#ifndef PACKAGE_OWN_NUMPY_ARRAYOBJECT_H",
                "#define PACKAGE_OWN_NUMPY_ARRAYOBJECT_H",
                "#define PACKAGE_OWN_NUMPY_SENTINEL 7",
                "#endif",
                "",
            ]
        ),
        encoding="utf-8",
    )
    source = tmp_path / "source_compat_numpy_custody_probe.c"
    source.write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "#include <numpy/arrayobject.h>",
                "",
                "#ifndef PACKAGE_OWN_NUMPY_SENTINEL",
                '#error "Molt overlay shadowed the package\'s own numpy/ header"',
                "#endif",
                "",
                "int probe(void) { return PACKAGE_OWN_NUMPY_SENTINEL; }",
                "",
            ]
        ),
        encoding="utf-8",
    )
    result = run_cli_test_process(
        [
            clang,
            "-std=c11",
            "-Werror",
            f"-I{ROOT / 'include'}",
            "-I",
            str(package_include),
            "-fsyntax-only",
            str(source),
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr


def test_datetime_header_smoke(tmp_path: Path) -> None:
    clang = shutil.which("clang")
    if clang is None:
        pytest.skip("clang is required for datetime.h compatibility smoke test")
    source = tmp_path / "datetime_h_smoke.c"
    source.write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "#include <datetime.h>",
                "",
                "int main(void) {",
                "    PyDateTime_IMPORT;",
                "    (void)PyDateTimeAPI;",
                "    (void)PyDate_Check;",
                "    (void)PyDateTime_Check;",
                "    (void)PyDelta_Check;",
                "    return 0;",
                "}",
                "",
            ]
        )
    )
    result = run_cli_test_process(
        [
            clang,
            "-std=c11",
            "-Wall",
            "-Wextra",
            "-Werror",
            f"-I{ROOT / 'include'}",
            "-fsyntax-only",
            str(source),
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr


def test_datetime_header_overlay_is_not_a_fail_open_stub() -> None:
    """POISON Lane A #3 — the source-compat include/datetime.h must be the real
    CPython datetime C-API, not the former fail-open, memory-unsafe stub.

    The stub had a 4-byte ``{ int _molt_reserved; }`` PyDateTime_CAPI (so
    ``PyDateTimeAPI->DateType`` read OOB), ``PyDate_/PyDateTime_/PyDelta_Check``
    that unconditionally ``return 0`` (wrong branch, silent), and no
    ``PyTime_Check``. This text-level check needs no toolchain and fails against
    that stub.
    """
    header = (ROOT / "include" / "datetime.h").read_text(encoding="utf-8")
    # The fail-open stub markers must be gone.
    assert "_molt_reserved" not in header, (
        "PyDateTime_CAPI is still the 4-byte placeholder stub"
    )
    assert "return 0;" not in header, (
        "a *_Check still unconditionally returns 0 (fail-open)"
    )
    # The 15-field CPython 3.12 PyDateTime_CAPI must be present, in order.
    for field in (
        "DateType",
        "DateTimeType",
        "TimeType",
        "DeltaType",
        "TZInfoType",
        "TimeZone_UTC",
        "Date_FromDate",
        "DateTime_FromDateAndTime",
        "Time_FromTime",
        "Delta_FromDelta",
        "TimeZone_FromTimeZone",
        "DateTime_FromTimestamp",
        "Date_FromTimestamp",
        "DateTime_FromDateAndTimeAndFold",
        "Time_FromTimeAndFold",
    ):
        assert field in header, f"PyDateTime_CAPI is missing the {field} field"
    # PyDateTime_IMPORT resolves the capsule the runtime publishes.
    assert '"datetime.datetime_CAPI"' in header
    assert "PyCapsule_Import(PyDateTime_CAPSULE_NAME" in header
    # Every check tests the real type via the capsule, and PyTime_Check exists.
    for check in (
        "PyDate_Check",
        "PyDateTime_Check",
        "PyTime_Check",
        "PyDelta_Check",
        "PyTZInfo_Check",
    ):
        assert check in header, f"{check} is missing from the overlay datetime.h"
    assert "PyObject_TypeCheck(op, PyDateTimeAPI->" in header, (
        "checks must be real PyObject_TypeCheck"
    )


def test_datetime_header_overlay_capi_struct_and_checks_compile(tmp_path: Path) -> None:
    """POISON Lane A #3 (compile-level mask-proof) — accessing the PyDateTime_CAPI
    struct fields and every ``*_Check`` (incl. the previously-missing
    ``PyTime_Check`` / ``PyTZInfo_Check``) must compile against the source-compat
    tier. Against the former 4-byte stub these were ``no member named 'DateType'``
    / ``call to undeclared 'PyTime_Check'`` errors — this test fails there.
    """
    clang = shutil.which("clang")
    if clang is None:
        pytest.skip("clang is required for the datetime.h overlay CAPI compile test")
    source = tmp_path / "datetime_h_overlay_capi.c"
    source.write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "#include <datetime.h>",
                "",
                "int main(void) {",
                "    PyObject *op = NULL;",
                "    PyDateTime_IMPORT;",
                "    (void)PyDateTimeAPI->DateType;",
                "    (void)PyDateTimeAPI->DateTimeType;",
                "    (void)PyDateTimeAPI->TimeType;",
                "    (void)PyDateTimeAPI->DeltaType;",
                "    (void)PyDateTimeAPI->TZInfoType;",
                "    (void)PyDateTimeAPI->TimeZone_UTC;",
                "    (void)PyDateTimeAPI->Date_FromDate;",
                "    (void)PyDateTimeAPI->Time_FromTimeAndFold;",
                "    (void)PyDate_Check(op);",
                "    (void)PyDateTime_Check(op);",
                "    (void)PyTime_Check(op);",
                "    (void)PyDelta_Check(op);",
                "    (void)PyTZInfo_Check(op);",
                "    return 0;",
                "}",
                "",
            ]
        ),
        encoding="utf-8",
    )
    result = run_cli_test_process(
        [
            clang,
            "-std=c11",
            "-Wall",
            "-Wextra",
            "-Werror",
            f"-I{ROOT / 'include'}",
            "-fsyntax-only",
            str(source),
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr


def test_datetime_header_cpython_abi_tier_smoke(tmp_path: Path) -> None:
    clang = shutil.which("clang")
    if clang is None:
        pytest.skip("clang is required for datetime.h CPython ABI smoke test")
    source = tmp_path / "datetime_h_cpython_abi_smoke.c"
    source.write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "#include <datetime.h>",
                "",
                "int main(void) {",
                "    PyObject *date;",
                "    PyObject *datetime;",
                "    PyObject *delta;",
                "    PyDateTime_IMPORT;",
                "    date = PyDate_FromDate(2026, 6, 30);",
                "    datetime = PyDateTimeAPI->DateTime_FromDateAndTime(",
                "        2026, 6, 30, 9, 45, 0, 0,",
                "        PyDateTime_TimeZone_UTC, PyDateTimeAPI->DateTimeType);",
                "    delta = PyDelta_FromDSU(1, 2, 3);",
                "    (void)PyDateTime_Check(datetime);",
                "    (void)PyDate_Check(date);",
                "    (void)PyDelta_Check(delta);",
                "    return 0;",
                "}",
                "",
            ]
        ),
        encoding="utf-8",
    )
    result = run_cli_test_process(
        [
            clang,
            "-std=c11",
            "-Wall",
            "-Wextra",
            "-Werror",
            f"-I{ROOT / 'runtime' / 'molt-cpython-abi' / 'include'}",
            f"-I{ROOT / 'include' / 'molt' / 'shared'}",
            "-fsyntax-only",
            str(source),
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr


def test_source_extension_cpython_abi_tier_is_single_self_complete_authority() -> None:
    """The linked tier has one public home plus shared scalar primitives.

    It must NOT also inject the repo-root ``include/`` tier. The source-compat
    tier is a separate libmolt header authority, while package headers such as
    ``numpy/*`` are admitted only through package custody/source plans.
    """
    abi_authority = (ROOT / "runtime" / "molt-cpython-abi" / "include").resolve()
    repo_root_include = (ROOT / "include").resolve()

    cpython_abi_dirs = tuple(
        path.resolve()
        for path in (
            cli_source_extension_toolchain._source_extension_include_dirs_for_abi_tier(
                molt_root=ROOT,
                abi_tier="cpython-abi",
            )
        )
    )
    assert cpython_abi_dirs == (abi_authority, ROOT / "include" / "molt" / "shared")
    assert repo_root_include not in cpython_abi_dirs

    # The ABI authority must be self-complete for a generic C extension: the
    # stock-CPython public headers numpy's _core includes directly all live
    # under the single authority, not in the repo-root include/ tier.
    for header in ("Python.h", "structmember.h", "pymem.h", "pyerrors.h"):
        assert (abi_authority / header).is_file(), header

    # source-compat is unchanged: it owns the repo-root libmolt include/ tier
    # and does NOT pull the standalone ABI authority.
    source_compat_dirs = tuple(
        path.resolve()
        for path in (
            cli_source_extension_toolchain._source_extension_include_dirs_for_abi_tier(
                molt_root=ROOT,
                abi_tier="source-compat",
            )
        )
    )
    assert source_compat_dirs == (repo_root_include,)


@pytest.mark.parametrize("installed", [False, True], ids=["source-tree", "installed"])
def test_cpython_abi_authority_self_complete_without_repo_include_smoke(
    tmp_path: Path,
    installed: bool,
) -> None:
    """Linked headers and their shared primitives work independently of checkout.

    No ``-I include`` on the command line. Proves ``structmember.h`` /
    ``pymem.h`` / ``pyerrors.h`` resolve ``<Python.h>`` to the tier's own
    complete ``Python.h`` and that the tier stands alone.
    """
    clang = shutil.which("clang")
    if clang is None:
        pytest.skip("clang is required for the CPython-ABI authority smoke test")
    abi_authority = ROOT / "runtime" / "molt-cpython-abi" / "include"
    shared = ROOT / "include" / "molt" / "shared"
    if installed:
        abi_authority = Path(shutil.copytree(abi_authority, tmp_path / "sdk" / "abi"))
        shared = Path(shutil.copytree(shared, tmp_path / "primitives"))
    source = tmp_path / "cpython_abi_self_complete_smoke.c"
    source.write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "#include <structmember.h>",
                "#include <pymem.h>",
                "#include <pyerrors.h>",
                "#if SIZEOF_VOID_P != 4 && SIZEOF_VOID_P != 8",
                '#error "SIZEOF_VOID_P must be a preprocessing integer"',
                "#endif",
                "#if SIZEOF_INT != 4 || SIZEOF_LONG_LONG != 8 || SIZEOF_SIZE_T != SIZEOF_VOID_P",
                '#error "scalar model must be preprocessing integers"',
                "#endif",
                "#if LONG_BIT != SIZEOF_LONG * CHAR_BIT",
                '#error "long model must agree"',
                "#endif",
                '_Static_assert(SIZEOF_VOID_P == sizeof(void *), "pointer width");',
                '_Static_assert(SIZEOF_LONG == sizeof(long), "long width");',
                '_Static_assert(SIZEOF_SIZE_T == sizeof(size_t), "size_t width");',
                '_Static_assert(sizeof(((PyTypeObject *)0)->tp_version_tag) == sizeof(unsigned int), "CPython version tag width");',
                "",
                "/* structmember legacy aliases resolve to the Py_T_* / Py_* */",
                "/* constants defined by this tier's own Python.h.           */",
                "static PyMemberDef probe_members[] = {",
                '    {"legacy", T_INT, 0, READONLY, NULL},',
                "    {NULL, 0, 0, 0, NULL},",
                "};",
                "",
                "int probe(void) {",
                "    void *block = PyMem_Malloc(8);",
                "    if (block == NULL) {",
                "        PyErr_NoMemory();",
                "        return -1;",
                "    }",
                "    PyMem_Free(block);",
                "    return probe_members[0].type;",
                "}",
                "",
            ]
        ),
        encoding="utf-8",
    )
    result = run_cli_test_process(
        [
            clang,
            "-std=c11",
            "-Wall",
            "-Wextra",
            "-Werror",
            f"-I{abi_authority}",
            f"-I{shared}",
            "-fsyntax-only",
            str(source),
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr


@pytest.mark.parametrize("installed", [False, True], ids=["source-tree", "installed"])
def test_l7_numeric_headers_expose_one_external_authority(
    tmp_path: Path, installed: bool
) -> None:
    clang = shutil.which("clang")
    if clang is None:
        pytest.skip("clang is required for the L7 integer header smoke test")

    abi_authority = ROOT / "runtime" / "molt-cpython-abi" / "include"
    source_authority = ROOT / "include"
    if installed:
        abi_authority = Path(shutil.copytree(abi_authority, tmp_path / "linked-sdk"))
        source_authority = Path(
            shutil.copytree(source_authority, tmp_path / "source-sdk")
        )
    source = tmp_path / "l7_integer_header_smoke.c"
    smoke_source = "\n".join(
        [
            "#include <Python.h>",
            "#if SIZEOF_VOID_P != SIZEOF_SIZE_T || SIZEOF_INT != 4 || SIZEOF_LONG_LONG != 8",
            '#error "shared scalar widths must be preprocessing constants"',
            "#endif",
            "#if SIZEOF_LONG != 4 && SIZEOF_LONG != 8",
            '#error "shared long width must be a preprocessing constant"',
            "#endif",
            '_Static_assert(SIZEOF_VOID_P == sizeof(void *), "pointer width");',
            '_Static_assert(SIZEOF_LONG == sizeof(long), "long width");',
            '_Static_assert(SIZEOF_INT == sizeof(int), "int width");',
            '_Static_assert(SIZEOF_LONG_LONG == sizeof(long long), "long long width");',
            '_Static_assert(SIZEOF_SIZE_T == sizeof(size_t), "size_t width");',
            '_Static_assert(LONG_BIT == sizeof(long) * CHAR_BIT, "long bits");',
            "int probe(PyObject *value, PyLongObject *long_value, void *out) {",
            "    char *end = NULL;",
            "    unsigned char bytes[2] = {0};",
            "    char packed[8] = {0};",
            "    Py_complex ca = {3.0, 4.0};",
            "    Py_complex cb = {1.0, -2.0};",
            '    PyObject *parsed = PyLong_FromString("0x_FF", &end, 0);',
            "    size_t size = PyLong_AsSize_t(value);",
            "    size_t bits = _PyLong_NumBits(value);",
            "    int compact = PyUnstable_Long_IsCompact(long_value);",
            "    Py_ssize_t compact_value = PyUnstable_Long_CompactValue(long_value);",
            "    int converted = _PyLong_Size_t_Converter(value, out);",
            "    converted += _PyLong_UnsignedShort_Converter(value, out);",
            "    converted += _PyLong_UnsignedInt_Converter(value, out);",
            "    converted += _PyLong_UnsignedLong_Converter(value, out);",
            "    converted += _PyLong_UnsignedLongLong_Converter(value, out);",
            "    converted += _PyLong_AsByteArray(long_value, bytes, 2, 1, 1);",
            "    converted += PyFloat_Pack2(1.0, packed, 0);",
            "    converted += PyFloat_Pack4(1.0, packed, 1);",
            "    converted += PyFloat_Pack8(1.0, packed, 0);",
            "    converted += (int)PyFloat_Unpack2(packed, 0);",
            "    converted += (int)PyFloat_Unpack4(packed, 1);",
            "    converted += (int)PyFloat_Unpack8(packed, 0);",
            "    ca = _Py_c_sum(ca, cb);",
            "    ca = _Py_c_diff(ca, cb);",
            "    ca = _Py_c_neg(ca);",
            "    ca = _Py_c_prod(ca, cb);",
            "    ca = _Py_c_quot(ca, cb);",
            "    ca = _Py_c_pow(ca, cb);",
            "    converted += (int)_Py_c_abs(ca);",
            "    converted += (int)PyFloat_GetMax() + (int)PyFloat_GetMin();",
            "    if (Py_True != (PyObject *)&_Py_TrueStruct || Py_False != (PyObject *)&_Py_FalseStruct) return -2;",
            "    Py_XDECREF(parsed);",
            "    Py_XDECREF(PyLong_GetInfo());",
            "    Py_XDECREF(PyFloat_GetInfo());",
            "    return converted + compact + (int)compact_value + (int)size + (int)bits;",
            "}",
            "",
        ]
    )
    for include_root, header in (
        (abi_authority, "Python.h"),
        (source_authority, "molt/Python.h"),
    ):
        source.write_text(
            smoke_source.replace("#include <Python.h>", f"#include <{header}>"),
            encoding="utf-8",
        )
        result = run_cli_test_process(
            [
                clang,
                "-std=c11",
                "-Wall",
                "-Wextra",
                "-Werror",
                f"-I{include_root}",
                f"-I{source_authority / 'molt' / 'shared'}",
                "-fsyntax-only",
                str(source),
            ],
            capture_output=True,
            text=True,
            check=False,
        )
        assert result.returncode == 0, f"{include_root}: {result.stderr}"

    source_overlay = (ROOT / "include" / "molt" / "Python.h").read_text(
        encoding="utf-8"
    )
    assert "static inline PyObject *PyLong_" not in source_overlay
    assert "static inline long PyLong_" not in source_overlay
    assert "static inline unsigned long PyLong_" not in source_overlay
    assert "static inline size_t PyLong_" not in source_overlay
    assert "static inline PyObject *PyFloat_" not in source_overlay
    assert "static inline double PyFloat_" not in source_overlay
    assert "static inline PyObject *PyComplex_" not in source_overlay
    assert "static inline double PyComplex_" not in source_overlay
    assert "static inline Py_complex PyComplex_" not in source_overlay
    assert "static inline PyObject *PyBool_" not in source_overlay
    for symbol in (
        "PyLong_FromString",
        "PyLong_AsSize_t",
        "PyUnstable_Long_IsCompact",
        "_PyLong_NumBits",
        "_PyLong_FromByteArray",
        "_PyLong_AsByteArray",
        "PyLong_GetInfo",
        "PyFloat_Pack2",
        "PyFloat_Pack4",
        "PyFloat_Pack8",
        "PyFloat_Unpack2",
        "PyFloat_Unpack4",
        "PyFloat_Unpack8",
        "PyFloat_GetInfo",
        "_Py_c_sum",
        "_Py_c_abs",
    ):
        assert any(
            line.startswith("extern ") and symbol in line
            for line in source_overlay.splitlines()
        ), symbol


@pytest.mark.parametrize(
    ("target", "pointer_width", "long_width"),
    [
        ("x86_64-unknown-linux-gnu", 8, 8),
        ("aarch64-unknown-linux-gnu", 8, 8),
        ("x86_64-apple-darwin", 8, 8),
        ("aarch64-apple-darwin", 8, 8),
        ("x86_64-pc-windows-msvc", 8, 4),
        ("aarch64-pc-windows-msvc", 8, 4),
        ("wasm32-unknown-unknown", 4, 4),
    ],
)
def test_shared_c_data_model_target_compiler(
    tmp_path: Path, target: str, pointer_width: int, long_width: int
) -> None:
    """Real target frontend proof, not an OS macro simulation or runtime claim.

    Freestanding Clang provides its target limits/stdint/stddef headers without
    requiring unrelated SDK libc headers. The installed shared primitives must
    agree with both #if and C sizeof on each supported scalar model.
    """
    clang = shutil.which("clang")
    if clang is None:
        pytest.skip("clang is required for target C data-model compilation")
    shared = Path(
        shutil.copytree(ROOT / "include" / "molt" / "shared", tmp_path / "sdk")
    )
    source = tmp_path / "data_model.c"
    source.write_text(
        f"""#include <_c_data_model.h>
#if SIZEOF_VOID_P != {pointer_width} || SIZEOF_LONG != {long_width}
#error "wrong target model"
#endif
#if SIZEOF_INT != 4 || SIZEOF_LONG_LONG != 8 || SIZEOF_SIZE_T != {pointer_width}
#error "wrong scalar model"
#endif
#if LONG_BIT != {long_width * 8}
#error "wrong target long bits"
#endif
_Static_assert(SIZEOF_VOID_P == sizeof(void *), "pointer width");
_Static_assert(SIZEOF_LONG == sizeof(long), "long width");
_Static_assert(SIZEOF_INT == sizeof(int), "int width");
_Static_assert(SIZEOF_LONG_LONG == sizeof(long long), "long long width");
_Static_assert(SIZEOF_SIZE_T == sizeof(size_t), "size_t width");
typedef intptr_t Py_ssize_t;
typedef uint32_t digit;
typedef struct {{ double real, imag; }} Py_complex;
#include <_numeric_scalar_abi.h>
#include <_gil_state_abi.h>
_Static_assert(sizeof(PyObject) == 2 * SIZEOF_VOID_P, "object head");
_Static_assert(offsetof(PyLongObject, long_value) == 2 * SIZEOF_VOID_P, "long head");
_Static_assert(PyGILState_LOCKED == 0 && PyGILState_UNLOCKED == 1, "GIL enum");
""",
        encoding="utf-8",
    )
    command = [
        clang,
        f"--target={target}",
        "-ffreestanding",
        "-std=c11",
        "-Werror",
        f"-I{shared}",
        "-fsyntax-only",
        str(source),
    ]
    result = run_cli_test_process(command, capture_output=True, text=True, check=False)
    assert result.returncode == 0, result.stderr

    # A consumer-provided model must not silently override compiler facts.
    # Exercise every public override on LLP64, where pointer != long is critical.
    if target == "x86_64-pc-windows-msvc":
        for macro in (
            "SIZEOF_VOID_P",
            "SIZEOF_INT",
            "SIZEOF_LONG",
            "SIZEOF_LONG_LONG",
            "SIZEOF_SIZE_T",
            "LONG_BIT",
        ):
            result = run_cli_test_process(
                [*command, f"-D{macro}=1"], capture_output=True, text=True, check=False
            )
            assert result.returncode != 0
            assert f"{macro} conflicts with the target C data model" in result.stderr


def test_cpython_abi_tier_does_not_shadow_package_numpy_headers(
    tmp_path: Path,
) -> None:
    """A package shipping its OWN ``numpy/*`` headers is not shadowed by Molt.

    The cpython-abi tier carries no ``numpy/*`` overlay, so a ``<numpy/...>``
    lookup resolves to the package's own header on the include path -- the exact
    custody the numpy self-recompile needs. Sentinel-guarded so a future
    reintroduction of the overlay into the cpython-abi tier fails this test.
    """
    clang = shutil.which("clang")
    if clang is None:
        pytest.skip("clang is required for the numpy header custody test")

    cpython_abi_dirs = (
        cli_source_extension_toolchain._source_extension_include_dirs_for_abi_tier(
            molt_root=ROOT,
            abi_tier="cpython-abi",
        )
    )
    # The tier must expose no numpy/* header of its own.
    for include_dir in cpython_abi_dirs:
        assert not (Path(include_dir) / "numpy").exists(), (
            f"cpython-abi tier must not ship a numpy/ overlay: {include_dir}"
        )

    # A source-recompiled package ships its own numpy header via package custody.
    package_include = tmp_path / "pkg_numpy_include"
    (package_include / "numpy").mkdir(parents=True)
    (package_include / "numpy" / "arrayobject.h").write_text(
        "\n".join(
            [
                "#ifndef PACKAGE_OWN_NUMPY_ARRAYOBJECT_H",
                "#define PACKAGE_OWN_NUMPY_ARRAYOBJECT_H",
                "#define PACKAGE_OWN_NUMPY_SENTINEL 1",
                "#endif",
                "",
            ]
        ),
        encoding="utf-8",
    )

    source = tmp_path / "numpy_custody_probe.c"
    source.write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "#include <numpy/arrayobject.h>",
                "",
                "#ifndef PACKAGE_OWN_NUMPY_SENTINEL",
                '#error "Molt overlay shadowed the package\'s own numpy/ header"',
                "#endif",
                "",
                "int probe(void) { return PACKAGE_OWN_NUMPY_SENTINEL; }",
                "",
            ]
        ),
        encoding="utf-8",
    )
    include_flags: list[str] = []
    for include_dir in cpython_abi_dirs:
        include_flags.extend(["-I", str(include_dir)])
    include_flags.extend(["-I", str(package_include)])
    result = run_cli_test_process(
        [
            clang,
            "-std=c11",
            "-Werror",
            *include_flags,
            "-fsyntax-only",
            str(source),
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr


def _adversarial_meson_fold_fixture(
    build_root: Path,
    suffix: str = ".a",
) -> tuple[dict[str, Any], dict[str, Any], dict[str, Any]]:
    """Metadata only: no compiler, linker, or archive-content mock is involved."""
    local: dict[str, Any] = {
        "id": "local.library",
        "name": "same",
        "type": "static library",
        "filename": str(build_root / "local" / f"libsame{suffix}"),
        "target_sources": [{"language": "c", "sources": ["local.c"]}],
    }
    other: dict[str, Any] = {
        "id": "other.library",
        "name": "same",
        "type": "static library",
        "filename": str(build_root / "other" / f"libsame{suffix}"),
        "target_sources": [{"language": "c", "sources": ["other.c"]}],
    }
    primary: dict[str, Any] = {
        "id": "extension",
        "name": "extension",
        "type": "shared module",
        "filename": str(build_root / "extension.so"),
        "target_sources": [],
        "linker_parameters": [],
    }
    return primary, local, other


@pytest.mark.parametrize("suffix", [".a", ".lib"])
@pytest.mark.parametrize("separator", ["/", "\\"])
def test_meson_fold_exact_output_beats_same_basename(
    tmp_path: Path,
    suffix: str,
    separator: str,
) -> None:
    primary, local, other = _adversarial_meson_fold_fixture(tmp_path, suffix)
    primary["linker_parameters"] = [f"local{separator}libsame{suffix}", "-lm", "-lm"]
    projection = cli_source_extensions._meson_static_library_projection(
        primary_target=primary,
        payload=[primary, local, other],
        build_root=tmp_path,
    )
    assert [target["id"] for target in projection.targets] == ["local.library"]
    assert projection.link_args == ("-lm", "-lm")


@pytest.mark.parametrize("suffix", [".a", ".lib"])
def test_meson_fold_ambiguous_bare_archive_fails_closed(
    tmp_path: Path,
    suffix: str,
) -> None:
    primary, local, other = _adversarial_meson_fold_fixture(tmp_path, suffix)
    primary["linker_parameters"] = [f"libsame{suffix}"]
    with pytest.raises(ValueError, match="(?i)ambig"):
        cli_source_extensions._meson_static_library_projection(
            primary_target=primary,
            payload=[primary, local, other],
            build_root=tmp_path,
        )


@pytest.mark.parametrize("suffix", [".a", ".lib"])
def test_meson_fold_explicit_external_path_cannot_alias_declared_basename(
    tmp_path: Path,
    suffix: str,
) -> None:
    primary, local, _other = _adversarial_meson_fold_fixture(tmp_path, suffix)
    external = tmp_path / "external" / f"libsame{suffix}"
    external.parent.mkdir()
    external.write_bytes(static_archive_bytes(b"external"))
    primary["linker_parameters"] = [f"external/libsame{suffix}"]
    projection = cli_source_extensions._meson_static_library_projection(
        primary_target=primary,
        payload=[primary, local],
        build_root=tmp_path,
    )
    assert not projection.targets
    assert projection.link_args == (f"external/libsame{suffix}",)


def test_meson_fold_nested_link_operands_preserve_scope_and_repeats(
    tmp_path: Path,
) -> None:
    primary, local, other = _adversarial_meson_fold_fixture(tmp_path)
    primary.pop("linker_parameters")
    primary["target_sources"] = [
        {"language": "c", "sources": ["main.c"], "parameters": ["other/libsame.a"]},
        {
            "linker": ["cc"],
            "parameters": [
                "-Wl,--start-group",
                "-Wl,--whole-archive",
                "local/libsame.a",
                "-Wl,--no-whole-archive",
                "-lm",
                "-lm",
                "-Wl,--end-group",
            ],
        },
    ]
    projection = cli_source_extensions._meson_static_library_projection(
        primary_target=primary,
        payload=[primary, local, other],
        build_root=tmp_path,
    )
    assert [target["id"] for target in projection.targets] == ["local.library"]
    assert set(projection.forced_target_ids) == {"local.library"}
    assert projection.link_args == (
        "-Wl,--start-group",
        "-Wl,--whole-archive",
        "-Wl,--no-whole-archive",
        "-lm",
        "-lm",
        "-Wl,--end-group",
    )


@pytest.mark.parametrize(
    "operand",
    [
        "/WHOLEARCHIVE:local/libsame.lib",
        "-Wl,/WHOLEARCHIVE:local/libsame.lib",
        "-Wl,-force_load,local/libsame.lib",
    ],
)
def test_meson_fold_forced_operand_preserves_member_root_custody(
    tmp_path: Path,
    operand: str,
) -> None:
    primary, local, _other = _adversarial_meson_fold_fixture(tmp_path, ".lib")
    primary["linker_parameters"] = [operand]
    projection = cli_source_extensions._meson_static_library_projection(
        primary_target=primary,
        payload=[primary, local],
        build_root=tmp_path,
    )
    assert [target["id"] for target in projection.targets] == ["local.library"]
    assert set(projection.forced_target_ids) == {"local.library"}
    assert projection.link_args == ()


@pytest.mark.parametrize(
    "directive",
    [
        "--undefined=registration",
        "-Wl,--undefined=registration",
        "/INCLUDE:registration",
    ],
)
def test_meson_fold_retained_symbol_is_forwarded_not_erased(
    tmp_path: Path,
    directive: str,
) -> None:
    primary, local, _other = _adversarial_meson_fold_fixture(tmp_path)
    primary["linker_parameters"] = [directive, "local/libsame.a"]
    projection = cli_source_extensions._meson_static_library_projection(
        primary_target=primary,
        payload=[primary, local],
        build_root=tmp_path,
    )
    assert [target["id"] for target in projection.targets] == ["local.library"]
    assert projection.producer_link_args == (directive, "local/libsame.a")
    from molt.cli.source_extension_link_requirements import (
        source_extension_link_requirements,
    )

    requirements = source_extension_link_requirements(
        projection.link_args,
        target_triple=(
            "x86_64-pc-windows-msvc"
            if directive.startswith("/")
            else "x86_64-unknown-linux-gnu"
        ),
    )
    assert requirements.retained_symbols == ("registration",)


@pytest.mark.parametrize("force_folded", [False, True])
@pytest.mark.parametrize(
    "external_operand",
    [
        "external/libconsumer.a",
        "-lconsumer",
        "/DEFAULTLIB:consumer.lib",
    ],
)
def test_meson_fold_preserves_external_operands_for_typed_custody_validation(
    tmp_path: Path,
    force_folded: bool,
    external_operand: str,
) -> None:
    primary, local, _other = _adversarial_meson_fold_fixture(tmp_path)
    external = tmp_path / "external" / "libconsumer.a"
    external.parent.mkdir()
    external.write_bytes(static_archive_bytes(b"consumer"))
    folded = (
        ["-Wl,--whole-archive", "local/libsame.a", "-Wl,--no-whole-archive"]
        if force_folded
        else ["local/libsame.a"]
    )
    primary["linker_parameters"] = [
        "-Wl,--start-group",
        *folded,
        external_operand,
        "-Wl,--end-group",
    ]
    projection = cli_source_extensions._meson_static_library_projection(
        primary_target=primary,
        payload=[primary, local],
        build_root=tmp_path,
    )
    assert set(projection.forced_target_ids) == (
        {"local.library"} if force_folded else set()
    )
    assert projection.lazy_static_target_ids == (
        () if force_folded else ("local.library",)
    )
    assert [target["id"] for target in projection.targets] == ["local.library"]
    assert projection.link_args == (
        "-Wl,--start-group",
        *(("-Wl,--whole-archive", "-Wl,--no-whole-archive") if force_folded else ()),
        external_operand,
        "-Wl,--end-group",
    )


def test_meson_fold_contradictory_ordered_link_views_fail_closed(
    tmp_path: Path,
) -> None:
    primary, local, _other = _adversarial_meson_fold_fixture(tmp_path)
    primary["linker_parameters"] = ["local/libsame.a", "-lm", "-ldl"]
    primary["target_sources"] = [
        {
            "linker": ["cc"],
            "parameters": ["local/libsame.a", "-ldl", "-lm"],
        }
    ]
    with pytest.raises(ValueError):
        cli_source_extensions._meson_static_library_projection(
            primary_target=primary,
            payload=[primary, local],
            build_root=tmp_path,
        )


def test_meson_fold_import_library_output_has_no_static_source_authority(
    tmp_path: Path,
) -> None:
    primary, local, _other = _adversarial_meson_fold_fixture(tmp_path, ".lib")
    local["type"] = "shared library"
    primary["linker_parameters"] = ["local/libsame.lib"]
    projection = cli_source_extensions._meson_static_library_projection(
        primary_target=primary,
        payload=[primary, local],
        build_root=tmp_path,
    )
    assert not projection.targets
    assert projection.link_args == ("local/libsame.lib",)


def _adversarial_rooted_meson_plan(project_root: Path, root_kind: str) -> Path:
    intro_path = _write_meson_source_plan_project(
        project_root, linked_static_library=True
    )
    targets: list[dict[str, Any]] = json.loads(intro_path.read_text())
    # Root custody is independent of the shared fixture's cleaned-source case.
    for target in targets[1:]:
        for group in target.get("target_sources", []):
            group["generated_sources"] = []
    original_args: list[str] = targets[0]["linker_parameters"]
    if root_kind == "whole-archive":
        targets[0]["linker_parameters"] = [
            f"/WHOLEARCHIVE:{argument}" for argument in original_args
        ]
    elif root_kind == "retained-symbol":
        targets[0]["linker_parameters"] = [
            "/INCLUDE:array__unique_hash",
            *original_args,
        ]
    elif root_kind == "direct-symbol":
        pyproject = project_root / "pyproject.toml"
        pyproject.write_text(
            pyproject.read_text() + "\n[[tool.molt.extension.callable_exports]]\n"
            'module = "pkg.demoext"\n'
            'name = "unique_hash"\n'
            'binding = "direct_symbol"\n'
            'abi = "molt.object_call_v1"\n'
            'symbol = "array__unique_hash"\n'
            "arity = 1\n"
            'effects = ["read"]\n'
            "deterministic = true\n",
            encoding="utf-8",
        )
    elif root_kind != "lazy":
        raise AssertionError(f"unsupported root fixture: {root_kind}")
    intro_path.write_text(json.dumps(targets), encoding="utf-8")
    return intro_path


def _adversarial_meson_append_external_operand(intro_path: Path, operand: str) -> None:
    external = intro_path.parent.parent / "external" / "libconsumer.a"
    external.parent.mkdir()
    external.write_bytes(static_archive_bytes(b"consumer"))
    targets: list[dict[str, Any]] = json.loads(intro_path.read_text(encoding="utf-8"))
    targets[0]["linker_parameters"].append(operand)
    intro_path.write_text(json.dumps(targets), encoding="utf-8")


@pytest.mark.parametrize(
    ("root_kind", "external_operand"),
    [
        ("whole-archive", None),
        ("retained-symbol", None),
        ("direct-symbol", None),
        ("whole-archive", "external/libconsumer.a"),
        ("whole-archive", "/DEFAULTLIB:consumer.lib"),
    ],
)
def test_extension_build_keeps_folded_member_root_not_reachable_from_init(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    root_kind: str,
    external_operand: str | None,
) -> None:
    project_root = tmp_path / "meson_rooted"
    project_root.mkdir()
    intro_path = _adversarial_rooted_meson_plan(project_root, root_kind)
    if external_operand is not None:
        _adversarial_meson_append_external_operand(intro_path, external_operand)
    commands: list[list[str]] = []

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        commands.append(cmd)
        _materialize_fake_extension_command(cmd)
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli_commands, "_run_completed_command", fake_run)
    _install_extension_object_symbol_facts(
        monkeypatch,
        default_init_symbol="PyInit_demoext",
        by_stem={
            "demoext": ({"PyInit_demoext"}, {"helper_generated"}),
            "helper_generated": ({"helper_generated"}, set()),
            "unique": ({"array__unique_hash"}, set()),
        },
    )
    out_dir = project_root / "dist"
    rc = cli_commands.extension_build(
        project=str(project_root),
        out_dir=str(out_dir),
        deterministic=False,
        target="x86_64-pc-windows-msvc",
        json_output=False,
        verbose=False,
    )
    captured = capsys.readouterr()
    assert rc == 0, captured.err
    manifest = json.loads((out_dir / "extension_manifest.json").read_text())
    assert manifest["build"]["linked_object_count"] == 3
    if external_operand is not None:
        assert manifest["link_requirements"]["items"]
    assert any(
        "array__unique_hash" in obj["defined_symbols"]
        for obj in manifest["object_closure"]["objects"]
    )
    if root_kind == "direct-symbol":
        assert any(
            export["binding"] == "direct_symbol"
            and export["symbol"] == "array__unique_hash"
            for export in manifest["callable_exports"]
        )
    archive_cmd = next(cmd for cmd in commands if "rcsD" in cmd)
    assert any("2_unique.o" in part for part in archive_cmd)


@pytest.mark.parametrize(
    ("external_operand", "target"),
    [
        ("external/libconsumer.a", "x86_64-pc-windows-msvc"),
        ("-lconsumer", "x86_64-unknown-linux-gnu"),
        ("/DEFAULTLIB:consumer.lib", "x86_64-pc-windows-msvc"),
    ],
)
def test_extension_build_rejects_lazy_fold_with_typed_external_provider_before_compile(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    external_operand: str,
    target: str,
) -> None:
    project_root = tmp_path / "meson_lazy_external"
    project_root.mkdir()
    intro_path = _adversarial_rooted_meson_plan(project_root, "lazy")
    _adversarial_meson_append_external_operand(intro_path, external_operand)
    commands: list[list[str]] = []

    def reject_compile(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        commands.append(cmd)
        if "-c" in cmd:
            raise AssertionError(
                "opaque-provider custody must be rejected before compilation"
            )
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli_commands, "_run_completed_command", reject_compile)
    out_dir = project_root / "dist"
    rc = cli_commands.extension_build(
        project=str(project_root),
        out_dir=str(out_dir),
        deterministic=False,
        target=target,
        json_output=False,
        verbose=False,
    )
    captured = capsys.readouterr()
    assert rc != 0
    assert (
        "Source-plan lazy static targets lack external member undefined-symbol custody "
        "for typed link requirements: pkg.libunique_hash"
    ) in captured.err
    assert not commands
    assert not (out_dir / "extension_manifest.json").exists()


def test_meson_force_member_target_gate_rejects_lazy_elf_publication(
    tmp_path: Path,
) -> None:
    project_root = tmp_path / "meson_elf"
    project_root.mkdir()
    intro_path = _adversarial_rooted_meson_plan(project_root, "whole-archive")
    plan, errors = (
        cli_source_extensions._load_meson_intro_targets_source_extension_plan(
            plan_path=intro_path,
            project_root=project_root,
            module_name="pkg.demoext",
            selector="pkg.demoext",
            source_root=".",
            build_root="build",
        )
    )
    assert not errors
    assert plan is not None
    assert any(unit.force_include for unit in plan.compile_units)
    errors = cli_source_extensions._validate_source_extension_build_plan_target(
        plan,
        target_triple="x86_64-unknown-linux-gnu",
    )
    assert any("ELF forced source members require" in error for error in errors)
    assert not cli_source_extensions._validate_source_extension_build_plan_target(
        plan,
        target_triple="x86_64-pc-windows-msvc",
    )
