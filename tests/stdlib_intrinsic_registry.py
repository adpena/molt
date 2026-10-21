"""The intrinsic registry for tests that run Molt stdlib sources on CPython.

A Molt stdlib module asks ``_intrinsics.require_intrinsic`` for its runtime
hooks, and the host interpreter has no runtime to answer. A test therefore
installs a registry: the real resolver (``src/molt/stdlib/_intrinsics.py``) in
strict mode, reading ``builtins._molt_intrinsics``. The registry always grants
the capability checks (``molt_capabilities_has``, ``molt_capabilities_trusted``);
a test passes only the behavior it exercises. There are no readiness anchors:
spec 0016 admits no intrinsic that exists only to make a module count.

A probe that runs in a child interpreter calls :func:`install_registry` once;
an in-process test uses :func:`intrinsic_registry`, which restores what it
replaced.
"""

from __future__ import annotations

import builtins
import ctypes
from collections.abc import Callable, Iterator, Mapping
from contextlib import contextmanager
import importlib.util
import os
from pathlib import Path
import sys
from types import ModuleType

ROOT = Path(__file__).resolve().parents[1]
RESOLVER = ROOT / "src" / "molt" / "stdlib" / "_intrinsics.py"

_CAPABILITY_CHECKS = ("molt_capabilities_has", "molt_capabilities_trusted")
_REGISTRY_BUILTINS = ("_molt_intrinsics", "_molt_intrinsics_strict")
_UNAME_RESULT_FIELDS = ("sysname", "nodename", "release", "version", "machine")


class _UnameField(ctypes.Structure):
    _fields_ = [("name", ctypes.c_char_p), ("doc", ctypes.c_char_p)]


class _UnameDescriptor(ctypes.Structure):
    _fields_ = [
        ("name", ctypes.c_char_p),
        ("doc", ctypes.c_char_p),
        ("fields", ctypes.POINTER(_UnameField)),
        ("n_in_sequence", ctypes.c_int),
    ]


def _grant(*_args: object) -> bool:
    return True


def capability_grants() -> dict[str, Callable[..., bool]]:
    """The capability checks, each granting every capability."""

    return {name: _grant for name in _CAPABILITY_CHECKS}


def _load_resolver() -> ModuleType:
    spec = importlib.util.spec_from_file_location("_intrinsics", RESOLVER)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load the intrinsic resolver from {RESOLVER}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _uname_result_type(module: str) -> type[tuple]:
    """CPython's fixed uname descriptor for an explicitly selected OS module."""
    import weakref

    assert module in {"posix", "nt"}
    host = os.uname_result
    if host.__module__ == module:
        return host

    fields = (_UnameField * (len(_UNAME_RESULT_FIELDS) + 1))()
    for index, name in enumerate(_UNAME_RESULT_FIELDS):
        doc = getattr(host, name).__doc__
        fields[index] = _UnameField(
            name.encode("utf-8"), None if doc is None else doc.encode("utf-8")
        )
    descriptor = _UnameDescriptor(
        f"{module}.uname_result".encode("utf-8"),
        None if host.__doc__ is None else host.__doc__.encode("utf-8"),
        fields,
        len(_UNAME_RESULT_FIELDS),
    )
    new_type = ctypes.PYFUNCTYPE(ctypes.c_void_p, ctypes.POINTER(_UnameDescriptor))(
        ("PyStructSequence_NewType", ctypes.pythonapi)
    )
    decref = ctypes.PYFUNCTYPE(None, ctypes.c_void_p)(("Py_DecRef", ctypes.pythonapi))
    pointer = new_type(ctypes.byref(descriptor))
    if not pointer:
        raise MemoryError("PyStructSequence_NewType returned NULL")
    try:
        cls = ctypes.cast(pointer, ctypes.py_object).value
        # CP member names/docs borrow the descriptor's UTF-8 storage. The
        # finalizer retains that storage without retaining or mutating the type.
        weakref.finalize(cls, lambda _storage: None, descriptor)
        return cls
    finally:
        decref(pointer)  # The returned Python type owns its own reference.


def struct_sequence_intrinsics() -> dict[str, Callable[..., object]]:
    """Explicit CPython providers for the sys, time and os source loaders.

    These are real CPython type objects and the public C construction API. They
    provide no Python tuple-class implementation, and are never default grants.
    Native/C tests separately prove Molt's implementation of this boundary.
    """
    import time

    types = {
        "sys.version_info": type(sys.version_info),
        "sys.flags": type(sys.flags),
        "sys.float_info": type(sys.float_info),
        "sys.int_info": type(sys.int_info),
        "sys.hash_info": type(sys.hash_info),
        "sys.thread_info": type(sys.thread_info),
        "asyncgen_hooks": type(sys.get_asyncgen_hooks()),
        "time.struct_time": time.struct_time,
        "os.terminal_size": os.terminal_size,
    }
    # CP publishes the type even where the uname function is unavailable.
    types[f"{os.uname_result.__module__}.uname_result"] = os.uname_result
    if os.name == "posix":
        import pwd

        types["pwd.struct_passwd"] = pwd.struct_passwd
    # Independent CP field order; dir()/namespace iteration does not establish
    # the physical field order of a native struct sequence.
    fields_by_name = {
        "sys.version_info": ("major", "minor", "micro", "releaselevel", "serial"),
        "sys.float_info": (
            "max",
            "max_exp",
            "max_10_exp",
            "min",
            "min_exp",
            "min_10_exp",
            "dig",
            "mant_dig",
            "epsilon",
            "radix",
            "rounds",
        ),
        "sys.int_info": (
            "bits_per_digit",
            "sizeof_digit",
            "default_max_str_digits",
            "str_digits_check_threshold",
        ),
        "sys.hash_info": (
            "width",
            "modulus",
            "inf",
            "nan",
            "imag",
            "algorithm",
            "hash_bits",
            "seed_bits",
            "cutoff",
        ),
        "sys.thread_info": ("name", "lock", "version"),
        "asyncgen_hooks": ("firstiter", "finalizer"),
        "time.struct_time": (
            "tm_year",
            "tm_mon",
            "tm_mday",
            "tm_hour",
            "tm_min",
            "tm_sec",
            "tm_wday",
            "tm_yday",
            "tm_isdst",
            "tm_zone",
            "tm_gmtoff",
        ),
        "os.terminal_size": ("columns", "lines"),
        "posix.uname_result": _UNAME_RESULT_FIELDS,
        "nt.uname_result": _UNAME_RESULT_FIELDS,
        "sys.flags": (
            "debug",
            "inspect",
            "interactive",
            "optimize",
            "dont_write_bytecode",
            "no_user_site",
            "no_site",
            "ignore_environment",
            "verbose",
            "bytes_warning",
            "quiet",
            "hash_randomization",
            "isolated",
            "dev_mode",
            "utf8_mode",
            "warn_default_encoding",
            "safe_path",
            "int_max_str_digits",
        )
        + tuple(
            name
            for name in (
                "gil",
                "thread_inherit_context",
                "context_aware_warnings",
                "lazy_imports",
            )
            if hasattr(sys.flags, name)
        ),
    }
    if os.name == "posix":
        fields_by_name["pwd.struct_passwd"] = (
            "pw_name",
            "pw_passwd",
            "pw_uid",
            "pw_gid",
            "pw_gecos",
            "pw_dir",
            "pw_shell",
        )
    new = ctypes.PYFUNCTYPE(ctypes.c_void_p, ctypes.py_object)(
        ("PyStructSequence_New", ctypes.pythonapi)
    )
    set_item = ctypes.PYFUNCTYPE(
        None, ctypes.c_void_p, ctypes.c_ssize_t, ctypes.py_object
    )(("PyStructSequence_SetItem", ctypes.pythonapi))
    incref = ctypes.PYFUNCTYPE(None, ctypes.py_object)(("Py_IncRef", ctypes.pythonapi))
    decref = ctypes.PYFUNCTYPE(None, ctypes.c_void_p)(("Py_DecRef", ctypes.pythonapi))

    def publish(
        fullname, module, fields, docs, visible, constructor, static_origin, doc
    ):
        if fullname not in types and fullname in {
            "posix.uname_result",
            "nt.uname_result",
        }:
            assert module == fullname.rsplit(".", 1)[0]
            types[fullname] = _uname_result_type(module)
        if fullname not in types:
            raise AssertionError(f"unprovided native struct sequence: {fullname}")
        cls = types[fullname]
        expected_module = (
            "builtins" if fullname == "asyncgen_hooks" else fullname.rsplit(".", 1)[0]
        )
        expected_role = (
            1
            if fullname in {"sys.flags", "sys.version_info"}
            else 3
            if fullname == "time.struct_time"
            else 0
        )
        assert module == expected_module
        assert fields == fields_by_name[fullname]
        assert type(fields) is tuple and type(docs) is tuple
        assert len(fields) == len(docs) == cls.n_fields
        assert visible == cls.n_sequence_fields
        assert sum(name is None for name in fields) == cls.n_unnamed_fields
        assert constructor == expected_role
        assert type(static_origin) is bool
        assert static_origin == (not bool(cls.__flags__ & (1 << 9)))
        assert all(value is None or type(value) is str for value in docs)
        assert doc is None or type(doc) is str
        return cls

    def populate(cls, values):
        if cls is not os.stat_result and not any(
            cls is candidate for candidate in types.values()
        ):
            raise AssertionError("unprovided native struct sequence type")
        assert type(values) is tuple and len(values) == cls.n_fields
        pointer = new(cls)
        if not pointer:
            raise MemoryError("PyStructSequence_New returned NULL")
        try:
            for index, value in enumerate(values):
                incref(value)  # SetItem steals this new reference.
                set_item(pointer, index, value)
            return ctypes.cast(pointer, ctypes.py_object).value
        finally:
            decref(pointer)  # The returned Python value owns its own reference.

    return {
        "molt_structseq_type": publish,
        "molt_structseq_new": populate,
        "molt_os_stat_result_type": lambda: os.stat_result,
    }


def struct_sequence_values(value: tuple) -> tuple:
    """Project a real CPython struct-sequence through its public physical API.

    A source-loader fixture returns the complete native result protocol, including
    fields beyond len(value); tuple(value) silently loses those fields.
    """
    cls = type(value)
    assert cls.__base__ is tuple and type(cls.n_fields) is int
    assert cls.n_sequence_fields == len(value)
    get_item = ctypes.PYFUNCTYPE(ctypes.c_void_p, ctypes.py_object, ctypes.c_ssize_t)(
        ("PyStructSequence_GetItem", ctypes.pythonapi)
    )
    # GetItem is borrowed. A py_object function result would consume a reference;
    # casting the raw address creates the Python owner before the record retires.
    return tuple(
        ctypes.cast(get_item(value, index), ctypes.py_object).value
        for index in range(cls.n_fields)
    )


def _publish_struct_sequence_helpers(behavior: Mapping[str, object]) -> None:
    names = ("molt_structseq_type", "molt_structseq_new")
    supplied = tuple(name in behavior for name in names)
    if not any(supplied):
        return
    if not all(supplied):
        raise ValueError(
            "struct sequence factory and value producer must be supplied together"
        )
    factory, new = (behavior[name] for name in names)
    if not callable(factory) or not callable(new):
        raise TypeError("struct sequence providers must be callable")

    def publish(
        name,
        fields,
        *,
        module="sys",
        visible=None,
        constructor=0,
        static_origin=True,
        docs=None,
        doc=None,
    ):
        fullname = name if module == "builtins" else module + "." + name
        return factory(
            fullname,
            module,
            fields,
            (None,) * len(fields) if docs is None else docs,
            len(fields) if visible is None else visible,
            constructor,
            static_origin,
            doc,
        )

    sys._struct_sequence_type = publish
    sys._struct_sequence_new = new


def install_registry(
    behavior: Mapping[str, object] = {},  # noqa: B006 - read only
    *,
    with_capabilities: bool = True,
) -> ModuleType:
    """Install the resolver and a strict registry for the rest of this process.

    ``with_capabilities=False`` leaves out the capability checks, for a test that
    proves what a module does without them. Returns the installed resolver module.
    """

    _publish_struct_sequence_helpers(behavior)
    resolver = _load_resolver()
    sys.modules["_intrinsics"] = resolver
    builtins._molt_intrinsics = {
        **(capability_grants() if with_capabilities else {}),
        **behavior,
    }  # type: ignore[attr-defined]
    builtins._molt_intrinsics_strict = True  # type: ignore[attr-defined]
    return resolver


@contextmanager
def intrinsic_registry(
    behavior: Mapping[str, object] = {},  # noqa: B006 - read only
    *,
    with_capabilities: bool = True,
) -> Iterator[ModuleType]:
    """:func:`install_registry` for one in-process block, then restore."""

    missing = object()
    saved_module = sys.modules.get("_intrinsics", missing)
    saved = {name: getattr(builtins, name, missing) for name in _REGISTRY_BUILTINS}
    saved_sys = {
        name: getattr(sys, name, missing)
        for name in ("_struct_sequence_type", "_struct_sequence_new")
    }
    try:
        yield install_registry(behavior, with_capabilities=with_capabilities)
    finally:
        if saved_module is missing:
            sys.modules.pop("_intrinsics", None)
        else:
            sys.modules["_intrinsics"] = saved_module  # type: ignore[assignment]
        for name, value in saved.items():
            if value is missing:
                if hasattr(builtins, name):
                    delattr(builtins, name)
            else:
                setattr(builtins, name, value)
        for name, value in saved_sys.items():
            if value is missing:
                if hasattr(sys, name):
                    delattr(sys, name)
            else:
                setattr(sys, name, value)
