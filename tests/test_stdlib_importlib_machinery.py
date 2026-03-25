from __future__ import annotations

import builtins
import importlib.util
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "src" / "molt" / "stdlib" / "importlib" / "machinery.py"

_MACHINERY_INTRINSICS = [
    "molt_stdlib_probe",
    "molt_importlib_source_exec_payload",
    "molt_importlib_zip_source_exec_payload",
    "molt_importlib_read_file",
    "molt_importlib_coerce_module_name",
    "molt_importlib_pathfinder_find_spec",
    "molt_importlib_filefinder_find_spec",
    "molt_importlib_filefinder_invalidate",
    "molt_importlib_exec_restricted_source",
    "molt_importlib_exec_extension",
    "molt_importlib_exec_sourceless",
    "molt_importlib_extension_loader_payload",
    "molt_importlib_sourceless_loader_payload",
    "molt_importlib_module_spec_is_package",
    "molt_importlib_resources_reader_resource_path_from_roots",
    "molt_importlib_resources_reader_open_resource_bytes_from_roots",
    "molt_importlib_resources_reader_is_resource_from_roots",
    "molt_importlib_resources_reader_contents_from_roots",
    "molt_importlib_path_is_archive_member",
    "molt_importlib_package_root_from_origin",
    "molt_importlib_validate_resource_name",
    "molt_importlib_set_module_state",
    "molt_importlib_stabilize_module_state",
    "molt_exception_clear",
    "molt_module_import",
]


def _coerce_stub(module, loader, spec=None):
    """Pure-Python fallback matching the runtime _coerce_module_name intrinsic."""
    name = getattr(module, "__name__", None)
    if isinstance(name, str):
        return name
    if spec is None:
        spec = getattr(module, "__spec__", None)
    if spec is not None:
        sname = getattr(spec, "name", None)
        if isinstance(sname, str):
            return sname
    if loader is not None:
        lname = getattr(loader, "name", None)
        if isinstance(lname, str):
            return lname
    raise TypeError("module name must be str")


def _load_machinery_module():
    registry = getattr(builtins, "_molt_intrinsics", None)
    if not isinstance(registry, dict):
        registry = {}
        builtins._molt_intrinsics = registry

    _noop = lambda *a, **kw: None
    for name in _MACHINERY_INTRINSICS:
        registry.setdefault(name, _noop)
    registry["molt_importlib_coerce_module_name"] = _coerce_stub

    def _lookup(intrinsic_name):
        return registry.get(intrinsic_name)

    builtins._molt_intrinsic_lookup = _lookup

    for key in list(sys.modules):
        if "molt_stdlib_importlib_machinery" in key:
            sys.modules.pop(key, None)

    spec = importlib.util.spec_from_file_location(
        "molt_stdlib_importlib_machinery", SCRIPT_PATH
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_coerce_module_name_prefers_spec_name_when_module_name_invalid() -> None:
    machinery = _load_machinery_module()

    class _Module:
        __name__ = 123
        __spec__ = type("Spec", (), {"name": "resolved.from.spec"})()

    resolved = machinery._coerce_module_name(_Module(), loader=None)  # noqa: SLF001
    assert resolved == "resolved.from.spec"


def test_coerce_module_name_prefers_loader_name_when_spec_missing() -> None:
    machinery = _load_machinery_module()

    class _Loader:
        name = "resolved.from.loader"

    class _Module:
        __name__ = 123
        __spec__ = None

    resolved = machinery._coerce_module_name(  # noqa: SLF001
        _Module(), loader=_Loader()
    )
    assert resolved == "resolved.from.loader"


def test_coerce_module_name_raises_without_any_string_source() -> None:
    machinery = _load_machinery_module()

    class _Loader:
        name = 42

    class _Module:
        __name__ = None
        __spec__ = type("Spec", (), {"name": 99})()

    try:
        machinery._coerce_module_name(  # noqa: SLF001
            _Module(), loader=_Loader()
        )
    except TypeError as exc:
        assert str(exc) == "module name must be str"
    else:
        raise AssertionError("expected TypeError")
