# MOLT_META: stderr=exact
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

# Gzip archives retain their own header policy while sharing the fallible
# compression engine with the public zlib stream.
gzip_path = os.path.join(tmpdir, "test.tar.gz")
with tarfile.open(gzip_path, "w:gz") as tf:
    tf.add(src_path, arcname="hello.txt")
with open(gzip_path, "rb") as stream:
    header = stream.read(10)
print("gzip_header", header[:3] == b"\x1f\x8b\x08", header[9] == 255)
print("gzip_is_tarfile", tarfile.is_tarfile(gzip_path))
with tarfile.open(gzip_path, "r:gz") as tf:
    print("gzip_names", tf.getnames() == ["hello.txt"])
os.unlink(gzip_path)

# Clean up
os.unlink(src_path)
os.unlink(tar_path)
os.rmdir(tmpdir)
print("cleanup_done", True)
