"""Purpose: `_intrinsics.require_intrinsic` should work without relying on print/sys.stdout."""

import _intrinsics


resolve = _intrinsics.require_intrinsic
if not callable(resolve):
    raise RuntimeError("require_intrinsic not callable")

resolved = resolve("molt_sys_stdout", globals())
if not callable(resolved):
    raise RuntimeError("resolved intrinsic not callable")

stream = resolved()
if type(stream).__name__ != "TextIOWrapper":
    raise RuntimeError("resolved intrinsic returned wrong object")
