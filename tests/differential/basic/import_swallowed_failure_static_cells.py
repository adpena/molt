"""Purpose: a swallowed import failure re-runs the body from a fresh namespace.

The first import of `flaky` binds some globals and raises; the handler
swallows it, and CPython removes the module, so the next import runs the body
again in a new namespace where `runs` is unbound. Static cells kept from the
first run would let the second body read `runs == 1` (M11). A handler that can
swallow a module's failure keeps that module's dictionary namespace.
"""

from import_redirect_support import control

control.fail_next = True
try:
    import import_redirect_support.flaky as flaky
except RuntimeError as exc:
    print("failed:", exc)
import import_redirect_support.flaky as flaky  # noqa: E402

print(flaky.runs, flaky.get())
