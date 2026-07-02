from __future__ import annotations

import hashlib
import json
import shutil
import subprocess
import zipfile
from pathlib import Path

import molt.cli as cli
import molt.wasm_artifact as wasm_artifact
from molt.cli import commands as cli_commands
from molt.cli import entrypoint_parser as cli_entrypoint_parser
from molt.cli.extension_manifest import _manifest_support_file_payloads
from molt.cli import source_extension_toolchain as cli_source_extension_toolchain
from molt.cli import wasm_toolchain as cli_wasm_toolchain
import pytest

from tests.cli.process_guard import run_cli_test_process


ROOT = Path(__file__).resolve().parents[2]


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
) -> None:
    symbol_facts = by_stem or {}

    def fake_object_symbols(path: Path) -> tuple[set[str], set[str]] | None:
        stem = path.stem.split("_", 1)[1] if "_" in path.stem else path.stem
        if stem in symbol_facts:
            defined, undefined = symbol_facts[stem]
            return set(defined), set(undefined)
        return {default_init_symbol}, {"PyModule_Create", "molt_c_api_version"}

    monkeypatch.setattr(
        cli_commands._source_extensions,
        "_native_object_global_symbol_sets",
        fake_object_symbols,
    )


def _write_fake_wasi_sysroot(root: Path) -> Path:
    sysroot = root / "wasi-sysroot"
    include_dir = sysroot / "include"
    include_dir.mkdir(parents=True)
    (include_dir / "errno.h").write_text("#define EINVAL 28\n")
    return sysroot


def _wasm_exporting_i64_unary_symbol(
    symbol: str,
    *,
    imports: tuple[str, ...] = (),
) -> bytes:
    def uleb(value: int) -> bytes:
        out = bytearray()
        while True:
            byte = value & 0x7F
            value >>= 7
            out.append(byte | 0x80 if value else byte)
            if not value:
                return bytes(out)

    def wasm_string(value: str) -> bytes:
        encoded = value.encode("utf-8")
        return uleb(len(encoded)) + encoded

    def section(section_id: int, payload: bytes) -> bytes:
        return bytes([section_id]) + uleb(len(payload)) + payload

    type_section = uleb(1) + b"\x60" + uleb(1) + b"\x7e" + uleb(1) + b"\x7e"
    import_entries = b"".join(
        wasm_string("env") + wasm_string(import_name) + b"\x00" + uleb(0)
        for import_name in imports
    )
    import_section = section(2, uleb(len(imports)) + import_entries) if imports else b""
    function_section = uleb(1) + uleb(0)
    export_section = uleb(1) + wasm_string(symbol) + b"\x00" + uleb(len(imports))
    body = uleb(0) + b"\x42\x00\x0b"
    code_section = uleb(1) + uleb(len(body)) + body
    return (
        b"\x00asm\x01\x00\x00\x00"
        + section(1, type_section)
        + import_section
        + section(3, function_section)
        + section(7, export_section)
        + section(10, code_section)
    )


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
                'molt_c_api_version = "1"',
                *(extension_extra_lines or []),
                "",
            ]
        )
    )


def _write_meson_source_plan_project(project_root: Path) -> Path:
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
            "linker_parameters": ["-Wl,--as-needed"],
        }
    ]
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
                "-c",
                "pkg/demoext.c",
                "-o",
                "build/demoext.c.o",
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
                "generated/helper_generated.c.o",
            ],
        },
    ]
    (project_root / "build" / "compile_commands.json").write_text(
        json.dumps(compile_commands, indent=2) + "\n",
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
    assert data["symbol_status"]["npyv_u8"] == "missing"
    assert "npyv_u8" in data["missing_symbols"]


def test_extension_scan_numpy_surface_reports_fail_fast_symbols(
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
    assert rc == 0
    payload = json.loads(capsys.readouterr().out)
    assert payload["status"] == "ok"
    data = payload["data"]
    assert data["missing_symbols"] == []
    assert data["fail_fast_symbols"] == []
    assert data["symbol_status"]["PyArray_CastScalarToCtype"] == "source_compile_only"
    assert "PyArray_CastScalarToCtype" in data["source_compile_only_symbols"]
    assert "PyArray_NDIM" in data["source_compile_only_symbols"]
    assert data["symbol_status"]["PyArray_NDIM"] == "source_compile_only"
    assert data["symbol_primitive_class"]["PyArray_NDIM"] == "numpy_c_api"
    assert "PyArray_SIZE" in data["source_compile_only_symbols"]
    assert "PyArray_TYPE" in data["source_compile_only_symbols"]
    assert "PyTypeNum_ISINTEGER" in data["source_compile_only_symbols"]
    assert "PyArray_CheckScalar" in data["source_compile_only_symbols"]
    assert "PyArray_ISDATETIME" in data["source_compile_only_symbols"]
    assert "PyArray_DescrFromScalar" in data["source_compile_only_symbols"]
    assert "PyArray_DescrFromType" in data["source_compile_only_symbols"]
    assert "NPY_INT" in data["source_compile_only_symbols"]
    assert "NPY_NOTYPE" in data["source_compile_only_symbols"]
    assert "NPY_ARRAY_BEHAVED_NS" in data["source_compile_only_symbols"]
    assert "npy_creal" in data["source_compile_only_symbols"]
    assert "npy_cimag" in data["source_compile_only_symbols"]
    assert data["symbol_primitive_class"]["NPY_INT"] == "numpy_c_api"
    assert data["symbol_primitive_class"]["npy_creal"] == "numpy_c_api"
    assert data["primitive_class_counts"]["numpy_c_api"] >= 1
    assert "numpy_c_api" in data["symbols_by_primitive_class"]


def test_cpython_abi_variadic_shim_owns_variadic_exports() -> None:
    shim = (ROOT / "runtime/molt-cpython-abi/shims/pyarg_variadic.c").read_text()
    build_rs = (ROOT / "runtime/molt-cpython-abi/build.rs").read_text()
    runtime_anchor = (
        ROOT / "runtime/molt-runtime/src/c_api/cpython_abi_variadic_exports.rs"
    ).read_text()
    runtime_c_api_mod = (ROOT / "runtime/molt-runtime/src/c_api/mod.rs").read_text()

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
    assert "mod cpython_abi_variadic_exports;" in runtime_c_api_mod
    assert "MOLT_CPYTHON_ABI_VARIADIC_EXPORT_ANCHORS" in runtime_anchor
    assert "static:+whole-archive=molt_pyarg_shims" in build_rs
    for symbol in required_variadic_exports:
        assert f"{symbol}(" in shim
        assert f"fn {symbol}();" in runtime_anchor
        assert symbol in runtime_anchor
    assert "PyOS_snprintf(" in shim
    assert "fn PyOS_snprintf();" in runtime_anchor
    assert "vsnprintf(str, size, format, ap)" in shim


def test_extension_build_emits_wheel_and_manifest(tmp_path: Path, monkeypatch) -> None:
    project_root = tmp_path / "extproj"
    project_root.mkdir()
    _write_extension_project(project_root)

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        out_index = cmd.index("-o")
        out_path = Path(cmd[out_index + 1])
        out_path.parent.mkdir(parents=True, exist_ok=True)
        if "-c" in cmd:
            out_path.write_bytes(b"obj")
        else:
            out_path.write_bytes(b"shared")
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli_commands, "_run_completed_command", fake_run)
    monkeypatch.setattr(
        cli_commands,
        "_shared_library_defines_symbol",
        lambda _path, symbol: (symbol == "PyInit_demoext", None),
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

    wheels = sorted(out_dir.glob("*.whl"))
    assert len(wheels) == 1
    wheel_path = wheels[0]

    manifest_path = out_dir / "extension_manifest.json"
    assert manifest_path.exists()
    manifest = json.loads(manifest_path.read_text())
    assert manifest["wheel"] == wheel_path.name
    assert manifest["molt_c_api_version"] == "1"
    assert manifest["capabilities"] == ["fs.read"]
    assert manifest["abi_tag"] == "molt_abi1"
    assert manifest["loader_kind"] == "libmolt_source"
    assert manifest["init_symbol"] == "PyInit_demoext"
    assert manifest["runtime_linkage"] == "host_resolved"
    assert manifest["artifact_kind"] == "shared_library"

    with zipfile.ZipFile(wheel_path) as zf:
        names = set(zf.namelist())
        assert "extension_manifest.json" in names
        assert manifest["extension"] in names


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
        out_index = cmd.index("-o")
        out_path = Path(cmd[out_index + 1])
        out_path.parent.mkdir(parents=True, exist_ok=True)
        out_path.write_bytes(b"obj" if "-c" in cmd else b"shared")
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli_commands, "_run_completed_command", fake_run)
    monkeypatch.setattr(
        cli_commands,
        "_shared_library_defines_symbol",
        lambda _path, symbol: (symbol == "PyInit_demoext", None),
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
        out_index = cmd.index("-o")
        out_path = Path(cmd[out_index + 1])
        out_path.parent.mkdir(parents=True, exist_ok=True)
        out_path.write_bytes(b"obj" if "-c" in cmd else b"shared")
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli_commands, "_run_completed_command", fake_run)
    monkeypatch.setattr(
        cli_commands,
        "_shared_library_defines_symbol",
        lambda _path, symbol: (symbol == "PyInit_demoext", None),
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
) -> None:
    if shutil.which("clang") is None:
        pytest.skip("clang is required for real libmolt extension build smoke")
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

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        commands.append(cmd)
        out_index = cmd.index("-o")
        out_path = Path(cmd[out_index + 1])
        out_path.parent.mkdir(parents=True, exist_ok=True)
        if "-c" in cmd:
            out_path.write_bytes(b"obj")
        else:
            out_path.write_bytes(b"shared")
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(
        cli_commands, "_ensure_rustup_target", lambda _target, _warnings: True
    )
    monkeypatch.setattr(
        cli_commands.shutil,
        "which",
        lambda tool: "/usr/bin/zig" if tool == "zig" else None,
    )
    monkeypatch.setattr(cli_commands, "_run_completed_command", fake_run)
    monkeypatch.setattr(
        cli_commands,
        "_shared_library_defines_symbol",
        lambda _path, symbol: (symbol == "PyInit_demoext", None),
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
    assert any(
        cmd[:2] == ["zig", "cc"] and "-target" in cmd and "-c" in cmd
        for cmd in commands
    )
    manifest = json.loads((out_dir / "extension_manifest.json").read_text())
    assert manifest["target_triple"] == target
    assert manifest["runtime_linkage"] == "host_resolved"
    assert manifest["artifact_kind"] == "shared_library"


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
        out_index = cmd.index("-o")
        out_path = Path(cmd[out_index + 1])
        out_path.parent.mkdir(parents=True, exist_ok=True)
        out_path.write_bytes(b"obj" if "-c" in cmd else b"shared")
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli_commands, "_run_completed_command", fake_run)
    _install_extension_object_symbol_facts(
        monkeypatch,
        default_init_symbol="PyInit_demoext",
        by_stem={
            "demoext": ({"PyInit_demoext"}, {"PyModule_Create", "helper_generated"}),
            "helper_generated": (
                {"helper_generated"},
                {"PyLong_AsLong", "PyLong_FromLong"},
            ),
        },
    )
    monkeypatch.setattr(
        cli_commands,
        "_shared_library_defines_symbol",
        lambda _path, symbol: (symbol == "PyInit_demoext", None),
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
        if "-c" in cmd and any("demoext.c" in part for part in cmd)
    )
    include_dirs = [
        Path(compile_cmd[idx + 1]).resolve()
        for idx, token in enumerate(compile_cmd[:-1])
        if token == "-I"
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
    link_cmd = next(cmd for cmd in commands if "-shared" in cmd)
    assert any("0_demoext.o" in part for part in link_cmd)
    assert any("1_helper_generated.o" in part for part in link_cmd)
    manifest = json.loads((out_dir / "extension_manifest.json").read_text())
    assert manifest["source_plan"]["kind"] == "meson-intro-targets"
    assert manifest["source_plan"]["plan"] == str(intro_path.resolve())
    assert manifest["source_plan"]["compile_commands"] == str(
        (project_root / "build" / "compile_commands.json").resolve()
    )
    assert manifest["source_plan"]["digest"]
    assert manifest["build"]["source_plan_digest"] == manifest["source_plan"]["digest"]
    assert manifest["build"]["object_count"] == 2
    assert manifest["build"]["linked_object_count"] == 2
    assert manifest["build"]["source_c_api_scan"][
        "project_generated_c_api_prefixes"
    ] == ["npy_generated_"]
    assert manifest["build"]["source_c_api_scan"][
        "project_generated_c_api_symbols"
    ] == ["npy_generated_int8"]
    assert manifest["object_closure"]["root_symbol"] == "PyInit_demoext"
    assert manifest["object_closure"]["init_symbol_owner"] == "0_demoext.o"
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
    assert "npy_generated_int8" in project_generated_c_api_symbols
    assert "npy_generated_int8" not in required_c_api_symbols
    assert "NPY_HEADER_ONLY_MACRO" not in required_c_api_symbols
    assert "NPY_DISABLED_Alias" not in required_c_api_symbols
    assert "NPY_DISABLED_SVE" not in required_c_api_symbols
    assert "PyCode_NewWithPosOnlyArgs" not in required_c_api_symbols
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
            "--abi-tier",
            "cpython-abi",
            "--json",
        ]
    )
    assert args.command == "extension"
    assert args.extension_command == "metadata"
    assert args.target == "wasm32-wasip1"
    assert args.out_dir == "dist/meta"
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
    monkeypatch.setattr(
        cli_source_extension_toolchain,
        "_resolve_source_extension_wasm_toolchain",
        lambda: cli_source_extension_toolchain._SourceExtensionWasmToolchain(
            ok=True,
            compiler_kind="zig",
            compiler_cmd=("zig", "cc"),
            wasm_ld="/usr/bin/wasm-ld",
            wasi_sysroot=None,
            detail="wasm-ld=/usr/bin/wasm-ld; zig=/usr/bin/zig",
        ),
    )
    monkeypatch.setattr(
        cli_source_extension_toolchain.shutil,
        "which",
        lambda tool: "/usr/bin/pkg-config" if tool == "pkg-config" else None,
    )

    out_dir = tmp_path / "metadata"
    rc = cli_commands.extension_metadata(
        target="wasm",
        out_dir=str(out_dir),
        abi_tier="cpython-abi",
        json_output=True,
        verbose=False,
    )

    assert rc == 0
    payload = json.loads(capsys.readouterr().out)
    assert payload["data"]["target_triple"] == "wasm32-wasip1"
    assert payload["data"]["abi"]["tier"] == "cpython-abi"
    assert payload["data"]["paths"]["python_pc"] == str(
        out_dir / "pkgconfig" / "python3.pc"
    )
    assert (out_dir / "pkgconfig" / "python3.pc").read_text(encoding="utf-8")
    assert "wasm32" in (out_dir / "meson.cross").read_text(encoding="utf-8")
    assert (out_dir / "source-extension-target-metadata.json").is_file()


def test_source_extension_toolchain_rejects_wasm_cc_without_wasi_headers(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MOLT_WASM_CC", "clang")
    monkeypatch.delenv("MOLT_CROSS_CC", raising=False)
    monkeypatch.setattr(
        cli_source_extension_toolchain.shutil,
        "which",
        lambda tool: {
            "clang": "/tools/clang",
            "wasm-ld": "/tools/wasm-ld",
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

    toolchain = (
        cli_source_extension_toolchain._resolve_source_extension_wasm_toolchain()
    )

    assert toolchain.ok is False
    assert toolchain.compiler_kind == "molt_wasm_cc"
    assert "MOLT_WASM_CC cannot compile the WASI source-extension probe" in (
        toolchain.detail
    )
    assert "errno.h" in toolchain.detail
    assert "WASI_SYSROOT" in toolchain.detail


def test_source_extension_toolchain_prefers_wasm_cc_and_probes_target(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    seen_commands: list[list[str]] = []
    monkeypatch.setenv("MOLT_WASM_CC", "clang-wasm")
    monkeypatch.setenv("MOLT_CROSS_CC", "wrong-cross")
    monkeypatch.setattr(
        cli_source_extension_toolchain.shutil,
        "which",
        lambda tool: {
            "clang-wasm": "/tools/clang-wasm",
            "wrong-cross": "/tools/wrong-cross",
            "wasm-ld": "/tools/wasm-ld",
        }.get(tool),
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

    toolchain = (
        cli_source_extension_toolchain._resolve_source_extension_wasm_toolchain()
    )

    assert toolchain.ok is True
    assert toolchain.compiler_kind == "molt_wasm_cc"
    assert toolchain.compiler_cmd == ("/tools/clang-wasm",)
    assert "MOLT_WASM_CC=/tools/clang-wasm" in toolchain.detail
    assert "wrong-cross" not in toolchain.detail
    assert seen_commands
    assert seen_commands[0][:3] == ["/tools/clang-wasm", "-target", "wasm32-wasip1"]


def test_source_extension_toolchain_accepts_target_specific_wasi_sysroot_layout(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    sysroot = tmp_path / "wasi-sysroot-33.0+m"
    include_dir = sysroot / "include" / "wasm32-wasip1"
    include_dir.mkdir(parents=True)
    (include_dir / "errno.h").write_text("#define EINVAL 28\n")
    cli_wasm_toolchain._resolve_wasi_sysroot_cached.cache_clear()
    monkeypatch.setenv("WASI_SYSROOT", str(sysroot))
    monkeypatch.delenv("MOLT_WASM_CC", raising=False)
    monkeypatch.delenv("MOLT_CROSS_CC", raising=False)
    monkeypatch.setattr(
        cli_source_extension_toolchain.shutil,
        "which",
        lambda tool: {
            "clang": "/tools/clang",
            "wasm-ld": "/tools/wasm-ld",
        }.get(tool),
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

    toolchain = (
        cli_source_extension_toolchain._resolve_source_extension_wasm_toolchain()
    )

    assert toolchain.ok is True
    assert toolchain.compiler_kind == "clang"
    assert toolchain.compiler_cmd == (
        "/tools/clang",
        "--sysroot",
        str(sysroot.resolve(strict=False)),
    )
    assert toolchain.wasi_sysroot == sysroot.resolve(strict=False)
    assert seen_commands
    assert seen_commands[0][:4] == [
        "/tools/clang",
        "--sysroot",
        str(sysroot.resolve(strict=False)),
        "-target",
    ]


def test_extension_build_wasm_target_emits_static_link_artifact_and_manifest(
    tmp_path: Path,
    monkeypatch,
    capsys,
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
            'effects = ["read"]',
            "deterministic = true",
        ],
    )
    wasm_imports = (
        "molt_alloc",
        "molt_cpython_abi_date_from_date",
        "PyOS_strtol",
        "malloc",
    )
    wasm_bytes = _wasm_exporting_i64_unary_symbol(
        native_symbol,
        imports=wasm_imports,
    )
    commands: list[list[str]] = []

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        commands.append(cmd)
        out_index = cmd.index("-o")
        out_path = Path(cmd[out_index + 1])
        out_path.parent.mkdir(parents=True, exist_ok=True)
        out_path.write_bytes(wasm_bytes)
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli_commands, "_run_completed_command", fake_run)
    monkeypatch.setattr(
        cli_commands, "_ensure_rustup_target", lambda _target, _warnings: True
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
        json_output=True,
        verbose=False,
    )
    assert rc == 0
    payload = json.loads(capsys.readouterr().out)
    assert payload["data"]["target_triple"] == "wasm32-wasip1"
    assert payload["data"]["runtime_linkage"] == "static_link"
    assert payload["data"]["artifact_kind"] == "wasm_relocatable_object"
    assert any("--target=wasm32-wasip1" in cmd for cmd in commands)
    assert any(f"--sysroot={wasi_sysroot}" in cmd for cmd in commands)
    assert any("-wasm-enable-sjlj" in cmd for cmd in commands)

    artifact_path = out_dir / "demoext.molt.wasm"
    assert artifact_path.exists()
    assert artifact_path.read_bytes() == wasm_bytes
    assert [
        export.name
        for export in wasm_artifact.read_wasm_function_exports(artifact_path)
    ] == [native_symbol]

    manifest = json.loads((out_dir / "extension_manifest.json").read_text())
    assert manifest["target_triple"] == "wasm32-wasip1"
    assert manifest["runtime_linkage"] == "static_link"
    assert manifest["artifact_kind"] == "wasm_relocatable_object"
    assert manifest["build"]["wasi_sysroot"] == str(wasi_sysroot)
    assert manifest["extension"] == "demoext.molt.wasm"
    assert manifest["extension_sha256"] == hashlib.sha256(wasm_bytes).hexdigest()
    object_closure = manifest["object_closure"]
    assert object_closure["defined_symbols"] == [native_symbol]
    assert object_closure["undefined_symbols"] == sorted(wasm_imports)
    assert object_closure["runtime_symbols"] == sorted(wasm_imports)
    assert "PyModule_Create" in object_closure["required_c_api_symbols"]
    assert "PyOS_strtol" in object_closure["required_c_api_symbols"]
    assert "PyInit_demoext" not in object_closure["required_c_api_symbols"]
    assert "PyMODINIT_FUNC" not in object_closure["required_c_api_symbols"]
    assert "PyTypeObject" not in object_closure["required_c_api_symbols"]
    assert "Python" not in object_closure["required_c_api_symbols"]
    assert manifest["callable_exports"] == [
        {
            "module": "demoext.ndimage",
            "name": "distance_transform_edt",
            "binding": "direct_symbol",
            "abi": "molt.object_call_v1",
            "symbol": native_symbol,
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
    assert embedded["object_closure"] == manifest["object_closure"]


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
    monkeypatch.setattr(
        cli_commands, "_ensure_rustup_target", lambda _target, _warnings: True
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
    assert "WASM source-recompiled extension builds for 'numpy'" in stderr
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
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli_commands, "_run_completed_command", fake_run)
    monkeypatch.setattr(
        cli_commands, "_ensure_rustup_target", lambda _target, _warnings: True
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
    monkeypatch.setattr(
        cli_commands, "_ensure_rustup_target", lambda _target, _warnings: True
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
        cli_commands, "_ensure_rustup_target", lambda _target, _warnings: True
    )
    monkeypatch.setattr(
        cli_commands,
        "resolve_wasi_sysroot",
        lambda: None,
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

    assert cli_wasm_toolchain.normalize_wasi_sysroot(sysroot) == sysroot.resolve(
        strict=False
    )
    assert cli_wasm_toolchain.normalize_wasi_sysroot(include_dir) == sysroot.resolve(
        strict=False
    )


@pytest.mark.parametrize(
    "target",
    [None, "aarch64-unknown-linux-gnu"],
    ids=["native", "cross-aarch64-gnu"],
)
def test_extension_numpy_build_audit_publish_dry_run_matrix(
    tmp_path: Path,
    monkeypatch,
    target: str | None,
) -> None:
    project_root = tmp_path / "numpy_extproj"
    project_root.mkdir()
    _write_extension_numpy_project(project_root)

    def fake_run(cmd: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        out_index = cmd.index("-o")
        out_path = Path(cmd[out_index + 1])
        out_path.parent.mkdir(parents=True, exist_ok=True)
        if "-c" in cmd:
            out_path.write_bytes(b"obj")
        else:
            out_path.write_bytes(b"shared")
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli_commands, "_run_completed_command", fake_run)
    monkeypatch.setattr(
        cli_commands,
        "_shared_library_defines_symbol",
        lambda _path, symbol: (symbol == "PyInit_demoext_numpy", None),
    )

    if target is not None:
        monkeypatch.setattr(
            cli_commands, "_ensure_rustup_target", lambda _target, _warnings: True
        )
        monkeypatch.setattr(
            cli_commands.shutil,
            "which",
            lambda tool: "/usr/bin/zig" if tool == "zig" else None,
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

    wheel_path = next(out_dir.glob("*.whl"))
    audit_rc = cli.extension_audit(
        path=str(wheel_path),
        require_capabilities=True,
        require_abi="1",
        require_checksum=True,
        json_output=False,
        verbose=False,
    )
    assert audit_rc == 0

    publish_rc = cli.publish(
        package_path=str(wheel_path),
        registry=str(out_dir / "registry"),
        dry_run=True,
        json_output=False,
        verbose=False,
        deterministic=False,
        capabilities="fs.read",
    )
    assert publish_rc == 0


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
        "target_triple": "wasm32-wasip1",
        "platform_tag": "wasm32_wasip1",
        "loader_kind": "libmolt_source",
        "init_symbol": "PyInit__multiarray_umath",
        "runtime_linkage": "static_link",
        "artifact_kind": "wasm_relocatable_object",
        "capabilities": ["module.extension.exec"],
        "extension": "numpy/_core/_multiarray_umath.molt.wasm",
        "extension_sha256": extension_sha256,
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
            "schema_version": 1,
            "root_symbol": "PyInit__nd_image",
            "init_symbol_owner": "0_nd_image.o",
            "closure_sha256": extension_sha256,
            "runtime_symbols": [],
            "required_capsules": [],
            "objects": [
                {
                    "source": str(source_path),
                    "object": "0_nd_image.o",
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
        assert sealed_manifest["object_closure"]["objects"][0]["required_capsules"] == [
            capsule
        ]


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
            "schema_version": 1,
            "root_symbol": "PyInit__nd_image",
            "init_symbol_owner": "0_nd_image.o",
            "closure_sha256": extension_sha256,
            "runtime_symbols": [],
            "required_capsules": [],
            "objects": [
                {
                    "object": "0_nd_image.o",
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
        "def distance_transform_edt(mask):\n"
        "    return _nd_image.euclidean_feature_transform(mask)\n",
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
        "target_triple": "wasm32-wasip1",
        "platform_tag": "wasm32_wasip1",
        "loader_kind": "libmolt_source",
        "init_symbol": "PyInit__nd_image",
        "runtime_linkage": "static_link",
        "artifact_kind": "wasm_relocatable_object",
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
            "schema_version": 1,
            "root_symbol": "PyInit__nd_image",
            "init_symbol_owner": "0_nd_image.o",
            "closure_sha256": extension_sha256,
            "runtime_symbols": [],
            "required_capsules": [],
            "objects": [
                {
                    "object": "0_nd_image.o",
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
        support_file=[str(provider_source)],
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
        str(sealed_root / "scipy" / "ndimage" / "_morphology.py")
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
        }
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
        required_modules={"scipy.ndimage.distance_transform_edt"},
    )
    assert errors == []
    assert plan is not None
    assert plan.support_source_module_names() == frozenset(
        {"scipy.ndimage._morphology"}
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
            str(provider_source),
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
    ]


def test_extension_audit_requires_static_link_artifact_custody(
    tmp_path: Path,
    capsys,
) -> None:
    out_dir = tmp_path / "dist"
    artifact_dir = out_dir / "nativepkg"
    artifact_dir.mkdir(parents=True)
    artifact_bytes = b"\0asm-static-link-probe"
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
        "target_triple": "wasm32-wasip1",
        "platform_tag": "wasm32_wasip1",
        "loader_kind": "libmolt_source",
        "runtime_linkage": "static_link",
        "artifact_kind": "wasm_relocatable_object",
        "capabilities": ["ffi.unsafe"],
        "init_symbol": "PyInit__native",
        "extension": "nativepkg/_native.molt.wasm",
        "extension_sha256": hashlib.sha256(artifact_bytes).hexdigest(),
        "object_closure": {
            "schema_version": 1,
            "root_symbol": "PyInit__native",
            "init_symbol_owner": "0_native.o",
            "closure_sha256": "a" * 64,
            "project_generated_c_api_prefixes": ["npy_generated_"],
            "objects": [
                {
                    "path": "0_native.o",
                    "defined_symbols": ["PyInit__native"],
                    "undefined_symbols": ["molt_add"],
                    "required_c_api_symbols": ["PyLong_FromLong"],
                    "required_capsules": [
                        "numpy.core._multiarray_umath._ARRAY_API",
                    ],
                    "project_generated_c_api_symbols": ["npy_generated_int8"],
                }
            ],
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
        require_checksum=True,
        json_output=True,
        verbose=False,
    )

    assert rc == 0
    payload = json.loads(capsys.readouterr().out)
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
                "static int memoryview_descriptor(PyObject *obj, char *data) {",
                "    Py_buffer view;",
                "    PyObject *from_object;",
                "    PyObject *from_memory;",
                "    PyObject *from_buffer;",
                "    Py_buffer *exported;",
                "    PyObject *base;",
                "    if (PyBuffer_FillInfo(&view, obj, data, 4, 0, PyBUF_FORMAT | PyBUF_STRIDES) != 0) {",
                "        return -1;",
                "    }",
                "    from_object = PyMemoryView_FromObject(obj);",
                "    from_memory = PyMemoryView_FromMemory(data, 4, PyBUF_WRITABLE);",
                "    from_buffer = PyMemoryView_FromBuffer(&view);",
                "    exported = PyMemoryView_GET_BUFFER(from_buffer);",
                "    base = PyMemoryView_GET_BASE(from_buffer);",
                "    (void)PyMemoryView_Check(from_object);",
                "    (void)from_memory;",
                "    (void)exported;",
                "    (void)base;",
                "    PyBuffer_Release(&view);",
                "    return 0;",
                "}",
                "",
                "int main(void) {",
                "    char data[4] = {0, 1, 2, 3};",
                "    (void)getbuffer_descriptor;",
                "    (void)memoryview_descriptor;",
                "    (void)PyObject_CheckBuffer(NULL);",
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


def test_numpy_header_arrayobject_smoke(tmp_path: Path) -> None:
    clang = shutil.which("clang")
    if clang is None:
        pytest.skip("clang is required for NumPy compatibility header smoke test")
    source = tmp_path / "numpy_h_arrayobject_smoke.c"
    source.write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "#include <numpy/arrayobject.h>",
                "",
                "static int numpy_smoke(PyObject *obj) {",
                "    PyArrayObject *arr = (PyArrayObject *)obj;",
                "    PyArray_Descr *descr = PyArray_DescrFromType(NPY_INT);",
                "    PyArray_Descr *scalar_descr = PyArray_DescrFromScalar(obj);",
                "    npy_intp nd = PyArray_NDIM(arr);",
                "    npy_intp size = PyArray_SIZE(arr);",
                "    int is_int = PyTypeNum_ISINTEGER(PyArray_TYPE(arr));",
                "    int is_scalar = PyArray_CheckScalar(obj);",
                "    int is_datetime = PyArray_ISDATETIME(arr);",
                "    PyObject *from_any = PyArray_FromAny(obj, PyArray_DescrFromType(NPY_UBYTE), 1, 2, NPY_ARRAY_C_CONTIGUOUS, NULL);",
                "    import_array1(-1);",
                "    (void)from_any;",
                "    if (descr != NULL) {",
                "        PyMem_Free(descr);",
                "    }",
                "    if (scalar_descr != NULL) {",
                "        PyMem_Free(scalar_descr);",
                "    }",
                "    return (int)(nd + size + is_int + is_scalar + is_datetime);",
                "}",
                "",
                "int main(void) {",
                "    (void)numpy_smoke;",
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


def test_numpy_header_arrayobject_cpython_abi_tier_smoke(tmp_path: Path) -> None:
    clang = shutil.which("clang")
    if clang is None:
        pytest.skip("clang is required for NumPy CPython ABI header smoke test")
    source = tmp_path / "numpy_h_arrayobject_cpython_abi_smoke.c"
    source.write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "#include <numpy/arrayobject.h>",
                "",
                "static int numpy_cpython_abi_smoke(PyObject *obj) {",
                "    PyArrayObject *arr = (PyArrayObject *)obj;",
                "    PyArray_Descr *descr = PyArray_DescrFromScalar(obj);",
                "    PyObject *from_any = PyArray_FromAny(",
                "        obj, PyArray_DescrFromType(NPY_DOUBLE), 1, 2,",
                "        NPY_ARRAY_C_CONTIGUOUS, NULL);",
                "    int ok = PyArray_Check(arr) + PyArray_DescrCheck(descr);",
                "    PyArray_DTypeMeta *dtype_meta = (PyArray_DTypeMeta *)descr;",
                "    int dtype_ok = PyObject_TypeCheck(dtype_meta, &PyArrayDTypeMeta_Type);",
                "    int type_ok = PyType_Check(&PyArray_Type);",
                "    PyObject *type_ref = Py_NewRef(&PyArray_Type);",
                "    Py_XSETREF(type_ref, Py_NewRef(Py_TYPE(obj)));",
                "    if (from_any != NULL) {",
                "        Py_DECREF(from_any);",
                "    }",
                "    if (type_ref != NULL) {",
                "        Py_DECREF(type_ref);",
                "    }",
                "    if (descr != NULL) {",
                "        PyMem_Free(descr);",
                "    }",
                "    return ok + dtype_ok + type_ok + (int)PyArray_SIZE(arr);",
                "}",
                "",
                "int main(void) {",
                "    (void)numpy_cpython_abi_smoke;",
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
            f"-I{ROOT / 'include'}",
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
            f"-I{ROOT / 'include'}",
            "-fsyntax-only",
            str(source),
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
