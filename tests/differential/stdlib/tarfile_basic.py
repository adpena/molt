"""Purpose: differential coverage for tarfile basic."""

import os
import tarfile
import tempfile

# is_tarfile on a non-tar file returns False
with tempfile.NamedTemporaryFile(suffix=".txt", delete=False) as f:
    f.write(b"this is not a tar file")
    tmpname = f.name

try:
    print("is_tarfile_false", tarfile.is_tarfile(tmpname) is False)
finally:
    os.unlink(tmpname)

# Create a tarball, add a file, then read it back
tmpdir = tempfile.mkdtemp()
src_path = os.path.join(tmpdir, "hello.txt")
with open(src_path, "w") as f:
    f.write("hello world")

tar_path = os.path.join(tmpdir, "test.tar")

# Write a tar archive
with tarfile.open(tar_path, "w") as tf:
    tf.add(src_path, arcname="hello.txt")

# Verify the tarball is recognized
print("is_tarfile_true", tarfile.is_tarfile(tar_path) is True)

# Read back and check names
with tarfile.open(tar_path, "r") as tf:
    names = tf.getnames()
    print("getnames_len", len(names) == 1)
    print("getnames_value", names[0] == "hello.txt")

# Clean up
os.unlink(src_path)
os.unlink(tar_path)
os.rmdir(tmpdir)
print("cleanup_done", True)
