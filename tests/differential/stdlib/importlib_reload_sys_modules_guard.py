"""Purpose: differential coverage for importlib.reload sys.modules guard semantics."""

import importlib
import sys
import types


def _expect_error(label, fn, kind, message):
    try:
        fn()
    except Exception as exc:
        print(label, type(exc).__name__, str(exc))
        assert type(exc).__name__ == kind
        assert str(exc) == message
    else:
        raise AssertionError(f"{label} unexpectedly succeeded")


missing = types.ModuleType("molt_reload_guard_missing")
_expect_error(
    "missing-module",
    lambda: importlib.reload(missing),
    "ImportError",
    "module molt_reload_guard_missing not in sys.modules",
)


class NamedObject:
    __name__ = "molt_reload_named_not_module"


_expect_error(
    "named-object",
    lambda: importlib.reload(NamedObject()),
    "ImportError",
    "module molt_reload_named_not_module not in sys.modules",
)


class NonStringName:
    __name__ = 1


_expect_error(
    "non-string-name",
    lambda: importlib.reload(NonStringName()),
    "ImportError",
    "module 1 not in sys.modules",
)

_expect_error(
    "bad-arg",
    lambda: importlib.reload(1),
    "TypeError",
    "reload() argument must be a module",
)

math_mod = importlib.import_module("math")
removed = sys.modules.pop("math", None)
try:
    _expect_error(
        "popped-real-module",
        lambda: importlib.reload(math_mod),
        "ImportError",
        "module math not in sys.modules",
    )
finally:
    if removed is not None:
        sys.modules["math"] = removed
