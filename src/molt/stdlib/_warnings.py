"""CPython's `_warnings`: the warnings provider (`Python/_warnings.c`).

The runtime engine owns the filter list, the once registry, the default action
and the filters version, and runs `warn` and `warn_explicit`. This module
exposes them, and `warnings` imports them from here, as CPython's
`Lib/warnings.py` does. It never imports `warnings`: while that module is
loaded, the engine reads its `filters`, `defaultaction`, `onceregistry` and
`_showwarnmsg` instead (CPython `get_warnings_attr`).
"""

from _intrinsics import require_intrinsic as _require_intrinsic


_MOLT_WARNINGS_WARN = _require_intrinsic("molt_warnings_warn")
_MOLT_WARNINGS_WARN_EXPLICIT = _require_intrinsic("molt_warnings_warn_explicit")

filters = _require_intrinsic("molt_warnings_filters_get")()
_defaultaction = "default"
_onceregistry = _require_intrinsic("molt_warnings_onceregistry_get")()
_filters_mutated = _require_intrinsic("molt_warnings_filters_mutated")


def warn(message, category=None, stacklevel=1, source=None):
    """Issue a warning, or maybe ignore it or raise an exception."""
    # The engine attributes the warning as CPython's setup_context does,
    # starting below this function's own frame.
    _MOLT_WARNINGS_WARN(message, category, stacklevel)


# CPython's `warn_explicit` names the module after the file only when the
# caller omits `module`; an explicit None ignores the warning.
_MODULE_FROM_FILENAME = object()


def warn_explicit(
    message,
    category,
    filename,
    lineno,
    module=_MODULE_FROM_FILENAME,
    registry=None,
    module_globals=None,
    source=None,
):
    """Low-level interface to warnings functionality."""
    if module is _MODULE_FROM_FILENAME:
        module = filename
        if isinstance(filename, str):
            if not filename:
                module = "<unknown>"
            elif filename.endswith(".py"):
                module = filename[:-3]
    _MOLT_WARNINGS_WARN_EXPLICIT(
        message, category, filename, lineno, module, registry, module_globals
    )


del _require_intrinsic
