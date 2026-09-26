from __future__ import annotations

import builtins
import importlib.util
import sys
import types
from pathlib import Path

import pytest


REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "src" / "molt" / "stdlib" / "importlib" / "machinery.py"

_MACHINERY_INTRINSICS = [
    "molt_stdlib_probe",
    "molt_importlib_read_file",
    "molt_importlib_pathfinder_find_spec",
    "molt_importlib_filefinder_find_spec",
    "molt_importlib_filefinder_invalidate",
    "molt_importlib_sourcefileloader_exec_module",
    "molt_importlib_zip_source_loader_exec_module",
    "molt_importlib_extension_loader_exec_module",
    "molt_importlib_extension_loader_create_module",
    "molt_importlib_sourceless_loader_exec_module",
    "molt_importlib_resources_reader_resource_path_from_roots",
    "molt_importlib_resources_reader_open_resource_bytes_from_roots",
    "molt_importlib_resources_reader_is_resource_from_roots",
    "molt_importlib_resources_reader_contents_from_roots",
    "molt_importlib_package_root_from_origin",
    "molt_importlib_validate_resource_name",
    "molt_importlib_load_module_from_spec",
    "molt_exception_clear",
    "molt_exception_last",
    "molt_exception_last_pending",
    "molt_exception_pending",
    "molt_sys_platform",
    "molt_importlib_module_spec_type",
    "molt_importlib_compiled_loader",
    "molt_importlib_compiled_loader_types",
]

# Stands in for the runtime-owned class the facade must bind, not define.
_RUNTIME_MODULE_SPEC = type("ModuleSpec", (), {})
_RUNTIME_LOADER_BASE = type("_MoltLoader", (), {})
_RUNTIME_BUILTIN_IMPORTER = type("BuiltinImporter", (_RUNTIME_LOADER_BASE,), {})
_RUNTIME_FROZEN_IMPORTER = type("FrozenImporter", (_RUNTIME_LOADER_BASE,), {})
_RUNTIME_LOADER_TYPES = (
    _RUNTIME_LOADER_BASE,
    _RUNTIME_BUILTIN_IMPORTER,
    _RUNTIME_FROZEN_IMPORTER,
)
_RUNTIME_LOADER = _RUNTIME_BUILTIN_IMPORTER()


def _bootstrap_intrinsics():
    # Identity sentinels only: behavior is tested against the real Rust owner.
    return {
        "molt_importlib_module_spec_type": lambda: _RUNTIME_MODULE_SPEC,
        "molt_importlib_compiled_loader": lambda: _RUNTIME_LOADER,
        "molt_importlib_compiled_loader_types": lambda: _RUNTIME_LOADER_TYPES,
    }


def _load_machinery_module(missing_intrinsics=frozenset()):
    registry = {}
    builtins._molt_intrinsics = registry

    def _noop(*_args, **_kwargs):
        return None

    for name in _MACHINERY_INTRINSICS:
        if name not in missing_intrinsics:
            registry[name] = _noop
    registry["molt_sys_platform"] = lambda: sys.platform
    registry.update(
        (name, value)
        for name, value in _bootstrap_intrinsics().items()
        if name not in missing_intrinsics
    )

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


def test_module_spec_is_the_runtime_type_authority() -> None:
    machinery = _load_machinery_module()

    assert machinery.ModuleSpec is _RUNTIME_MODULE_SPEC


@pytest.mark.parametrize("missing", tuple(_bootstrap_intrinsics()))
def test_bootstrap_fails_closed_without_runtime_authority(missing: str) -> None:
    try:
        _load_machinery_module(missing_intrinsics={missing})
    except RuntimeError as exc:
        assert str(exc) == f"intrinsic unavailable: {missing}"
    else:
        raise AssertionError("expected missing bootstrap authority to fail closed")


def test_loader_classes_and_singleton_are_runtime_authorities() -> None:
    machinery = _load_machinery_module()
    assert machinery._MoltLoader is _RUNTIME_LOADER_BASE
    assert machinery._LoaderBasics is _RUNTIME_LOADER_BASE
    assert machinery.BuiltinImporter is _RUNTIME_BUILTIN_IMPORTER
    assert machinery.FrozenImporter is _RUNTIME_FROZEN_IMPORTER
    assert machinery._MOLT_LOADER is _RUNTIME_LOADER


def test_platform_suffixes_resolve_when_sys_is_partially_initialized() -> None:
    registry = getattr(builtins, "_molt_intrinsics", None)
    if not isinstance(registry, dict):
        registry = {}
        builtins._molt_intrinsics = registry
    registry["molt_sys_platform"] = lambda: "darwin"
    registry.update(_bootstrap_intrinsics())

    spec = importlib.util.spec_from_file_location(
        "molt_stdlib_importlib_machinery_partial_sys", SCRIPT_PATH
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    partial_sys = types.ModuleType("sys")
    original_sys_module = sys.modules["sys"]
    sys.modules["sys"] = partial_sys
    try:
        spec.loader.exec_module(module)
    finally:
        sys.modules["sys"] = original_sys_module

    assert module.EXTENSION_SUFFIXES == [".so", ".dylib"]


def test_ensure_intrinsics_does_not_publish_partial_registry() -> None:
    machinery = _load_machinery_module(missing_intrinsics={"molt_exception_pending"})

    for _ in range(2):
        try:
            machinery._ensure_intrinsics()  # noqa: SLF001
        except RuntimeError as exc:
            assert str(exc) == "intrinsic unavailable: molt_exception_pending"
        else:
            raise AssertionError("expected missing intrinsic to fail closed")

        assert machinery._MOLT_IMPORTLIB_INTRINSICS_READY is False  # noqa: SLF001
        assert (  # noqa: SLF001
            machinery._MOLT_IMPORTLIB_SOURCEFILELOADER_EXEC_MODULE is None
        )
        assert machinery._MOLT_EXCEPTION_PENDING is None  # noqa: SLF001
