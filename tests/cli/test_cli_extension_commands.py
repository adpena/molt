from __future__ import annotations

import hashlib
import json
import shutil
import subprocess
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
    manifest = json.loads((out_dir / "extension_manifest.json").read_text())
    assert manifest["module"] == "demoext_iter"
    assert manifest["capabilities"] == ["fs.read"]
    with zipfile.ZipFile(wheels[0]) as zf:
        names = set(zf.namelist())
        assert "extension_manifest.json" in names
        assert manifest["extension"] in names
        assert zf.read(manifest["extension"])


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
                "    int is_int = PyTypeNum_ISINTEGER(PyArray_TYPE(arr));",
                "    int is_scalar = PyArray_CheckScalar(obj);",
                "    int is_datetime = PyArray_ISDATETIME(arr);",
                "    import_array1(-1);",
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
