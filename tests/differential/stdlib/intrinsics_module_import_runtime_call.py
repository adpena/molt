"""Purpose: `_intrinsics` module imports must preserve wrapper call semantics."""

import _intrinsics


resolve = _intrinsics.require_intrinsic
print(callable(resolve))
resolved = resolve("molt_sys_stdout", globals())
print(callable(resolved))
print(type(resolved()).__name__)
