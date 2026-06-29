from __future__ import annotations

import hashlib
import json
import shutil
import subprocess
import zipfile
from pathlib import Path

import molt.cli as cli
from molt.cli import commands as cli_commands
import pytest

from tests.cli.process_guard import run_cli_test_process


ROOT = Path(__file__).resolve().parents[2]


def _write_extension_project(project_root: Path) -> None:
    src_dir = project_root / "src"
    src_dir.mkdir(parents=True, exist_ok=True)
    (src_dir / "demoext.c").write_text(
        "#include <Python.h>\n"
        "#include <molt/molt.h>\n"
        "int demoext_version(void) { return (int)molt_c_api_version(); }\n"
        "static PyModuleDef demoext_module = {\n"
        "    PyModuleDef_HEAD_INIT,\n"
        "    \"demoext\",\n"
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
                "",
            ]
        )
    )


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
                "",
                "static int numpy_probe(PyObject *obj) {",
                "    PyArrayObject *arr = (PyArrayObject *)obj;",
                "    PyArray_Descr *descr = PyArray_DescrFromType(NPY_INT);",
                "    PyArray_Descr *scalar_descr = PyArray_DescrFromScalar(obj);",
                "    npy_intp ndim = PyArray_NDIM(arr);",
                "    npy_intp size = PyArray_SIZE(arr);",
                "    int is_int = PyTypeNum_ISINTEGER(PyArray_TYPE(arr));",
                "    int scalar_check = PyArray_CheckScalar(obj);",
                "    int is_datetime = PyArray_ISDATETIME(arr);",
                "    (void)PyArray_CastScalarToCtype;",
                "    if (descr != NULL) {",
                "        PyMem_Free(descr);",
                "    }",
                "    if (scalar_descr != NULL) {",
                "        PyMem_Free(scalar_descr);",
                "    }",
                "    return (int)(ndim + size + is_int + scalar_check + is_datetime);",
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
    ]:
        assert symbol in data["project_defined_symbols"]
        assert data["symbol_status"][symbol] == "project_defined"
    assert "PyParam" not in data["required_symbols"]
    assert "PyLocalTemp" not in data["required_symbols"]
    assert "PyLocal_Type" not in data["project_defined_symbols"]
    assert "PyLong_FromLong" in data["required_symbols"]
    assert data["symbol_status"]["PyLong_FromLong"] == "runtime_backed"
    assert "PyCythonCommentOnly" not in data["required_symbols"]
    assert "PyTrailingCommentOnly" not in data["required_symbols"]
    assert "PyTripleDocOnly" not in data["required_symbols"]


def test_extension_scan_preserves_guarded_body_symbols(
    tmp_path: Path, capsys
) -> None:
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
                "#define PyLocalMacro(value) \\",
                "    (PyMacroMissingAPI((value)))",
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
    assert "PyArray_SIZE" in data["source_compile_only_symbols"]
    assert "PyArray_TYPE" in data["source_compile_only_symbols"]
    assert "PyTypeNum_ISINTEGER" in data["source_compile_only_symbols"]
    assert "PyArray_CheckScalar" in data["source_compile_only_symbols"]
    assert "PyArray_ISDATETIME" in data["source_compile_only_symbols"]
    assert "PyArray_DescrFromScalar" in data["source_compile_only_symbols"]
    assert "PyArray_DescrFromType" in data["source_compile_only_symbols"]


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

    with zipfile.ZipFile(wheel_path) as zf:
        names = set(zf.namelist())
        assert "extension_manifest.json" in names
        assert manifest["extension"] in names


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

    monkeypatch.setattr(cli_commands, "_ensure_rustup_target", lambda _target, _warnings: True)
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


def test_extension_build_rejects_wasm_target(tmp_path: Path) -> None:
    project_root = tmp_path / "extproj"
    project_root.mkdir()
    _write_extension_project(project_root)
    rc = cli_commands.extension_build(
        project=str(project_root),
        out_dir=str(project_root / "dist"),
        target="wasm",
        json_output=False,
        verbose=False,
    )
    assert rc != 0


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
