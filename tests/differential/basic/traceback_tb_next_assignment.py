"""A traceback's tb_next is writable, as in CPython 3.7+: it takes None or a
traceback that does not lead back to itself. The other fields are read-only.
unittest trims runner frames this way before it formats a failure."""

import sys
import traceback


def inner():
    raise AssertionError("boom")


def middle():
    inner()


def outer():
    middle()


def caught():
    try:
        outer()
    except AssertionError as exc:
        return exc


def names(tb):
    out = []
    while tb is not None:
        out.append(tb.tb_frame.f_code.co_name)
        tb = tb.tb_next
    return out


def attempt(label, action):
    try:
        action()
        print(label, "ok")
    except Exception as exc:
        print(label, type(exc).__name__, exc)


exc = caught()
tb = exc.__traceback__
print("full", names(tb))

second = tb.tb_next
attempt("cut after second", lambda: setattr(second, "tb_next", None))
print("cut", names(tb))

attempt("relink", lambda: setattr(second, "tb_next", caught().__traceback__.tb_next))
print("relinked", names(tb))

attempt("int", lambda: setattr(tb, "tb_next", 5))
attempt("self loop", lambda: setattr(tb, "tb_next", tb))
attempt("long loop", lambda: setattr(second.tb_next, "tb_next", tb))
attempt("delete", lambda: delattr(tb, "tb_next"))
for field in ("tb_frame", "tb_lasti", "tb_lineno"):
    attempt("set " + field, lambda field=field: setattr(tb, field, None))
attempt("other", lambda: setattr(tb, "other", 1))
print("after refusals", names(tb))

# unittest's shape: trim the traceback, then format it through
# TracebackException with CPython's constructor.
exc = caught()
trimmed = exc.__traceback__.tb_next
trimmed.tb_next.tb_next = None
formatted = traceback.TracebackException(
    type(exc), exc, trimmed, capture_locals=False, compact=True
)
lines = "".join(formatted.format()).splitlines()
print("formatted head", lines[0])
print("formatted frames", [line.split(", in ")[-1] for line in lines if ", in " in line])
print("formatted last", lines[-1])
print("only", list(formatted.format_exception_only()))

limited = traceback.TracebackException.from_exception(exc, limit=1)
print("limited", len(limited.stack), limited.exc_type.__name__)
try:
    try:
        raise KeyError("first")
    except KeyError:
        raise ValueError("second")
except ValueError as chained:
    tbe = traceback.TracebackException(type(chained), chained, chained.__traceback__)
    print("context", tbe.__context__.exc_type.__name__, tbe.__suppress_context__)
    text = "".join(tbe.format())
    print("chain", "During handling" in text, text.splitlines()[-1])
sys.stdout.flush()
