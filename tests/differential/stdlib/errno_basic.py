"""Purpose: differential coverage for errno constants and errorcode mapping."""

import errno

constants = ["EACCES", "EEXIST", "EPIPE", "ECONNRESET", "EINTR", "EINVAL"]
values = []
for name in constants:
    values.append((name, getattr(errno, name)))

print(values)

# errorcode mapping should contain the constants
mapping = {errno.errorcode[val] for _, val in values}
print(sorted(mapping))

# These ordinary errno values are used by runtime failures beyond sockets.
# Check export and reverse-map coherence without borrowing host numeric values.
for name in (
    "EIO",
    "ENOMEM",
    "ENOSYS",
    "EPROTOTYPE",
    "EAFNOSUPPORT",
    "EISCONN",
    "ESHUTDOWN",
):
    value = getattr(errno, name)
    print(name, isinstance(value, int), getattr(errno, errno.errorcode[value]) == value)
print("wouldblock-alias", errno.EAGAIN == errno.EWOULDBLOCK)
print("separate-resolver-domain", hasattr(errno, "EAI_NONAME"))
