# MOLT_META: backends=native,wasm
"""Purpose: `_warnings` is the warnings provider and `warnings` its consumer.

CPython 3.12 `Lib/warnings.py` imports `filters`, `_defaultaction`,
`_onceregistry`, `warn`, `warn_explicit` and `_filters_mutated` from
`_warnings` (`Python/_warnings.c`), which never imports `warnings`. Until
`warnings` is loaded the engine uses its own filters and writes to the
`sys.stderr` object.
"""

import sys

# Observe the provider alone: a harness or startup path may already have
# loaded the consumer (CPython's `importlib` package imports `warnings`).
sys.modules.pop("warnings", None)

import _warnings

print("warnings loaded by _warnings", "warnings" in sys.modules)
print(
    "provider surface",
    [
        name
        for name in (
            "filters",
            "_defaultaction",
            "_onceregistry",
            "_filters_mutated",
            "warn",
            "warn_explicit",
        )
        if hasattr(_warnings, name)
    ],
)
print(
    "provider state",
    type(_warnings.filters).__name__,
    type(_warnings._onceregistry).__name__,
    _warnings._defaultaction,
)
print(
    "default filters",
    [
        (action, category.__name__, module)
        for action, _message, category, module, _lineno in _warnings.filters
    ],
)


class Stderr:
    def __init__(self):
        self.lines = []

    def write(self, text):
        self.lines.append(text)


def displayed(function, *args):
    """The header lines `function` writes to a replaced sys.stderr."""
    capture = Stderr()
    saved = sys.stderr
    sys.stderr = capture
    try:
        function(*args)
    finally:
        sys.stderr = saved
    text = "".join(capture.lines)
    headers = [line for line in text.splitlines() if not line.startswith("  ")]
    return [header.rsplit("/", 1)[-1] for header in headers]


def warn_from_here():
    _warnings.warn("provider display", UserWarning)


print("display", displayed(warn_from_here))
print("warnings loaded by warn", "warnings" in sys.modules)

# An error filter on the provider's own list applies before `warnings` exists:
# the import protocol's empty `sys.meta_path` warning raises.
_warnings.filters.insert(0, ("error", None, ImportWarning, None, 0))
_warnings._filters_mutated()
saved_meta_path = sys.meta_path[:]
sys.meta_path[:] = []
try:
    __import__("_molt_provider_direction_target")
except ImportWarning as error:
    print("empty meta path", type(error).__name__, error)
except ImportError as error:
    print("empty meta path", "no warning", type(error).__name__)
finally:
    sys.meta_path[:] = saved_meta_path
    del _warnings.filters[0]
    _warnings._filters_mutated()
print("warnings loaded by import warning", "warnings" in sys.modules)

print(
    "explicit",
    displayed(_warnings.warn_explicit, "explicit", RuntimeWarning, "provider.py", 7),
)

import warnings

print(
    "consumer aliases",
    warnings.filters is _warnings.filters,
    warnings.defaultaction is _warnings._defaultaction,
    warnings.onceregistry is _warnings._onceregistry,
    warnings._filters_mutated is _warnings._filters_mutated,
    warnings.warn is _warnings.warn,
    warnings.warn_explicit is _warnings.warn_explicit,
)

with warnings.catch_warnings(record=True) as caught:
    warnings.simplefilter("always")
    _warnings.warn("through the consumer hooks", UserWarning)
print("recorded", [(entry.category.__name__, str(entry.message)) for entry in caught])
print("filters restored", warnings.filters is _warnings.filters)
