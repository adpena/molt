"""Purpose: differential coverage for marshal versioning."""

import marshal

print(isinstance(marshal.version, int))
print(marshal.version >= 0)
