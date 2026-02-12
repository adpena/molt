"""Purpose: differential coverage for uuid basics."""

# MOLT_ENV: MOLT_CAPABILITIES=time.wall

import uuid

u = uuid.UUID("12345678-1234-5678-1234-567812345678")
print(u.version, u.variant, u.hex, u.bytes_le.hex(), u.fields)

u2 = uuid.UUID(int=0)
print(u2.version, u2.variant, u2.hex, u2.bytes_le.hex(), u2.fields)

u3 = uuid.UUID(bytes=b"\x12" * 16)
print(u3.version, u3.variant, u3.hex, u3.bytes_le.hex(), u3.fields)

print(uuid.NAMESPACE_DNS)
print(uuid.NAMESPACE_URL)

u4 = uuid.uuid3(uuid.NAMESPACE_DNS, "python.org")
print(u4.version, u4.variant, u4.hex)

u5 = uuid.uuid5(uuid.NAMESPACE_DNS, "python.org")
print(u5.version, u5.variant, u5.hex)

u6 = uuid.uuid4()
print(u6.version, u6.variant)

u7 = uuid.uuid1()
print(u7.version, u7.variant)
