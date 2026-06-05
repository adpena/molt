"""Purpose: differential regression for the LLVM exception-CFG fix.

A handler block reachable only via a mid-block ``CheckException`` edge must be
lowered (Class B): a raise inside an ``except`` block must transfer control to
the enclosing handler. Before the fix, the LLVM lowering's reverse-post-order
driver walked terminator successors only, so the handler block never entered
the RPO, was stamped with a bare ``unreachable``, and SimplifyCFG folded the
``CheckException`` branch into an ``llvm.assume`` that skipped the raise — the
program ran past the handler with the exception still pending and exited with an
unhandled traceback instead of catching ``RuntimeError``.

This also exercises the Class-A handler-rejoin path: ``label`` is computed in the
protected region and read again after the try/except merge, which requires the
SSA construction to place a phi at the post-handler join so the value is
dominance-correct on both the normal and exceptional paths.
"""

label = "start"
try:
    try:
        label = "inner-raised"
        raise ValueError("inner")
    except Exception:
        label = "outer-raised"
        raise RuntimeError("outer")
except Exception as exc:
    print("caught", type(exc).__name__)

# `label` is defined in the protected region and read past the try/except
# merge — the handler-rejoin value that must flow through a phi.
print("label", label)


def caught_type() -> str:
    try:
        try:
            raise KeyError("k")
        except KeyError:
            raise IndexError("i")
    except LookupError as exc:
        return type(exc).__name__
    return "none"


print("fn", caught_type())
