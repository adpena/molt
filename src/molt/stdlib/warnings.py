"""Python part of the warnings subsystem (CPython 3.12 ``Lib/warnings.py``).

The runtime's warnings engine is CPython's ``_warnings``: it owns the filter
list, the once registry, the default action and the filters version, and it
runs ``warn`` and ``warn_explicit``. ``filters`` and ``onceregistry`` here are
the engine's own objects. While this module is loaded the engine reads its
``filters``, ``defaultaction`` and ``onceregistry`` attributes and displays
through ``_showwarnmsg``, so ``catch_warnings`` and replaced ``showwarning`` or
``formatwarning`` hooks apply to every warning, including the runtime's own.
"""

import sys

from _intrinsics import require_intrinsic as _require_intrinsic


__all__ = [
    "warn",
    "warn_explicit",
    "showwarning",
    "formatwarning",
    "filterwarnings",
    "simplefilter",
    "resetwarnings",
    "catch_warnings",
    "deprecated",
]

_MOLT_WARNINGS_WARN = _require_intrinsic("molt_warnings_warn")
_MOLT_WARNINGS_WARN_EXPLICIT = _require_intrinsic("molt_warnings_warn_explicit")
_MOLT_WARNINGS_FORMATWARNING = _require_intrinsic("molt_warnings_formatwarning")
_MOLT_WARNINGS_SIMPLEFILTER = _require_intrinsic("molt_warnings_simplefilter")
_MOLT_WARNINGS_FILTERWARNINGS = _require_intrinsic("molt_warnings_filterwarnings")
_MOLT_WARNINGS_RESETWARNINGS = _require_intrinsic("molt_warnings_resetwarnings")


def showwarning(message, category, filename, lineno, file=None, line=None):
    """Hook to write a warning to a file; replace if you like."""
    msg = WarningMessage(message, category, filename, lineno, file, line)
    _showwarnmsg_impl(msg)


def formatwarning(message, category, filename, lineno, line=None):
    """Function to format a warning the standard way."""
    msg = WarningMessage(message, category, filename, lineno, None, line)
    return _formatwarnmsg_impl(msg)


def _showwarnmsg_impl(msg):
    file = msg.file
    if file is None:
        file = sys.stderr
        if file is None:
            # sys.stderr is None when run with pythonw.exe:
            # warnings get lost
            return
    text = _formatwarnmsg(msg)
    try:
        file.write(text)
    except OSError:
        # the file (probably stderr) is invalid - this warning gets lost.
        pass


def _formatwarnmsg_impl(msg):
    # The engine formats as CPython does and reads a missing source line
    # through the compiled-source reader the tracebacks use.
    return _MOLT_WARNINGS_FORMATWARNING(
        msg.message, msg.category, msg.filename, msg.lineno, msg.line
    )


# Keep a reference to check if the function was replaced
_showwarning_orig = showwarning


def _showwarnmsg(msg):
    """Hook to write a warning to a file; replace if you like."""
    try:
        sw = showwarning
    except NameError:
        pass
    else:
        if sw is not _showwarning_orig:
            # warnings.showwarning() was replaced
            if not callable(sw):
                raise TypeError(
                    "warnings.showwarning() must be set to a function or method"
                )

            sw(msg.message, msg.category, msg.filename, msg.lineno, msg.file, msg.line)
            return
    _showwarnmsg_impl(msg)


# Keep a reference to check if the function was replaced
_formatwarning_orig = formatwarning


def _formatwarnmsg(msg):
    """Function to format a warning the standard way."""
    try:
        fw = formatwarning
    except NameError:
        pass
    else:
        if fw is not _formatwarning_orig:
            # warnings.formatwarning() was replaced
            return fw(msg.message, msg.category, msg.filename, msg.lineno, msg.line)
    return _formatwarnmsg_impl(msg)


def filterwarnings(
    action, message="", category=Warning, module="", lineno=0, append=False
):
    """Insert an entry into the list of warnings filters (at the front).

    'action' -- one of "error", "ignore", "always", "default", "module",
                or "once"
    'message' -- a regex that the warning message must match
    'category' -- a class that the warning must be a subclass of
    'module' -- a regex that the module name must match
    'lineno' -- an integer line number, 0 matches all warnings
    'append' -- if true, append to the list of filters
    """
    assert action in ("error", "ignore", "always", "default", "module", "once"), (
        "invalid action: %r" % (action,)
    )
    assert isinstance(message, str), "message must be a string"
    assert isinstance(category, type), "category must be a class"
    assert issubclass(category, Warning), "category must be a Warning subclass"
    assert isinstance(module, str), "module must be a string"
    assert isinstance(lineno, int) and lineno >= 0, "lineno must be an int >= 0"

    if message or module:
        # A local import keeps `re` out of the static reach of programs that
        # never install a pattern filter.
        import re

    if message:
        message = re.compile(message, re.I)
    else:
        message = None
    if module:
        module = re.compile(module)
    else:
        module = None

    _MOLT_WARNINGS_FILTERWARNINGS(action, message, category, module, lineno, append)


def simplefilter(action, category=Warning, lineno=0, append=False):
    """Insert a simple entry into the list of warnings filters (at the front).

    A simple filter matches all modules and messages.
    'action' -- one of "error", "ignore", "always", "default", "module",
                or "once"
    'category' -- a class that the warning must be a subclass of
    'lineno' -- an integer line number, 0 matches all warnings
    'append' -- if true, append to the list of filters
    """
    assert action in ("error", "ignore", "always", "default", "module", "once"), (
        "invalid action: %r" % (action,)
    )
    assert isinstance(lineno, int) and lineno >= 0, "lineno must be an int >= 0"
    _MOLT_WARNINGS_SIMPLEFILTER(action, category, lineno, append)


def resetwarnings():
    """Clear the list of warning filters, so that no filters are active."""
    _MOLT_WARNINGS_RESETWARNINGS()


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


class WarningMessage(object):
    _WARNING_DETAILS = (
        "message",
        "category",
        "filename",
        "lineno",
        "file",
        "line",
        "source",
    )

    def __init__(
        self, message, category, filename, lineno, file=None, line=None, source=None
    ):
        self.message = message
        self.category = category
        self.filename = filename
        self.lineno = lineno
        self.file = file
        self.line = line
        self.source = source
        self._category_name = category.__name__ if category else None

    def __str__(self):
        return (
            "{message : %r, category : %r, filename : %r, lineno : %s, "
            "line : %r}"
            % (
                self.message,
                self._category_name,
                self.filename,
                self.lineno,
                self.line,
            )
        )


class catch_warnings(object):
    """A context manager that copies and restores the warnings filter upon
    exiting the context.

    The 'record' argument specifies whether warnings should be captured by a
    custom implementation of warnings.showwarning() and be appended to a list
    returned by the context manager. Otherwise None is returned by the context
    manager. The objects appended to the list are arguments whose attributes
    mirror the arguments to showwarning().

    The 'module' argument is to specify an alternative module to the module
    named 'warnings' and imported under that name. This argument is only useful
    when testing the warnings module itself.

    If the 'action' argument is not None, the remaining arguments are passed
    to warnings.simplefilter() as if it were called immediately on entering the
    context.
    """

    def __init__(
        self,
        *,
        record=False,
        module=None,
        action=None,
        category=Warning,
        lineno=0,
        append=False,
    ):
        """Specify whether to record warnings and if an alternative module
        should be used other than sys.modules['warnings'].

        For compatibility with Python 3.0, please consider all arguments to be
        keyword-only.

        """
        self._record = record
        self._module = sys.modules["warnings"] if module is None else module
        self._entered = False
        if action is None:
            self._filter = None
        else:
            self._filter = (action, category, lineno, append)

    def __repr__(self):
        args = []
        if self._record:
            args.append("record=True")
        if self._module is not sys.modules["warnings"]:
            args.append("module=%r" % self._module)
        name = type(self).__name__
        return "%s(%s)" % (name, ", ".join(args))

    def __enter__(self):
        if self._entered:
            raise RuntimeError("Cannot enter %r twice" % self)
        self._entered = True
        self._filters = self._module.filters
        self._module.filters = self._filters[:]
        self._module._filters_mutated()
        self._showwarning = self._module.showwarning
        self._showwarnmsg_impl = self._module._showwarnmsg_impl
        if self._filter is not None:
            simplefilter(*self._filter)
        if self._record:
            log = []
            self._module._showwarnmsg_impl = log.append
            # Reset showwarning() to the default implementation to make sure
            # that _showwarnmsg() calls _showwarnmsg_impl()
            self._module.showwarning = self._module._showwarning_orig
            return log
        else:
            return None

    def __exit__(self, *exc_info):
        if not self._entered:
            raise RuntimeError("Cannot exit %r without entering first" % self)
        self._module.filters = self._filters
        self._module._filters_mutated()
        self._module.showwarning = self._showwarning
        self._module._showwarnmsg_impl = self._showwarnmsg_impl


class deprecated:
    """Indicate that a class, function or overload is deprecated.

    PEP 702 -- added in Python 3.13.

    Usage::

        @deprecated("Use new_func instead")
        def old_func():
            ...

        @deprecated("Use NewClass instead")
        class OldClass:
            ...
    """

    def __init__(
        self,
        message: str,
        /,
        *,
        category: type = DeprecationWarning,
        stacklevel: int = 1,
    ):
        if not isinstance(message, str):
            raise TypeError("expected a string as the first argument")
        self.message = message
        self.category = category
        self.stacklevel = stacklevel

    def __call__(self, arg):
        if isinstance(arg, type):
            original_init = arg.__init__

            def new_init(self_inner, *args, **kwargs):
                warn(
                    self.message,
                    category=self.category,
                    stacklevel=self.stacklevel + 1,
                )
                original_init(self_inner, *args, **kwargs)

            new_init.__name__ = "__init__"
            new_init.__qualname__ = f"{arg.__qualname__}.__init__"
            arg.__init__ = new_init  # type: ignore[assignment]
            arg.__deprecated__ = self.message  # type: ignore[attr-defined]
            return arg
        else:

            def wrapper(*args, **kwargs):
                warn(
                    self.message,
                    category=self.category,
                    stacklevel=self.stacklevel + 1,
                )
                return arg(*args, **kwargs)

            wrapper.__name__ = getattr(arg, "__name__", wrapper.__name__)
            wrapper.__qualname__ = getattr(arg, "__qualname__", wrapper.__qualname__)
            wrapper.__doc__ = getattr(arg, "__doc__", None)
            wrapper.__wrapped__ = arg  # type: ignore[attr-defined]
            wrapper.__deprecated__ = self.message  # type: ignore[attr-defined]
            return wrapper


_DEPRECATED_MSG = "{name!r} is deprecated and slated for removal in Python {remove}"


def _deprecated(name, message=_DEPRECATED_MSG, *, remove, _version=sys.version_info):
    """Warn that *name* is deprecated or should be removed.

    RuntimeError is raised if *remove* specifies a major/minor tuple older than
    the current Python version or the same version but past the alpha.

    The *message* argument is formatted with *name* and *remove* as a Python
    version (e.g. "3.11").

    """
    remove_formatted = f"{remove[0]}.{remove[1]}"
    if (_version[:2] > remove) or (_version[:2] == remove and _version[3] != "alpha"):
        msg = f"{name!r} was slated for removal after Python {remove_formatted} alpha"
        raise RuntimeError(msg)
    else:
        msg = message.format(name=name, remove=remove_formatted)
        warn(msg, DeprecationWarning, stacklevel=3)


# filters contains a sequence of filter 5-tuples
# The components of the 5-tuple are:
# - an action: error, ignore, always, default, module, or once
# - a compiled regex that must match the warning message
# - a class representing the warning category
# - a compiled regex that must match the module that is being warned
# - a line number for the line being warning, or 0 to mean any line
# If either if the compiled regexs are None, match anything.
filters = _require_intrinsic("molt_warnings_filters_get")()
onceregistry = _require_intrinsic("molt_warnings_onceregistry_get")()
defaultaction = "default"
_filters_mutated = _require_intrinsic("molt_warnings_filters_mutated")
