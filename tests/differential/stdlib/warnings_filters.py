# MOLT_META: backends=native,wasm
"""Purpose: warnings keep one filter list with CPython's defaults, matching,
catch_warnings save/restore, registry aging and display through sys.stderr."""

import _warnings
import contextlib
import io
import warnings

import warnings_location_support as support


def describe(filters):
    return [
        (action, message is None, category.__name__, module, lineno)
        for action, message, category, module, lineno in filters
    ]


def shown(function):
    """The warnings `function` displays, read from a redirected sys.stderr:
    the line, category and message of each header line."""
    buffer = io.StringIO()
    with contextlib.redirect_stderr(buffer):
        function()
    rows = []
    for text in buffer.getvalue().splitlines():
        if text.startswith("  "):
            continue  # the source line
        location, category, message = text.split(": ", 2)
        rows.append((int(location.rsplit(":", 1)[1]), category, message))
    return rows


def main_deprecated():
    warnings.warn("main deprecated", DeprecationWarning)


def main_pending():
    warnings.warn("main pending", PendingDeprecationWarning)


def main_import():
    warnings.warn("main import", ImportWarning)


def main_resource():
    warnings.warn("main resource", ResourceWarning)


def main_user():
    warnings.warn("main user", UserWarning)


print("defaults")
print(describe(warnings.filters))
print(warnings.defaultaction, type(warnings.onceregistry).__name__)
print(_warnings.filters is warnings.filters)
print(_warnings._onceregistry is warnings.onceregistry)
print(shown(main_deprecated))
print(shown(support.deprecated_here))
print(shown(main_pending), shown(main_import), shown(main_resource))
print(shown(main_user))


def inside_catch():
    warnings.warn("inside catch", UserWarning)


def after_catch():
    warnings.warn("after catch", UserWarning)


print("restore after catch_warnings")
outer = warnings.filters
outer_rows = describe(outer)
with warnings.catch_warnings():
    warnings.simplefilter("ignore")
    print(warnings.filters is outer, describe(warnings.filters[:1]))
    print(shown(inside_catch))
print(warnings.filters is outer, describe(warnings.filters) == outer_rows)
print(shown(after_catch))
with warnings.catch_warnings(action="error", category=RuntimeWarning):
    print(describe(warnings.filters[:1]))
print(describe(warnings.filters) == outer_rows)


def repeat():
    warnings.warn("repeat me", UserWarning)


print("catch_warnings re-admits suppressed warnings")
print(len(shown(repeat)), len(shown(repeat)))
with warnings.catch_warnings():
    pass
print(len(shown(repeat)), len(shown(repeat)))
warnings._filters_mutated()
print(len(shown(repeat)))

print("no duplicate filters")
with warnings.catch_warnings():
    warnings.resetwarnings()
    warnings.simplefilter("ignore", UserWarning)
    warnings.simplefilter("ignore", UserWarning)
    warnings.simplefilter("error", RuntimeWarning)
    warnings.simplefilter("ignore", UserWarning)
    warnings.simplefilter("error", RuntimeWarning, append=True)
    warnings.simplefilter("always", BytesWarning, append=True)
    warnings.simplefilter("always", BytesWarning, append=True)
    warnings.filterwarnings("error", category=SyntaxWarning, lineno=7)
    warnings.filterwarnings("error", category=SyntaxWarning, lineno=7)
    print(describe(warnings.filters))

print("message regex")
with warnings.catch_warnings(record=True) as log:
    warnings.simplefilter("always")
    warnings.filterwarnings("ignore", message="skip")
    warnings.warn("skip me")
    warnings.warn("SKIP upper case")
    warnings.warn("do not skip")
    warnings.filterwarnings("error", message=r"fail \d+$")
    try:
        warnings.warn("fail 42")
    except UserWarning as exc:
        print("raised", type(exc).__name__, exc)
    warnings.warn("fail 42 later")
print([str(record.message) for record in log])


class CustomWarning(UserWarning):
    pass


print("category subclass")
with warnings.catch_warnings(record=True) as log:
    warnings.simplefilter("always")
    warnings.simplefilter("ignore", UserWarning)
    warnings.warn("custom", CustomWarning)
    warnings.warn("runtime", RuntimeWarning)
    warnings.warn(CustomWarning("instance"))
    warnings.simplefilter("error", Warning)
    try:
        warnings.warn("custom error", CustomWarning)
    except CustomWarning as exc:
        print("raised", type(exc).__name__, exc)
    warnings.simplefilter("ignore", DeprecationWarning)
    try:
        warnings.warn("not deprecated", CustomWarning)
    except CustomWarning as exc:
        print("raised", type(exc).__name__, exc)
print([(record.category.__name__, str(record.message)) for record in log])


def once_site_a():
    warnings.warn("once text", UserWarning)


def once_site_b():
    warnings.warn("once text", UserWarning)


def registry_keys():
    return sorted(
        (len(key), key[0], key[1].__name__)
        for key in globals().get("__warningregistry__", {})
        if key != "version"
    )


print("once and module")
with warnings.catch_warnings(record=True) as log:
    warnings.simplefilter("once")
    once_site_a()
    once_site_b()
    print(len(log), registry_keys())
    warnings.warn_explicit("once explicit", UserWarning, "x.py", 1)
    warnings.warn_explicit("once explicit", UserWarning, "y.py", 2)
    print(len(log), sorted(key[0] for key in warnings.onceregistry if key != "version"))
with warnings.catch_warnings(record=True) as log:
    warnings.simplefilter("module")
    once_site_a()
    once_site_b()
    print(len(log), registry_keys())

print("warn_explicit")
with warnings.catch_warnings(record=True) as log:
    warnings.simplefilter("always")
    warnings.filterwarnings("ignore", module="ignored_by_name")
    warnings.warn_explicit("omitted module", UserWarning, "omitted.py", 3)
    warnings.warn_explicit("named after file", UserWarning, "ignored_by_name.py", 4)
    warnings.warn_explicit("explicit none", UserWarning, "none.py", 5, None)
    registry = {}
    warnings.warn_explicit("with registry", UserWarning, "reg.py", 6, "regmod", registry)
    warnings.warn_explicit(DeprecationWarning("instance"), UserWarning, "inst.py", 7)
    try:
        warnings.warn_explicit("bad registry", UserWarning, "bad.py", 8, "m", [])
    except TypeError as exc:
        print("TypeError", exc)
print([(r.filename, r.lineno, r.category.__name__, str(r.message)) for r in log])

print("hooks")
seen = []


def custom_show(message, category, filename, lineno, file=None, line=None):
    seen.append((category.__name__, str(message)))


def custom_format(message, category, filename, lineno, line=None):
    return "formatted %s: %s\n" % (category.__name__, message)


def hooked():
    warnings.warn("hooked", UserWarning)


warnings.showwarning = custom_show
with warnings.catch_warnings(record=True) as log:
    warnings.simplefilter("always")
    warnings.warn("recorded")
    print(warnings.showwarning is custom_show)
print([str(record.message) for record in log], seen)
print(warnings.showwarning is custom_show)
with warnings.catch_warnings():
    warnings.simplefilter("always")
    hooked()
print(seen)
warnings.showwarning = warnings._showwarning_orig
warnings.formatwarning = custom_format
buffer = io.StringIO()
with warnings.catch_warnings(), contextlib.redirect_stderr(buffer):
    warnings.simplefilter("always")
    hooked()
print(repr(buffer.getvalue()))
warnings.formatwarning = warnings._formatwarning_orig

print("policy objects")
with warnings.catch_warnings():
    warnings.filters = [("error", None, Warning, None, 0)]
    try:
        warnings.warn("rebound filters")
    except UserWarning as exc:
        print("raised", exc)
with warnings.catch_warnings():
    warnings.resetwarnings()
    warnings.defaultaction = "error"
    try:
        warnings.warn("default action")
    except UserWarning as exc:
        print("raised", exc)
    warnings.defaultaction = "default"
with warnings.catch_warnings():
    warnings.filters.insert(0, ("bogus", None, Warning, None, 0))
    try:
        warnings.warn("unknown action")
    except RuntimeError as exc:
        print("RuntimeError", exc)
try:
    warnings.simplefilter("bogus")
except AssertionError as exc:
    print("AssertionError", exc)
try:
    warnings.warn("bad category", 5)
except TypeError as exc:
    print("TypeError", exc)
print(describe(warnings.filters) == outer_rows)
