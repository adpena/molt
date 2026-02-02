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
