"""Purpose: differential coverage for os.dup2, os.lseek, os.isatty,
os.ftruncate, os.SEEK_SET/SEEK_CUR/SEEK_END."""

import os
import tempfile

print("SEEK_SET:", os.SEEK_SET)
print("SEEK_CUR:", os.SEEK_CUR)
print("SEEK_END:", os.SEEK_END)
tmpdir = tempfile.mkdtemp()
path = os.path.join(tmpdir, "testfile")
fd = os.open(path, os.O_CREAT | os.O_RDWR, 0o644)
os.write(fd, b"hello world")
print("lseek to 0:", os.lseek(fd, 0, os.SEEK_SET))
data = os.read(fd, 5)
print("read after lseek:", data)
print("isatty fd:", os.isatty(fd))
print("isatty stdin:", os.isatty(0))
os.ftruncate(fd, 5)
os.lseek(fd, 0, os.SEEK_SET)
print("after truncate:", os.read(fd, 100))
os.close(fd)
os.unlink(path)
os.rmdir(tmpdir)
print("dup2_lseek_isatty ok")
