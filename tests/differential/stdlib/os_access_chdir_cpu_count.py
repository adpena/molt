"""Purpose: differential coverage for os.access, os.cpu_count, os.getpid,
os.getppid, os.devnull, os.F_OK/R_OK/W_OK/X_OK, os.umask, os.uname."""

import os

print("cpu_count:", type(os.cpu_count()).__name__)
print("cpu_count > 0:", os.cpu_count() > 0)
print("getpid:", type(os.getpid()).__name__)
print("getpid > 0:", os.getpid() > 0)
print("devnull:", os.devnull)
print("F_OK:", os.F_OK)
print("R_OK:", os.R_OK)
print("W_OK:", os.W_OK)
print("X_OK:", os.X_OK)
print("access /tmp:", os.access("/tmp", os.F_OK))
# umask: get and restore
old = os.umask(0o022)
os.umask(old)
print("umask round-trip ok")
print("uname type:", type(os.uname()).__name__)
print("uname sysname:", type(os.uname().sysname).__name__)
