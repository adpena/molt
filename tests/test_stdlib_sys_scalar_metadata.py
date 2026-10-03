from __future__ import annotations

import importlib.util
import sys
import types
from pathlib import Path

import pytest


SYS_PATH = Path(__file__).resolve().parents[1] / "src/molt/stdlib/sys.py"


def _bootstrap_namespace() -> dict[str, object]:
    # The native module publisher owns these facts before executing sys.py.
    # CPython supplies independent values for this Python-only facade fixture.
    return {
        "modules": {},
        "platform": sys.platform,
        "version": sys.version,
        "version_info": tuple(sys.version_info),
        "hexversion": sys.hexversion,
        "api_version": sys.api_version,
        "implementation": sys.implementation,
        "maxsize": 2**31 - 1,
        "maxunicode": 0x10FFFF,
        "byteorder": "big",
        "prefix": "native-prefix",
        "exec_prefix": "native-exec-prefix",
        "base_prefix": "native-base-prefix",
        "base_exec_prefix": "native-base-exec-prefix",
        "platlibdir": getattr(sys, "platlibdir", "lib"),
        "argv": ["native-program"],
        "executable": "native-program",
        "path": ["native-path"],
        "meta_path": [],
        "path_hooks": [],
        "path_importer_cache": {},
        "orig_argv": ["native-origin"],
        "copyright": "native-copyright",
        "stdlib_module_names": tuple(sys.stdlib_module_names),
        "builtin_module_names": sys.builtin_module_names,
        "stdin": sys.stdin,
        "stdout": sys.stdout,
        "stderr": sys.stderr,
        "__stdin__": sys.stdin,
        "__stdout__": sys.stdout,
        "__stderr__": sys.stderr,
        **({"abiflags": sys.abiflags} if hasattr(sys, "abiflags") else {}),
    }


def _load_sys_shim(
    monkeypatch,
    *,
    bootstrap=None,
    unavailable=frozenset(),
    overrides=None,
    resolver_failures=None,
):
    require_calls = []
    intrinsics = types.ModuleType("_intrinsics")
    flags = {
        name: int(getattr(sys.flags, name))
        for name in dir(sys.flags)
        if not name.startswith("_") and isinstance(getattr(sys.flags, name), int)
    }
    flags.setdefault("gil", 1)
    values = {
        "molt_getframe": sys._getframe,
        "molt_sys_flags_payload": lambda: flags,
        "molt_sys_float_info": lambda: tuple(sys.float_info),
        "molt_sys_int_info": lambda: tuple(sys.int_info),
        "molt_sys_hash_info": lambda: tuple(sys.hash_info),
        "molt_sys_thread_info": lambda: tuple(sys.thread_info),
    }
    values.update(overrides or {})

    def require_intrinsic(name, _namespace=None):
        require_calls.append(name)
        if resolver_failures and name in resolver_failures:
            raise resolver_failures[name]
        if name in unavailable:
            raise RuntimeError(f"intrinsic unavailable: {name}")
        if name in values:
            return values[name]

        def unexpected_execution(*_args, **_kwargs):
            raise AssertionError(
                f"API intrinsic executed during sys publication: {name}"
            )

        return unexpected_execution

    intrinsics.require_intrinsic = require_intrinsic
    monkeypatch.setitem(sys.modules, "_intrinsics", intrinsics)
    module_name = f"_molt_test_sys_publication_{id(intrinsics)}"
    spec = importlib.util.spec_from_file_location(module_name, SYS_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    module.__dict__.update(_bootstrap_namespace() if bootstrap is None else bootstrap)
    monkeypatch.setitem(sys.modules, module_name, module)
    spec.loader.exec_module(module)
    return module, require_calls


def test_native_bootstrap_values_survive_eager_publication(monkeypatch):
    bootstrap = _bootstrap_namespace()
    module, calls = _load_sys_shim(monkeypatch, bootstrap=bootstrap)
    for name in (
        "maxsize",
        "maxunicode",
        "byteorder",
        "prefix",
        "exec_prefix",
        "base_prefix",
        "base_exec_prefix",
        "argv",
        "executable",
        "orig_argv",
        "path",
        "meta_path",
        "path_hooks",
        "path_importer_cache",
        "stdin",
        "stdout",
        "stderr",
    ):
        assert module.__dict__[name] is bootstrap[name]
    assert not {
        "molt_sys_maxsize",
        "molt_sys_maxunicode",
        "molt_sys_byteorder",
        "molt_sys_stdin",
        "molt_sys_stdout",
        "molt_sys_stderr",
    }.intersection(calls)


@pytest.mark.parametrize("name", ["maxsize", "maxunicode", "byteorder"])
def test_native_bootstrap_scalar_is_required(monkeypatch, name):
    bootstrap = _bootstrap_namespace()
    del bootstrap[name]
    with pytest.raises(KeyError, match=name):
        _load_sys_shim(monkeypatch, bootstrap=bootstrap)


@pytest.mark.parametrize(
    ("name", "value"),
    [
        ("maxsize", True),
        ("maxsize", 0),
        ("maxunicode", False),
        ("maxunicode", -1),
        ("maxunicode", 0x110000),
        ("byteorder", b"little"),
        ("byteorder", "middle"),
    ],
)
def test_native_bootstrap_scalar_validation_is_eager(monkeypatch, name, value):
    bootstrap = _bootstrap_namespace()
    bootstrap[name] = value
    with pytest.raises(RuntimeError, match=f"invalid {name}"):
        _load_sys_shim(monkeypatch, bootstrap=bootstrap)


def test_shaped_metadata_preserves_intrinsic_failure(monkeypatch):
    failure = LookupError("target metadata unavailable")

    def fail():
        raise failure

    with pytest.raises(LookupError) as raised:
        _load_sys_shim(monkeypatch, overrides={"molt_sys_float_info": fail})
    assert raised.value is failure


@pytest.mark.parametrize(
    "intrinsic",
    [
        "molt_getrecursionlimit",
        "molt_sys_getfilesystemencodeerrors",
        "molt_sys_settrace",
        "molt_sys_intern",
        "molt_traceback_format_exception",
        "molt_sys_exit",
    ],
)
@pytest.mark.parametrize("error_type", [RuntimeError, TypeError])
def test_api_publication_preserves_exact_intrinsic_resolution_error(
    monkeypatch, intrinsic, error_type
):
    failure = error_type(f"missing required binding: {intrinsic}")
    with pytest.raises(error_type) as raised:
        _load_sys_shim(monkeypatch, resolver_failures={intrinsic: failure})
    assert raised.value is failure


def test_all_public_exports_exist_before_attribute_access(monkeypatch):
    module, _ = _load_sys_shim(monkeypatch)
    namespace = vars(module)
    assert all(name in namespace for name in namespace["__all__"])
    assert namespace["version_info"].major == sys.version_info.major
    assert namespace["implementation"].name == sys.implementation.name
    assert namespace["float_info"].max == sys.float_info.max
    assert isinstance(namespace["stdlib_module_names"], frozenset)
    assert namespace["displayhook"] is namespace["__displayhook__"]
    assert namespace["excepthook"] is namespace["__excepthook__"]
    assert namespace["unraisablehook"] is namespace["__unraisablehook__"]


def test_public_replacement_and_deletion_never_rerun_producers(monkeypatch):
    module, _ = _load_sys_shim(monkeypatch)
    marker = object()
    module.stdout = marker
    module.path.append("user-path")
    module.displayhook = marker
    del module.flags
    del module.getrecursionlimit
    assert module.version_info.major == sys.version_info.major
    assert module.stdout is marker and module.displayhook is marker
    assert module.path == ["native-path", "user-path"]
    assert not hasattr(module, "flags")
    assert not hasattr(module, "getrecursionlimit")
    assert "flags" not in vars(module) and "getrecursionlimit" not in vars(module)


def test_frame_publication_retains_runtime_callable_and_caller(monkeypatch):
    module, _ = _load_sys_shim(monkeypatch)
    assert module._getframe is sys._getframe
    assert module._getframe() is sys._getframe()


def test_frame_publication_rejects_unavailable_runtime_primitive(monkeypatch):
    with pytest.raises(RuntimeError, match="intrinsic unavailable: molt_getframe"):
        _load_sys_shim(monkeypatch, unavailable=frozenset({"molt_getframe"}))
