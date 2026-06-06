"""Purpose: differential coverage for TypeError message + traceback parity when
an augmented-assignment operator has NO in-place dunder and NO binary dunder.

CPython raises TypeError with a specific "unsupported operand type(s)" message
keyed on the BINARY operator symbol (//, /, %, **, <<, >>, @) even for the
augmented form. molt must reproduce the message byte-for-byte and the exit code.
"""


class Empty:
    def __init__(self, name):
        self.name = name

    def __repr__(self):
        return "Empty(%r)" % self.name


def attempt(label, fn):
    try:
        fn()
    except TypeError as exc:
        print(label, "TypeError:", exc)
    except Exception as exc:  # pragma: no cover - any other class is a bug
        print(label, "UNEXPECTED:", type(exc).__name__, exc)
    else:
        print(label, "NO ERROR (bug)")


def floordiv():
    x = Empty("a")
    x //= Empty("b")


def truediv():
    x = Empty("a")
    x /= Empty("b")


def mod():
    x = Empty("a")
    x %= Empty("b")


def pow_():
    x = Empty("a")
    x **= Empty("b")


def lshift():
    x = Empty("a")
    x <<= Empty("b")


def rshift():
    x = Empty("a")
    x >>= Empty("b")


def matmul():
    x = Empty("a")
    x @= Empty("b")


attempt("floordiv", floordiv)
attempt("truediv", truediv)
attempt("mod", mod)
attempt("pow", pow_)
attempt("lshift", lshift)
attempt("rshift", rshift)
attempt("matmul", matmul)
print("done")

# NOTE: the augmented operator symbol (`//=`, `**=`, ...) in the message is the
# fix verified here. We deliberately catch every case rather than let one raise
# uncaught: molt's uncaught-exception traceback does not yet render the
# `File/line/source/caret` frame for module-level expression raises (a
# pre-existing, operator-agnostic caret-annotation gap that affects binary `+`
# identically — see memory/project_caret_annotation_status.md), so an uncaught
# raise here would diff on that unrelated gap rather than on this task's fix.
