"""Purpose: differential coverage for oserror errno."""

import errno


e = OSError(errno.EEXIST, "File already exists", "foo.txt")
print(type(e).__name__)
print(e.errno, e.args[0])
print(e.strerror)
print(e.filename)

e2 = OSError(errno.ENOENT, "Missing")
print(type(e2).__name__)
print(e2.errno)

print(errno.errorcode[errno.EEXIST])
