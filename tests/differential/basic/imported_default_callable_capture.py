"""Purpose: imported default callables should preserve the captured callable."""

from _intrinsics import require_intrinsic as ri


def call_default(name, fn=ri):
    intrinsic = fn(name, globals())
    print(callable(fn))
    print(callable(intrinsic))
    return intrinsic


resolved = call_default("molt_sys_stdout")
print(type(resolved()).__name__)
