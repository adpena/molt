"""Purpose: differential regression for task #66.

A try/except INSIDE a function body (or ``__init__``) whose try body
*unconditionally raises* and whose ``except`` handler re-raises a DIFFERENT
exception silently dropped the handler: the original exception leaked past the
``except`` clause instead of being caught and replaced (P0 silent wrong
exception flow, observed on native and presumed on every backend).

Root cause was a three-layer label/edge-reachability gap. molt lowers ``raise``
to "set the pending-exception flag and CONTINUE" (the backend ``raise`` op calls
``molt_raise`` and falls through — it does NOT branch); the frontend routes to
the handler with the ``CHECK_EXCEPTION <handler>; JUMP <handler>`` it emits
immediately after the raise. But the CFG modelled ``raise`` as a hard terminator
(no successors), so when a try body's sole statement was ``raise`` that routing
became unreachable and SCCP/DCE pruned it, leaving the handler reachable only via
the implicit ``try_start`` exception edge — which the dead-label pruners
(frontend AND the TIR ``eliminate_dead_labels``) did not count as a label
reference, so they stripped the handler-entry label and orphaned the whole
``except`` clause.

The fix models ``raise`` as a fall-through edge (mirroring the real lowering) and
makes every label-reference enumeration count ``try_start``'s handler-edge label.
A try body that always raises is exactly WHEN the handler is needed.

Covers: simple-signature functions, full-binding (``**kwargs``) functions and
``__init__`` (the slow argument binder), catch-and-return, catch-specific-miss
(propagation), nested try, try/finally, bare ``raise``, and ``__context__`` /
``__cause__`` identity.
"""


def reraise_simple():
    try:
        raise KeyError("k")
    except KeyError:
        raise ValueError("v")


def reraise_fullbind(**kw):
    try:
        raise KeyError("k")
    except KeyError:
        raise ValueError("v")


def catch_specific_miss():
    try:
        raise KeyError("k")
    except TypeError:
        return "WRONG"


def catch_and_return(*args):
    try:
        raise KeyError("k")
    except KeyError:
        return "caught"


def nested_reraise():
    try:
        try:
            raise KeyError("inner")
        except KeyError:
            raise IndexError("mid")
    except IndexError:
        raise ValueError("outer")


def try_finally_reraise():
    try:
        try:
            raise KeyError("k")
        finally:
            print("finally ran")
    except KeyError:
        raise ValueError("v")


def bare_reraise():
    try:
        raise KeyError("k")
    except KeyError:
        raise


class Variadic:
    def __init__(self, **kw):
        try:
            raise KeyError("k")
        except KeyError:
            raise ValueError("init-v")


class VariadicMiss:
    def __init__(self, *args):
        try:
            raise KeyError("k")
        except ValueError:
            print("WRONG")


def chained():
    try:
        raise KeyError("k")
    except KeyError as e:
        raise ValueError("v") from e


def ctx_name(exc):
    c = exc.__context__
    return type(c).__name__ if c is not None else "None"


def cause_name(exc):
    c = exc.__cause__
    return type(c).__name__ if c is not None else "None"


# m01 simple-signature reraise -> ValueError with __context__ = KeyError
try:
    reraise_simple()
except ValueError as e:
    print("simple:", e, "ctx", ctx_name(e))
except KeyError as e:
    print("simple LEAK:", e)

# m02 full-binding reraise
try:
    reraise_fullbind()
except ValueError as e:
    print("fullbind:", e, "ctx", ctx_name(e))
except KeyError as e:
    print("fullbind LEAK:", e)

# m03 __init__ reraise
try:
    Variadic()
except ValueError as e:
    print("init:", e, "ctx", ctx_name(e))
except KeyError as e:
    print("init LEAK:", e)

# m04 catch-specific-miss propagates the original KeyError
try:
    catch_specific_miss()
except KeyError as e:
    print("miss propagated:", e)
except TypeError as e:
    print("miss WRONG:", e)

# m05 catch-and-return (no re-raise)
print("return:", catch_and_return())

# m06 nested reraise: ValueError <- IndexError <- KeyError
try:
    nested_reraise()
except ValueError as e:
    print("nested:", e, "ctx", ctx_name(e), "ctx2", ctx_name(e.__context__))

# m07 try/finally then reraise
try:
    try_finally_reraise()
except ValueError as e:
    print("finally:", e, "ctx", ctx_name(e))

# m08 bare reraise re-raises the same exception
try:
    bare_reraise()
except KeyError as e:
    print("bare:", e)

# m09 __init__ specific-miss propagates KeyError
try:
    VariadicMiss()
except KeyError as e:
    print("init miss propagated:", e)

# m10 explicit cause chain (raise ... from)
try:
    chained()
except ValueError as e:
    print("chained:", e, "cause", cause_name(e), "ctx", ctx_name(e))

# m11 handler-caught loop must not leak across many iterations (10k)
caught = 0
for _ in range(10000):
    try:
        try:
            raise KeyError("k")
        except KeyError:
            raise ValueError("v")
    except ValueError:
        caught += 1
print("loop caught:", caught)
