"""Purpose: differential coverage for uuid basics."""

import uuid

u = uuid.UUID("12345678-1234-5678-1234-567812345678")
print(u.version, u.hex)

u2 = uuid.UUID(int=0)
print(u2.hex)

u3 = uuid.UUID(bytes=b"\x12" * 16)
print(u3.hex)
