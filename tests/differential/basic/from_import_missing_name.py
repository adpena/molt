"""CPython parity: `from M import name` for a missing *name* raises ImportError.

CPython's IMPORT_FROM is not the same as a plain ``M.name`` attribute read: a
missing imported name raises ``ImportError("cannot import name 'name' from 'M'
(origin)")`` (after a ``sys.modules`` submodule-fallback used for circular
imports), NOT the ``AttributeError`` that ``M.name`` raises. molt previously
lowered ``from M import name`` through the generic module-attribute path and so
raised ``AttributeError`` — this is the bug.

The trailing ``(origin)`` suffix is environment-specific (``(unknown location)``
for a built-in module, an absolute ``.py`` path for a file module — and it
differs between any two installations / implementations), so this test asserts
the portable, byte-stable contract: the exception *type* (``ImportError``), the
message *prefix* (``cannot import name 'name' from 'module'``), and that the
miss is catchable as ``ImportError``. A name that *does* exist must still bind.
"""

import sys
import types


def kind(fn):
    try:
        fn()
        return "NO-RAISE"
    except ModuleNotFoundError as e:
        # A missing *name* is ImportError, never the ModuleNotFoundError
        # subclass (which is reserved for a missing *module*).
        return ("ModuleNotFoundError", str(e).split(" (")[0])
    except ImportError as e:
        return ("ImportError", str(e).split(" (")[0])
    except AttributeError as e:
        return ("AttributeError", str(e))


def from_builtin_module_missing():
    from sys import this_name_does_not_exist_qwerty  # noqa: F401


def from_file_module_missing():
    from os import another_missing_name_zzz  # noqa: F401


def from_present_name():
    # A name that exists must still bind correctly.
    from sys import maxsize

    return maxsize


print(kind(from_builtin_module_missing))
print(kind(from_file_module_missing))
print("present_ok", from_present_name() > 0)
print("import_error_is_exception", issubclass(ImportError, Exception))

# The miss must be catchable through the ImportError base class.
try:
    from sys import yet_another_missing_abc  # noqa: F401
except ImportError as e:
    print("caught_as_ImportError", type(e).__name__)

# Authored metadata gives both engines identical inputs: no path normalization
# or handwritten expected diagnostic can hide a lost field or the wrong origin.
_ABSENT = object()
_FIELDS = (
    "__name__",
    "__file__",
    "__spec__",
    "__getattr__",
    "molt_absent_first",
    "molt_absent_second",
)
_original = {key: types.__dict__.get(key, _ABSENT) for key in _FIELDS}


def _first():
    from types import molt_absent_first

    return molt_absent_first


def _second():
    from types import molt_absent_second

    return molt_absent_second


def _observe(operation):
    try:
        value = operation()
    except BaseException as error:
        return (
            type(error).__name__,
            str(error),
            getattr(error, "name", None),
            getattr(error, "path", None),
            getattr(error, "name_from", None),
        )
    return ("value", value)


def _configure(**metadata):
    for key in _FIELDS:
        types.__dict__.pop(key, None)
    types.__name__ = "authored_import_receiver"
    types.__dict__.update(metadata)


class _Spec:
    has_location = True
    origin = "/molt-import-receiver/spec.py"
    _initializing = False


class _NamespaceSpec:
    has_location = False
    origin = None
    _initializing = False


class _NonStringOrigin(_Spec):
    origin = 17


class _OriginText(str):
    def __str__(self):
        return "rendered-origin"


class _RenderedOrigin(_Spec):
    origin = _OriginText("/molt-import-receiver/original-origin.py")


class _CircularSpec(_Spec):
    _initializing = True


class _CircularNamespace(_NamespaceSpec):
    _initializing = True


class _InitializationFailure(_Spec):
    @property
    def _initializing(self):
        raise ValueError("initialization metadata failed")


class _OriginFailure(_Spec):
    @property
    def origin(self):
        raise LookupError("origin metadata failed")


class _LocationTruthFailure:
    def __bool__(self):
        raise ValueError("location truth failed")


class _LocationFailure(_Spec):
    has_location = _LocationTruthFailure()


# The origin property deletes the module's original name reference. The
# already-selected name must remain the typed ImportError.name and diagnostic.
class _ReentrantSpec(_Spec):
    @property
    def origin(self):
        types.__name__ = "replaced_during_origin"
        return "/molt-import-receiver/reentrant.py"


class _NonModuleReceiver:
    __name__ = "authored_non_module"
    __spec__ = None
    # PyModule_GetFilenameObject ignores this field on a non-module receiver.
    __file__ = "/molt-import-receiver/non-module.py"


class _NameFailureReceiver(_NonModuleReceiver):
    @property
    def __name__(self):
        raise ValueError("module name metadata failed")


try:
    for label, metadata in (
        (
            "file_vs_spec",
            {"__file__": "/molt-import-receiver/file.py", "__spec__": _Spec()},
        ),
        ("namespace", {"__spec__": _NamespaceSpec()}),
        (
            "ignored_non_string_origin",
            {
                "__file__": "/molt-import-receiver/file.py",
                "__spec__": _NonStringOrigin(),
            },
        ),
        (
            "origin_str_override",
            {
                "__file__": "/molt-import-receiver/file.py",
                "__spec__": _RenderedOrigin(),
            },
        ),
        (
            "surrogate_file",
            {"__file__": "/molt-import-receiver/path\ud800.py", "__spec__": None},
        ),
        ("spec_absent", {"__file__": "/molt-import-receiver/file.py"}),
        ("spec_none", {"__file__": "/molt-import-receiver/file.py", "__spec__": None}),
        ("non_string_file", {"__file__": 42, "__spec__": _NamespaceSpec()}),
        (
            "circular_file",
            {"__file__": "/molt-import-receiver/file.py", "__spec__": _CircularSpec()},
        ),
        ("circular_namespace", {"__spec__": _CircularNamespace()}),
        (
            "initialization_failure",
            {
                "__file__": "/molt-import-receiver/file.py",
                "__spec__": _InitializationFailure(),
            },
        ),
        (
            "origin_failure",
            {"__file__": "/molt-import-receiver/file.py", "__spec__": _OriginFailure()},
        ),
        ("location_failure", {"__spec__": _LocationFailure()}),
        (
            "reentrant_origin",
            {"__file__": "/molt-import-receiver/file.py", "__spec__": _ReentrantSpec()},
        ),
    ):
        for operation in (_first, _second):
            _configure(**metadata)
            print(label, operation.__name__, _observe(operation))

    for name in ("authored'quoted\\name", None, 42, _ABSENT):
        _configure(__spec__=None)
        if name is _ABSENT:
            del types.__name__
        else:
            types.__name__ = name
        print("module_name", _observe(_first))

    callback_error = ValueError("exact import callback")
    calls = []

    def _getattr(name):
        calls.append(name)
        if name == "molt_absent_first":
            raise callback_error
        raise AttributeError(name)

    _configure(__spec__=None, __getattr__=_getattr)
    try:
        _first()
    except ValueError as error:
        print("callback_identity", error is callback_error)
    print("callback_calls", calls)
    print("callback_attribute_error", _observe(_second))

    # IMPORT_FROM recovery returns exactly the child cache value; this differs
    # intentionally from IMPORT_NAME's None-in-sys.modules rejection.
    for cached in ("cached-child", None):
        _configure(__spec__=None)
        cache_key = "authored_import_receiver.molt_absent_first"
        prior = sys.modules.get(cache_key, _ABSENT)
        try:
            sys.modules[cache_key] = cached
            print("cached_child", _first() is cached)
        finally:
            if prior is _ABSENT:
                del sys.modules[cache_key]
            else:
                sys.modules[cache_key] = prior
    # An ordinary IMPORT_NAME cache hit may return a non-module. The same
    # IMPORT_FROM attribute/metadata owner must process it without a module-
    # only guard or a special test dispatcher.
    prior_types = sys.modules["types"]
    try:
        for receiver in (8, _NonModuleReceiver(), _NameFailureReceiver()):
            sys.modules["types"] = receiver
            print("non_module_cache", _observe(_first))
    finally:
        sys.modules["types"] = prior_types
finally:
    for key, value in _original.items():
        if value is _ABSENT:
            types.__dict__.pop(key, None)
        else:
            types.__dict__[key] = value
