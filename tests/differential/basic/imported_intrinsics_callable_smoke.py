"""Purpose: imported intrinsic resolver aliases must remain callable."""

from _intrinsics import require_intrinsic as ri


print(callable(ri))
print(type(ri).__name__)
