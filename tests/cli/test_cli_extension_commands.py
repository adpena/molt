from __future__ import annotations

import hashlib
import json
import shutil
import subprocess
import tarfile
import zipfile
from pathlib import Path

import molt.cli as cli
import pytest


ROOT = Path(__file__).resolve().parents[2]


def _write_extension_project(project_root: Path) -> None:
    src_dir = project_root / "src"
    src_dir.mkdir(parents=True, exist_ok=True)
    (src_dir / "demoext.c").write_text(
        "#include <molt/molt.h>\n"
        "int demoext_version(void) { return (int)molt_c_api_version(); }\n"
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
                "    (void)PyOS_string_to_double;",
                "    (void)PyObject_Vectorcall;",
                "    (void)PyObject_CallFinalizerFromDealloc;",
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


def _write_extension_scan_directory_project(project_root: Path) -> None:
    src_dir = project_root / "src"
    src_dir.mkdir(parents=True, exist_ok=True)
    (src_dir / "alpha.c").write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "int alpha(void) {",
                "    (void)PyLong_FromLong;",
                "    (void)PyObject_CallFinalizerFromDealloc;",
                "    return 0;",
                "}",
                "",
            ]
        )
    )
    (src_dir / "beta.c").write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "int beta(void) {",
                "    (void)PyObject_CallFinalizerFromDealloc;",
                "    return 0;",
                "}",
                "",
            ]
        )
    )
    (project_root / "pyproject.toml").write_text(
        "\n".join(
            [
                "[project]",
                'name = "scan-dir-ext"',
                'version = "0.1.0"',
                "",
                "[tool.molt.extension]",
                'module = "demoext"',
                'sources = ["src"]',
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


def _write_extension_numpy_batch_project(project_root: Path) -> None:
    src_dir = project_root / "src"
    src_dir.mkdir(parents=True, exist_ok=True)
    (src_dir / "demoext.c").write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "#include <numpy/arrayobject.h>",
                "#include <numpy/npy_common.h>",
                "#include <numpy/npy_math.h>",
                "#include <numpy/dtype_api.h>",
                "#include <numpy/__multiarray_api.h>",
                "#include <numpy/__ufunc_api.h>",
                "",
                "static int numpy_batch_probe(PyObject *obj) {",
                "    PyArrayObject arr_storage = {0};",
                "    PyArray_Descr descr_storage = {0};",
                "    npy_intp dims[2] = {2, 3};",
                "    npy_intp tuple_vals[2] = {2, 3};",
                "    char data[6] = {0};",
                "    npy_bool bool_flag = 0;",
                "    NPY_ORDER order = NPY_CORDER;",
                "    NPY_SORTKIND sortkind = NPY_QUICKSORT;",
                "    NPY_SEARCHSIDE side = NPY_SEARCHLEFT;",
                "    NPY_CLIPMODE clipmode = NPY_CLIP;",
                "    char byteorder = '=';",
                "    PyObject *capsule = PyCapsule_New((void *)data, \"demo.capsule\", NULL);",
                "    PyObject *tuple_obj = NULL;",
                "    PyObject *handler = NULL;",
                "    PyObject *descr_copy = NULL;",
                "    PyObject *ufunc_obj = NULL;",
                "    PyThreadState *ts = NULL;",
                "    PyUFuncGenericFunction ufunc_loop = PyUFunc_O_O;",
                "    descr_storage.type_num = NPY_INT;",
                "    descr_storage.elsize = 1;",
                "    descr_storage.byteorder = '=';",
                "    arr_storage.data = data;",
                "    arr_storage.nd = 2;",
                "    arr_storage.dimensions = dims;",
                "    arr_storage.strides = dims;",
                "    arr_storage.descr = &descr_storage;",
                "    arr_storage.flags = NPY_ARRAY_CARRAY;",
                "    (void)PyArray_CheckFromAny(obj, NULL, 0, 0, NPY_ARRAY_DEFAULT, NULL);",
                "    (void)PyArray_EnsureArray(obj);",
                "    (void)PyArray_EnsureAnyArray(obj);",
                "    descr_copy = (PyObject *)PyArray_DescrNew(&descr_storage);",
                "    (void)PyArray_DescrNewByteorder(&descr_storage, '<');",
                "    (void)PyArray_DescrFromTypeObject((PyObject *)&PyLong_Type);",
                "    (void)PyArray_DescrFromObject(obj, &descr_storage);",
                "    (void)PyArray_ObjectType(obj, NPY_NOTYPE);",
                "    (void)PyArray_MultiplyList(tuple_vals, 2);",
                "    (void)PyArray_PyIntAsInt(PyLong_FromLong(3));",
                "    (void)PyArray_PyIntAsIntp(PyLong_FromLong(4));",
                "    tuple_obj = PyArray_IntTupleFromIntp(2, tuple_vals);",
                "    (void)PyArray_IntpFromSequence(tuple_obj, tuple_vals, 2);",
                "    (void)PyArray_BoolConverter(Py_True, &bool_flag);",
                "    (void)PyArray_OrderConverter(PyLong_FromLong(NPY_CORDER), &order);",
                "    (void)PyArray_SortkindConverter(PyLong_FromLong(NPY_QUICKSORT), &sortkind);",
                "    (void)PyArray_SearchsideConverter(PyLong_FromLong(NPY_SEARCHLEFT), &side);",
                "    (void)PyArray_ClipmodeConverter(PyLong_FromLong(NPY_CLIP), &clipmode);",
                "    (void)PyArray_ByteorderConverter(PyUnicode_FromString(\"<\"), &byteorder);",
                "    PyArray_ENABLEFLAGS(&arr_storage, NPY_ARRAY_WRITEABLE);",
                "    (void)PyArray_CHKFLAGS(&arr_storage, NPY_ARRAY_WRITEABLE);",
                "    PyArray_CLEARFLAGS(&arr_storage, NPY_ARRAY_WRITEBACKIFCOPY);",
                "    PyArray_FILLWBYTE(&arr_storage, 0);",
                "    (void)PyArray_BASE(&arr_storage);",
                "    (void)PyArray_DTYPE(&arr_storage);",
                "    (void)PyArray_Size(&arr_storage);",
                "    (void)PyArray_CopyInto(&arr_storage, &arr_storage);",
                "    (void)PyArray_CopyAnyInto(&arr_storage, &arr_storage);",
                "    (void)PyArray_SetBaseObject(&arr_storage, obj);",
                "    (void)PyArray_Return(&arr_storage);",
                "    (void)PyArray_FromInterface(obj);",
                "    (void)PyArray_FromStructInterface(obj);",
                "    (void)PyDataType_ELSIZE(&descr_storage);",
                "    (void)PyDataType_FLAGS(&descr_storage);",
                "    (void)PyDataType_ISINTEGER(&descr_storage);",
                "    handler = PyDataMem_GetHandler();",
                "    (void)PyDataMem_SetHandler(handler);",
                "    (void)PyCapsule_SetContext(capsule, obj);",
                "    (void)PyCapsule_GetContext(capsule);",
                "    ts = PyEval_SaveThread();",
                "    PyEval_RestoreThread(ts);",
                "    (void)PyExc_ModuleNotFoundError;",
                "    (void)PyExc_IOError;",
                "    (void)PyErr_Print;",
                "    (void)PyUFunc_API;",
                "    (void)PyUFunc_ImportUFuncAPI();",
                "    ufunc_obj = PyUFunc_FromFuncAndData(NULL, NULL, NULL, 0, 1, 1, PyUFunc_None, \"demo\", NULL, 0);",
                "    (void)PyUFunc_RegisterLoopForType(ufunc_obj, NPY_INT, NULL, NULL, NULL);",
                "    (void)PyArrayMethod_GetLoop(NULL, NULL, 0, 0, NULL, NULL, NULL);",
                "    (void)PyArrayMethod_ResolveDescriptors(NULL, NULL, NULL, NULL, NULL);",
                "    (void)ufunc_loop;",
                "    Py_XDECREF(handler);",
                "    Py_XDECREF(tuple_obj);",
                "    Py_XDECREF(descr_copy);",
                "    Py_XDECREF(capsule);",
                "    Py_XDECREF(ufunc_obj);",
                "    return 0;",
                "}",
                "",
                "int demoext_numpy_batch_ready(void) {",
                "    import_array1(-1);",
                "    return 0;",
                "}",
                "",
                "int demoext_numpy_batch_touch(PyObject *obj) {",
                "    return numpy_batch_probe(obj);",
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
                'name = "demo-numpy-batch-ext"',
                'version = "0.1.0"',
                "",
                "[tool.molt.extension]",
                'module = "demoext_numpy_batch"',
                'sources = ["src/demoext.c"]',
                'capabilities = ["fs.read"]',
                'molt_c_api_version = "1"',
                "",
            ]
        )
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
    assert "PyOS_string_to_double" in data["supported_symbols"]
    assert "PyObject_Vectorcall" in data["supported_symbols"]
    assert "PyObject_CallFinalizerFromDealloc" in data["missing_symbols"]
    assert "PyLong_FromLong" in data["supported_symbols"]


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
    assert "PyObject_CallFinalizerFromDealloc" in payload["data"]["missing_symbols"]


def test_extension_scan_supports_directory_sources(tmp_path: Path, capsys) -> None:
    project_root = tmp_path / "scanproj_dir"
    project_root.mkdir()
    _write_extension_scan_directory_project(project_root)

    rc = cli.extension_scan(
        project=str(project_root),
        fail_on_missing=True,
        json_output=True,
        verbose=False,
    )
    assert rc == 1
    payload = json.loads(capsys.readouterr().out)
    assert payload["status"] == "error"
    data = payload["data"]
    assert data["source_count"] == 2
    assert data["missing_symbol_frequency"]["PyObject_CallFinalizerFromDealloc"] == 2
    assert data["top_missing_symbols"][0] == {
        "symbol": "PyObject_CallFinalizerFromDealloc",
        "file_count": 2,
    }
    assert data["coverage_ratio"] < 1.0


def test_extension_scan_supports_tar_archive_sources(
    tmp_path: Path, capsys
) -> None:
    archive_path = tmp_path / "demoext.tar.gz"
    archive_source = tmp_path / "demoext.c"
    archive_source.write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "int demo(void) {",
                "    (void)PyLong_FromLong;",
                "    (void)PyObject_CallFinalizerFromDealloc;",
                "    return 0;",
                "}",
                "",
            ]
        )
    )
    with tarfile.open(archive_path, "w:gz") as tf:
        tf.add(archive_source, arcname="pkg/demoext.c")

    rc = cli.extension_scan(
        project=str(tmp_path),
        sources=[str(archive_path)],
        fail_on_missing=True,
        json_output=True,
        verbose=False,
    )
    assert rc == 1
    payload = json.loads(capsys.readouterr().out)
    data = payload["data"]
    archive_label = f"{archive_path}!pkg/demoext.c"
    assert data["source_count"] == 1
    assert "PyLong_FromLong" in data["supported_symbols"]
    assert "PyObject_CallFinalizerFromDealloc" in data["missing_symbols"]
    assert data["missing_symbol_frequency"]["PyObject_CallFinalizerFromDealloc"] == 1
    assert archive_label in data["required_by_file"]


def test_extension_scan_supports_zip_archive_sources(
    tmp_path: Path, capsys
) -> None:
    archive_path = tmp_path / "demoext.zip"
    source_text = "\n".join(
        [
            "#include <Python.h>",
            "int demo(void) {",
            "    (void)PyLong_FromLong;",
            "    return 0;",
            "}",
            "",
        ]
    )
    with zipfile.ZipFile(archive_path, "w") as zf:
        zf.writestr("pkg/demoext.c", source_text)

    rc = cli.extension_scan(
        project=str(tmp_path),
        sources=[str(archive_path)],
        fail_on_missing=True,
        json_output=True,
        verbose=False,
    )
    assert rc == 0
    payload = json.loads(capsys.readouterr().out)
    data = payload["data"]
    archive_label = f"{archive_path}!pkg/demoext.c"
    assert data["source_count"] == 1
    assert data["missing_symbols"] == []
    assert archive_label in data["required_by_file"]
    assert data["coverage_ratio"] == 1.0


def test_extension_scan_ignores_locally_defined_py_symbols(
    tmp_path: Path, capsys
) -> None:
    project_root = tmp_path / "scanproj_local_defs"
    src_dir = project_root / "src"
    src_dir.mkdir(parents=True)
    source_path = src_dir / "demoext.c"
    source_path.write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "#define PyLocalMacro(x) (x)",
                "static int PyLocalHelper(PyObject *obj) {",
                "    (void)obj;",
                "    return 0;",
                "}",
                "int demo(void) {",
                "    (void)PyLocalMacro;",
                "    (void)PyLocalHelper;",
                "    (void)PyObject_CallFinalizerFromDealloc;",
                "    return 0;",
                "}",
                "",
            ]
        )
    )
    (project_root / "pyproject.toml").write_text(
        "\n".join(
            [
                "[project]",
                'name = "scan-local-defs"',
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

    rc = cli.extension_scan(
        project=str(project_root),
        fail_on_missing=True,
        json_output=True,
        verbose=False,
    )
    assert rc == 1
    payload = json.loads(capsys.readouterr().out)
    data = payload["data"]
    assert "PyObject_CallFinalizerFromDealloc" in data["missing_symbols"]
    assert "PyLocalMacro" not in data["missing_symbols"]
    assert "PyLocalHelper" not in data["missing_symbols"]
    local_defs = data["locally_defined_by_file"][str(source_path)]
    assert "PyLocalMacro" in local_defs
    assert "PyLocalHelper" in local_defs


def test_extension_scan_numpy_surface_symbols_supported(tmp_path: Path, capsys) -> None:
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
    assert "PyArray_DescrFromType" in data["supported_symbols"]
    assert "PyArray_NDIM" in data["supported_symbols"]
    assert "PyArray_SIZE" in data["supported_symbols"]
    assert "PyArray_TYPE" in data["supported_symbols"]
    assert "PyTypeNum_ISINTEGER" in data["supported_symbols"]
    assert "PyArray_CheckScalar" in data["supported_symbols"]
    assert "PyArray_ISDATETIME" in data["supported_symbols"]
    assert "PyArray_DescrFromScalar" in data["supported_symbols"]
    assert "PyArray_CastScalarToCtype" in data["supported_symbols"]


def test_extension_scan_numpy_surface_batch_symbols_supported(
    tmp_path: Path, capsys
) -> None:
    project_root = tmp_path / "numpy_scanproj_batch"
    project_root.mkdir()
    _write_extension_numpy_batch_project(project_root)

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
    assert "PyArray_DescrNew" in data["supported_symbols"]
    assert "PyArray_DescrFromTypeObject" in data["supported_symbols"]
    assert "PyArray_DescrFromObject" in data["supported_symbols"]
    assert "PyArray_MultiplyList" in data["supported_symbols"]
    assert "PyArray_IntpFromSequence" in data["supported_symbols"]
    assert "PyArray_PyIntAsIntp" in data["supported_symbols"]
    assert "PyArray_CopyInto" in data["supported_symbols"]
    assert "PyArray_SetBaseObject" in data["supported_symbols"]
    assert "PyArray_Return" in data["supported_symbols"]
    assert "PyDataType_ELSIZE" in data["supported_symbols"]
    assert "PyDataType_FLAGS" in data["supported_symbols"]
    assert "PyDataMem_GetHandler" in data["supported_symbols"]
    assert "PyCapsule_SetContext" in data["supported_symbols"]
    assert "PyCapsule_GetContext" in data["supported_symbols"]
    assert "PyEval_SaveThread" in data["supported_symbols"]
    assert "PyEval_RestoreThread" in data["supported_symbols"]
    assert "PyExc_ModuleNotFoundError" in data["supported_symbols"]
    assert "PyErr_Print" in data["supported_symbols"]
    assert "PyUFunc_FromFuncAndData" in data["supported_symbols"]
    assert "PyUFunc_RegisterLoopForType" in data["supported_symbols"]


def test_extension_build_emits_wheel_and_manifest(tmp_path: Path, monkeypatch) -> None:
    project_root = tmp_path / "extproj"
    project_root.mkdir()
    _write_extension_project(project_root)

    def fake_ensure_runtime_lib(
        runtime_lib: Path,
        target_triple: str | None,
        json_output: bool,
        cargo_profile: str,
        project_root: Path,
        cargo_timeout: float | None,
    ) -> bool:
        del target_triple, json_output, cargo_profile, project_root, cargo_timeout
        runtime_lib.parent.mkdir(parents=True, exist_ok=True)
        runtime_lib.write_bytes(b"runtime")
        return True

    def fake_run(
        cmd: list[str],
        *,
        cwd: Path,
        env: dict[str, str],
        capture_output: bool,
        text: bool,
        check: bool,
    ) -> subprocess.CompletedProcess[str]:
        del cwd, env, capture_output, text, check
        out_index = cmd.index("-o")
        out_path = Path(cmd[out_index + 1])
        out_path.parent.mkdir(parents=True, exist_ok=True)
        if "-c" in cmd:
            out_path.write_bytes(b"obj")
        else:
            out_path.write_bytes(b"shared")
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli, "_ensure_runtime_lib", fake_ensure_runtime_lib)
    monkeypatch.setattr(cli.subprocess, "run", fake_run)

    out_dir = project_root / "dist"
    rc = cli.extension_build(
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

    with zipfile.ZipFile(wheel_path) as zf:
        names = set(zf.namelist())
        assert "extension_manifest.json" in names
        assert manifest["extension"] in names


def test_extension_build_cross_target_uses_target_runtime(
    tmp_path: Path, monkeypatch
) -> None:
    project_root = tmp_path / "extproj"
    project_root.mkdir()
    _write_extension_project(project_root)
    seen: dict[str, object] = {}
    commands: list[list[str]] = []

    def fake_ensure_runtime_lib(
        runtime_lib: Path,
        target_triple: str | None,
        json_output: bool,
        cargo_profile: str,
        project_root: Path,
        cargo_timeout: float | None,
    ) -> bool:
        del json_output, cargo_profile, project_root, cargo_timeout
        seen["runtime_target"] = target_triple
        seen["runtime_lib"] = runtime_lib
        runtime_lib.parent.mkdir(parents=True, exist_ok=True)
        runtime_lib.write_bytes(b"runtime")
        return True

    def fake_run(
        cmd: list[str],
        *,
        cwd: Path,
        env: dict[str, str],
        capture_output: bool,
        text: bool,
        check: bool,
    ) -> subprocess.CompletedProcess[str]:
        del cwd, env, capture_output, text, check
        commands.append(cmd)
        out_index = cmd.index("-o")
        out_path = Path(cmd[out_index + 1])
        out_path.parent.mkdir(parents=True, exist_ok=True)
        if "-c" in cmd:
            out_path.write_bytes(b"obj")
        else:
            out_path.write_bytes(b"shared")
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli, "_ensure_runtime_lib", fake_ensure_runtime_lib)
    monkeypatch.setattr(cli, "_ensure_rustup_target", lambda _target, _warnings: True)
    monkeypatch.setattr(
        cli.shutil, "which", lambda tool: "/usr/bin/zig" if tool == "zig" else None
    )
    monkeypatch.setattr(cli.subprocess, "run", fake_run)

    out_dir = project_root / "dist"
    target = "aarch64-unknown-linux-gnu"
    rc = cli.extension_build(
        project=str(project_root),
        out_dir=str(out_dir),
        deterministic=False,
        target=target,
        json_output=False,
        verbose=False,
    )
    assert rc == 0
    assert seen["runtime_target"] == target
    runtime_lib = seen["runtime_lib"]
    assert isinstance(runtime_lib, Path)
    assert f"/{target}/" in runtime_lib.as_posix()
    assert any(
        cmd[:2] == ["zig", "cc"] and "-target" in cmd and "-c" in cmd
        for cmd in commands
    )
    manifest = json.loads((out_dir / "extension_manifest.json").read_text())
    assert manifest["target_triple"] == target


def test_extension_build_rejects_wasm_target(tmp_path: Path) -> None:
    project_root = tmp_path / "extproj"
    project_root.mkdir()
    _write_extension_project(project_root)
    rc = cli.extension_build(
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
    seen: dict[str, object] = {}

    def fake_ensure_runtime_lib(
        runtime_lib: Path,
        target_triple: str | None,
        json_output: bool,
        cargo_profile: str,
        project_root: Path,
        cargo_timeout: float | None,
    ) -> bool:
        del json_output, cargo_profile, project_root, cargo_timeout
        seen["runtime_target"] = target_triple
        runtime_lib.parent.mkdir(parents=True, exist_ok=True)
        runtime_lib.write_bytes(b"runtime")
        return True

    def fake_run(
        cmd: list[str],
        *,
        cwd: Path,
        env: dict[str, str],
        capture_output: bool,
        text: bool,
        check: bool,
    ) -> subprocess.CompletedProcess[str]:
        del cwd, env, capture_output, text, check
        out_index = cmd.index("-o")
        out_path = Path(cmd[out_index + 1])
        out_path.parent.mkdir(parents=True, exist_ok=True)
        if "-c" in cmd:
            out_path.write_bytes(b"obj")
        else:
            out_path.write_bytes(b"shared")
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli, "_ensure_runtime_lib", fake_ensure_runtime_lib)
    monkeypatch.setattr(cli.subprocess, "run", fake_run)

    if target is not None:
        monkeypatch.setattr(
            cli, "_ensure_rustup_target", lambda _target, _warnings: True
        )
        monkeypatch.setattr(
            cli.shutil, "which", lambda tool: "/usr/bin/zig" if tool == "zig" else None
        )

    out_dir = project_root / ("dist-" + (target or "native"))
    rc = cli.extension_build(
        project=str(project_root),
        out_dir=str(out_dir),
        deterministic=False,
        target=target,
        json_output=False,
        verbose=False,
    )
    assert rc == 0
    assert seen["runtime_target"] == target

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
    result = subprocess.run(
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
    result = subprocess.run(
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
                "    npy_intp mult = PyArray_MultiplyList(PyArray_DIMS(arr), PyArray_NDIM(arr));",
                "    npy_bool bool_out = 0;",
                "    int is_int = PyTypeNum_ISINTEGER(PyArray_TYPE(arr));",
                "    int is_scalar = PyArray_CheckScalar(obj);",
                "    int is_datetime = PyArray_ISDATETIME(arr);",
                "    int has_c = PyArray_ISCONTIGUOUS(arr);",
                "    int has_f = PyArray_IS_F_CONTIGUOUS(arr);",
                "    int eq = PyArray_EquivTypes(descr, scalar_descr);",
                "    PyArray_Dims dims_out = {0};",
                "    int conv = PyArray_IntpConverter(obj, &dims_out);",
                "    PyArrayObject *copied = PyArray_NewCopy(arr, 0);",
                "    PyObject *returned = PyArray_Return(arr);",
                "    PyObject *view_obj = PyArray_FromArray(arr, descr, 0);",
                "    int copied_into = PyArray_CopyInto(arr, copied);",
                "    int assigned = PyArray_AssignArray(arr, copied, NULL, 0);",
                "    int bool_conv = PyArray_BoolConverter(obj, &bool_out);",
                "    npy_intp value_intp = PyArray_PyIntAsIntp(PyLong_FromLong(5));",
                "    PyArray_DTypeMeta *dtype_from_num = PyArray_DTypeFromTypeNum(NPY_DOUBLE);",
                "    PyArray_Descr *descr2 = NULL;",
                "    int descr_ok = PyArray_DescrConverter(obj, &descr2);",
                "    PyArray_Descr *byteorder_descr = PyArray_DescrNewByteorder(descr, '>');",
                "    PyArray_Descr *descr_copy = PyArray_DescrNew(descr);",
                "    PyObject *dims_tuple = PyArray_IntTupleFromIntp(PyArray_NDIM(arr), PyArray_DIMS(arr));",
                "    int writeback = PyArray_ResolveWritebackIfCopy(arr);",
                "    int is_aligned = PyArray_ISALIGNED(arr);",
                "    int is_bool = PyArray_ISBOOL(arr);",
                "    int is_integer = PyArray_ISINTEGER(arr);",
                "    int is_object = PyArray_ISOBJECT(arr);",
                "    int is_byteswapped = PyArray_ISBYTESWAPPED(arr);",
                "    int is_writeable = PyArray_ISWRITEABLE(arr);",
                "    int one_segment = PyArray_ISONESEGMENT(arr);",
                "    PyObject *base = PyArray_BASE(arr);",
                "    PyArray_Descr *dtype_descr = PyArray_DTYPE(arr);",
                "    PyObject *ensured = PyArray_EnsureArray(obj);",
                "    PyObject *ensured_any = PyArray_EnsureAnyArray(obj);",
                "    PyArray_DTypeMeta *bool_dtype = PyArray_BoolDType;",
                "    PyArray_DTypeMeta *intp_dtype = PyArray_IntpDType;",
                "    PyArray_DTypeMeta *bytes_dtype = PyArray_BytesDType;",
                "    PyArray_DTypeMeta *unicode_dtype = PyArray_UnicodeDType;",
                "    PyArray_DTypeMeta *object_dtype = PyArray_ObjectDType;",
                "    PyArray_DTypeMeta *complex_dtype = PyArray_PyComplexDType;",
                "    PyArray_DTypeMeta *default_int_dtype = PyArray_DefaultIntDType;",
                "    PyArray_DTypeMeta *int_abstract_dtype = PyArray_IntAbstractDType;",
                "    NPY_ORDER order = NPY_CORDER;",
                "    int order_ok = PyArray_OrderConverter(Py_None, &order);",
                "    npy_intp array_size = PyArray_Size(obj);",
                "    PyObject *int_from_intp = PyArray_PyIntFromIntp(7);",
                "    npy_bool can_cast_type = PyArray_CanCastTypeTo(descr, scalar_descr, NPY_SAFE_CASTING);",
                "    npy_bool can_cast_arr = PyArray_CanCastArrayTo(arr, descr, NPY_SAFE_CASTING);",
                "    int can_cast_safe = PyArray_CanCastSafely(NPY_INT, NPY_LONG);",
                "    PyObject *new_like = PyArray_NewLikeArray(arr, NPY_KEEPORDER, descr, 0);",
                "    PyObject *view2 = PyArray_View(arr, descr, NULL);",
                "    PyObject *transposed = PyArray_Transpose(arr, NULL);",
                "    int axis = 0;",
                "    PyObject *axis_checked = PyArray_CheckAxis(arr, &axis, 0);",
                "    PyObject *casted = PyArray_CastToType(arr, descr, 0);",
                "    PyArray_Descr *promoted = PyArray_PromoteTypes(descr, scalar_descr);",
                "    PyArray_Descr *adapted = PyArray_AdaptDescriptorToArray(arr, bool_dtype, descr);",
                "    npy_intp view_offset = 0;",
                "    npy_intp safe_cast = PyArray_SafeCast(descr, scalar_descr, &view_offset, NPY_SAFE_CASTING, 1);",
                "    PyObject *scalar = PyArray_Scalar(NULL, descr, obj);",
                "    PyObject *scalar2 = PyArray_ToScalar(NULL, arr);",
                "    PyObject *tuple_items[1] = {obj};",
                "    PyObject *items_tuple = PyArray_TupleFromItems(1, tuple_items, 1);",
                "    PyArray_UpdateFlags(arr, NPY_ARRAY_ALIGNED | NPY_ARRAY_NOTSWAPPED);",
                "    PyArray_DTypeMeta *long_dtype = PyArray_PyLongDType;",
                "    PyArray_DTypeMeta *float_dtype = PyArray_PyFloatDType;",
                "    PyArray_DTypeMeta *cfloat_dtype = PyArray_CFloatDType;",
                "    PyArray_DTypeMeta *cdouble_dtype = PyArray_CDoubleDType;",
                "    PyArray_DTypeMeta *string_dtype = PyArray_StringDType;",
                "    void *subarray = PyDataType_SUBARRAY(descr);",
                "    PyObject *names = PyDataType_NAMES(descr);",
                "    PyObject *fields = PyDataType_FIELDS(descr);",
                "    int is_unsized = PyDataType_ISUNSIZED(descr);",
                "    int is_legacy = PyDataType_ISLEGACY(descr);",
                "    int dtype_not_swapped = PyDataType_ISNOTSWAPPED(descr);",
                "    npy_intp *shape = PyArray_SHAPE(arr);",
                "    PyArray_ArrayDescr array_descr = {descr, NULL};",
                "    PyArray_Chunk chunk = {0};",
                "    PyArrayInterface iface = {0};",
                "    PyArrayMapIterObject map_iter = {0};",
                "    PyArrayNeighborhoodIterObject neighborhood_iter = {0};",
                "    PyArray_StringDTypeObject string_dtype_obj = {0};",
                "    struct PyArrayMethodObject_tag *method_tag = NULL;",
                "    PyArray_GetItemFunc *get_item = NULL;",
                "    PyArrayMethod_GetTraverseLoop *get_traverse = NULL;",
                "    PyArrayMethod_GetMaskedStridedLoop *get_masked = NULL;",
                "    PyArrayMethod_GetReductionInitial *get_reduction_initial = NULL;",
                "    PyArrayMethod_ResolveDescriptors *resolve_descrs = NULL;",
                "    PyArrayMethod_PromoterFunction *promoter = NULL;",
                "    PyArrayMethod_TranslateGivenDescriptors *translate_given = NULL;",
                "    PyArrayMethod_TranslateLoopDescriptors *translate_loop = NULL;",
                "    NPY_ARRAYMETHOD_FLAGS method_flags = NPY_METH_REQUIRES_PYAPI;",
                "    int combined_flags = PyArrayMethod_COMBINED_FLAGS(method_flags, NPY_METH_SUPPORTS_UNALIGNED);",
                "    int minimal_flags = PyArrayMethod_MINIMAL_FLAGS;",
                "    PyArrayMethod_SortParameters sort_params = {0};",
                "    PyTypeObject *method_type = &PyArrayMethod_Type;",
                "    PyTypeObject *bound_method_type = &PyBoundArrayMethod_Type;",
                "    PyTypeObject *iter_type = &PyArrayIter_Type;",
                "    PyTypeObject *map_iter_type = &PyArrayMapIter_Type;",
                "    PyUFuncGenericFunction generic_fn = NULL;",
                "    PyTypeObject *ufunc_type = &PyUFunc_Type;",
                "    int ufunc_none = PyUFunc_None;",
                "    int fp_errors = PyUFunc_GiveFloatingpointErrors(\"numpy_smoke\", 0);",
                "    (void)PyArray_ClearBuffer;",
                "    (void)PyArray_AddCastingImplementation_FromSpec;",
                "    (void)PyArrayMethod_FromSpec_int;",
                "    (void)PyUFunc_AddLoop;",
                "    (void)PyUFunc_AddLoopFromSpec_int;",
                "    (void)PyArray_ImportNumPyAPI;",
                "    int pybuf_simple = PyBUF_SIMPLE;",
                "    int pybuf_writable = PyBUF_WRITABLE;",
                "    int mod_multi = Py_mod_multiple_interpreters;",
                "    size_t vector_nargs = PyVectorcall_NARGS(1);",
                "    PyThread_type_lock lock = NULL;",
                "    PyMutex mutex = {0};",
                "    PyLockStatus lock_status = PY_LOCK_FAILURE;",
                "    PyTupleObject tuple_obj = {0};",
                "    Py_uhash_t uhash = 0;",
                "    PyTypeObject *tuple_type = &PyTuple_Type;",
                "    PyTypeObject *type_type = &PyType_Type;",
                "    PyNumberMethods numbers = {0};",
                "    int overflow = 0;",
                "    long long_val = PyLong_AsLongAndOverflow(PyLong_FromLong(1), &overflow);",
                "    Py_ssize_t ssize_val = PyLong_AsSsize_t(PyLong_FromLong(2));",
                "    Py_ssize_t number_ssize = PyNumber_AsSsize_t(PyLong_FromLong(3), NULL);",
                "    double huge_val = Py_HUGE_VAL;",
                "    Py_ssize_t tuple_size_macro = Py_SIZE((PyObject *)&tuple_obj);",
                "    PyObject *unicode_concat = PyUnicode_Concat(PyUnicode_FromString(\"a\"), PyUnicode_FromString(\"b\"));",
                "    int unicode_cmp = PyUnicode_Compare(PyUnicode_FromString(\"a\"), PyUnicode_FromString(\"b\"));",
                "    Py_ssize_t unicode_len = PyUnicode_GET_LENGTH(PyUnicode_FromString(\"abc\"));",
                "    int unicode_space = Py_UNICODE_ISSPACE(' ');",
                "    char endian = '=';",
                "    int byteorder_ok = PyArray_ByteorderConverter(Py_None, &endian);",
                "    int dict_exact = PyDict_CheckExact(PyDict_New());",
                "    int err_match = PyErr_GivenExceptionMatches(PyExc_TypeError, PyExc_TypeError);",
                "    int type_check = PyType_Check((PyObject *)&PyType_Type);",
                "    int multi_interp_not_supported = Py_MOD_MULTIPLE_INTERPRETERS_NOT_SUPPORTED;",
                "    PyUFunc_LoopSlot loop_slot = {0};",
                "    (void)PyObject_Vectorcall;",
                "    (void)PyObject_Dir;",
                "    (void)PyObject_Format;",
                "    (void)PyObject_GetIter;",
                "    (void)PyObject_Length;",
                "    (void)PyObject_Size;",
                "    (void)PyUnicode_AsEncodedString;",
                "    (void)PyUnicode_FromWideChar;",
                "    (void)PyTraceMalloc_Track;",
                "    (void)PyTraceMalloc_Untrack;",
                "    (void)PyThread_allocate_lock;",
                "    (void)PyThread_free_lock;",
                "    (void)PyThread_acquire_lock;",
                "    (void)PyThread_acquire_lock_timed;",
                "    (void)PyThread_release_lock;",
                "    (void)PyMutex_Lock;",
                "    (void)PyMutex_Unlock;",
                "    (void)PyDict_DelItem;",
                "    (void)PyList_GetItemRef;",
                "    (void)PyArray_CopyObject;",
                "    (void)PyArray_PromoteDTypeSequence;",
                "    (void)PyArray_GenericBinaryFunction;",
                "    (void)PyArray_GenericReduceFunction;",
                "    (void)PyArray_GetCastingImpl;",
                "    (void)PyArray_CastDescrToDType;",
                "    (void)PyArray_AssignRawScalar;",
                "    (void)PyArray_GetStridedCopyFn;",
                "    (void)PyArray_CastRawArrays;",
                "    (void)PyArray_PrepareTwoRawArrayIter;",
                "    (void)PyArray_LookupSpecial;",
                "    (void)PyArray_LookupSpecial_OnInstance;",
                "    (void)PyArray_Any;",
                "    (void)PyArray_BufferConverter;",
                "    (void)PyArray_ByteorderConverter;",
                "    (void)PyArrayNeighborhoodIter_Next;",
                "    (void)PyArrayNeighborhoodIter_Reset;",
                "    (void)PyUFunc_FromFuncAndData;",
                "    (void)PyUFunc_FromFuncAndDataAndSignature;",
                "    (void)PyUFunc_AddLoopsFromSpecs;",
                "    import_array1(-1);",
                "    if (descr != NULL) {",
                "        PyMem_Free(descr);",
                "    }",
                "    if (scalar_descr != NULL) {",
                "        PyMem_Free(scalar_descr);",
                "    }",
                "    (void)mult;",
                "    (void)bool_out;",
                "    (void)eq;",
                "    (void)conv;",
                "    (void)copied;",
                "    (void)returned;",
                "    (void)view_obj;",
                "    (void)copied_into;",
                "    (void)assigned;",
                "    (void)bool_conv;",
                "    (void)value_intp;",
                "    (void)dtype_from_num;",
                "    (void)descr_ok;",
                "    (void)byteorder_descr;",
                "    (void)descr_copy;",
                "    (void)dims_tuple;",
                "    (void)writeback;",
                "    (void)is_aligned;",
                "    (void)is_bool;",
                "    (void)is_integer;",
                "    (void)is_object;",
                "    (void)is_byteswapped;",
                "    (void)is_writeable;",
                "    (void)one_segment;",
                "    (void)base;",
                "    (void)dtype_descr;",
                "    (void)ensured;",
                "    (void)ensured_any;",
                "    (void)bool_dtype;",
                "    (void)intp_dtype;",
                "    (void)bytes_dtype;",
                "    (void)unicode_dtype;",
                "    (void)object_dtype;",
                "    (void)complex_dtype;",
                "    (void)default_int_dtype;",
                "    (void)int_abstract_dtype;",
                "    (void)order;",
                "    (void)order_ok;",
                "    (void)array_size;",
                "    (void)int_from_intp;",
                "    (void)can_cast_type;",
                "    (void)can_cast_arr;",
                "    (void)can_cast_safe;",
                "    (void)new_like;",
                "    (void)view2;",
                "    (void)transposed;",
                "    (void)axis_checked;",
                "    (void)casted;",
                "    (void)promoted;",
                "    (void)adapted;",
                "    (void)view_offset;",
                "    (void)safe_cast;",
                "    (void)scalar;",
                "    (void)scalar2;",
                "    (void)items_tuple;",
                "    (void)long_dtype;",
                "    (void)float_dtype;",
                "    (void)cfloat_dtype;",
                "    (void)cdouble_dtype;",
                "    (void)string_dtype;",
                "    (void)subarray;",
                "    (void)names;",
                "    (void)fields;",
                "    (void)is_unsized;",
                "    (void)is_legacy;",
                "    (void)dtype_not_swapped;",
                "    (void)shape;",
                "    (void)array_descr;",
                "    (void)chunk;",
                "    (void)iface;",
                "    (void)map_iter;",
                "    (void)neighborhood_iter;",
                "    (void)string_dtype_obj;",
                "    (void)method_tag;",
                "    (void)get_item;",
                "    (void)get_traverse;",
                "    (void)get_masked;",
                "    (void)get_reduction_initial;",
                "    (void)resolve_descrs;",
                "    (void)promoter;",
                "    (void)translate_given;",
                "    (void)translate_loop;",
                "    (void)combined_flags;",
                "    (void)minimal_flags;",
                "    (void)sort_params;",
                "    (void)method_type;",
                "    (void)bound_method_type;",
                "    (void)iter_type;",
                "    (void)map_iter_type;",
                "    (void)has_c;",
                "    (void)has_f;",
                "    (void)generic_fn;",
                "    (void)ufunc_type;",
                "    (void)ufunc_none;",
                "    (void)pybuf_simple;",
                "    (void)pybuf_writable;",
                "    (void)mod_multi;",
                "    (void)vector_nargs;",
                "    (void)lock;",
                "    (void)mutex;",
                "    (void)lock_status;",
                "    (void)tuple_obj;",
                "    (void)uhash;",
                "    (void)tuple_type;",
                "    (void)type_type;",
                "    (void)numbers;",
                "    (void)long_val;",
                "    (void)ssize_val;",
                "    (void)number_ssize;",
                "    (void)huge_val;",
                "    (void)tuple_size_macro;",
                "    (void)unicode_concat;",
                "    (void)unicode_cmp;",
                "    (void)unicode_len;",
                "    (void)unicode_space;",
                "    (void)endian;",
                "    (void)byteorder_ok;",
                "    (void)dict_exact;",
                "    (void)err_match;",
                "    (void)type_check;",
                "    (void)multi_interp_not_supported;",
                "    (void)loop_slot;",
                "    (void)fp_errors;",
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
    result = subprocess.run(
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


def test_numpy_header_arrayobject_batch_smoke(tmp_path: Path) -> None:
    clang = shutil.which("clang")
    if clang is None:
        pytest.skip("clang is required for NumPy batch compatibility header smoke test")
    source = tmp_path / "numpy_h_arrayobject_batch_smoke.c"
    source.write_text(
        "\n".join(
            [
                "#include <Python.h>",
                "#include <numpy/arrayobject.h>",
                "#include <numpy/npy_common.h>",
                "#include <numpy/npy_math.h>",
                "#include <numpy/dtype_api.h>",
                "#include <numpy/__multiarray_api.h>",
                "#include <numpy/__ufunc_api.h>",
                "#include <numpy/npy_2_compat.h>",
                "",
                "int main(void) {",
                "    PyArrayObject arr = {0};",
                "    PyArray_Descr descr = {0};",
                "    npy_intp dims[2] = {2, 3};",
                "    char data[6] = {0};",
                "    npy_bool bool_flag = 0;",
                "    NPY_ORDER order = NPY_KEEPORDER;",
                "    NPY_SORTKIND sortkind = NPY_QUICKSORT;",
                "    NPY_SEARCHSIDE side = NPY_SEARCHLEFT;",
                "    NPY_CLIPMODE clipmode = NPY_CLIP;",
                "    char byteorder = '=';",
                "    PyObject *capsule = PyCapsule_New((void *)data, \"demo.capsule\", NULL);",
                "    PyObject *shape = PyTuple_Pack(2, PyLong_FromLong(2), PyLong_FromLong(3));",
                "    PyUFuncGenericFunction fn = PyUFunc_O_O;",
                "    (void)fn;",
                "    descr.type_num = NPY_INT;",
                "    descr.elsize = 1;",
                "    descr.byteorder = '=';",
                "    arr.data = data;",
                "    arr.nd = 2;",
                "    arr.dimensions = dims;",
                "    arr.strides = dims;",
                "    arr.descr = &descr;",
                "    arr.flags = NPY_ARRAY_CARRAY | NPY_ARRAY_WRITEABLE;",
                "    (void)PyArray_Size(&arr);",
                "    (void)PyArray_MAX(1, 2);",
                "    (void)PyArray_MIN(1, 2);",
                "    (void)PyArray_MultiplyList(dims, 2);",
                "    (void)PyArray_IntpFromSequence(shape, dims, 2);",
                "    (void)PyArray_BoolConverter(Py_True, &bool_flag);",
                "    (void)PyArray_OrderConverter(PyLong_FromLong(NPY_CORDER), &order);",
                "    (void)PyArray_SortkindConverter(PyLong_FromLong(NPY_QUICKSORT), &sortkind);",
                "    (void)PyArray_SearchsideConverter(PyLong_FromLong(NPY_SEARCHLEFT), &side);",
                "    (void)PyArray_ClipmodeConverter(PyLong_FromLong(NPY_CLIP), &clipmode);",
                "    (void)PyArray_ByteorderConverter(PyUnicode_FromString(\"<\"), &byteorder);",
                "    (void)PyArray_CopyInto(&arr, &arr);",
                "    (void)PyArray_SetBaseObject(&arr, Py_None);",
                "    (void)PyDataType_ELSIZE(&descr);",
                "    (void)PyDataType_FLAGS(&descr);",
                "    (void)PyDataType_ISINTEGER(&descr);",
                "    (void)PyDataMem_GetHandler();",
                "    (void)PyDataMem_SetHandler(Py_None);",
                "    (void)PyCapsule_SetContext(capsule, data);",
                "    (void)PyCapsule_GetContext(capsule);",
                "    (void)PyUFunc_ImportUFuncAPI();",
                "    (void)PyUFunc_FromFuncAndData(NULL, NULL, NULL, 0, 1, 1, PyUFunc_None, \"demo\", NULL, 0);",
                "    (void)PyUFunc_RegisterLoopForType(Py_None, NPY_INT, fn, NULL, NULL);",
                "    (void)PyArrayMethod_GetLoop(NULL, NULL, 0, 0, NULL, NULL, NULL);",
                "    (void)PyArrayMethod_ResolveDescriptors(NULL, NULL, NULL, NULL, NULL);",
                "    (void)PyExc_ModuleNotFoundError;",
                "    (void)PyExc_IOError;",
                "    Py_XDECREF(shape);",
                "    Py_XDECREF(capsule);",
                "    return 0;",
                "}",
                "",
            ]
        )
    )
    result = subprocess.run(
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
    result = subprocess.run(
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
