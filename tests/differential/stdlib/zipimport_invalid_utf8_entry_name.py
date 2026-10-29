# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
# MOLT_META: expect_fail=molt expect_fail_reason=open_finding finding=HF-247 expect_molt_returncode=0 expect_molt_stdout_sha256=bbdba2396fa1982cd6d399a117bc16901e0e1dee068c5271821e3f702d37994e
"""Purpose: an archive entry whose UTF-8-flagged name does not decode is an error.

CPython decodes each central-directory name (UTF-8 when flag bit 0x800 is set,
else cp437) and propagates the decode error from `zipfile.ZipFile` and from
`zipimport.zipimporter`; it never skips the entry (HF-247).
"""

import os
import struct
import tempfile
import zipfile
import zipimport

root = tempfile.mkdtemp()
path = os.path.join(root, "bad_name.zip")
with zipfile.ZipFile(path, "w") as archive:
    archive.writestr("good.py", "x = 1\n")
    archive.writestr("XXXX.py", "y = 2\n")
data = bytearray(open(path, "rb").read())
# Flag the last central-directory entry's name as UTF-8 (bit 0x800) and make
# its four bytes invalid UTF-8; both readers trust the central directory.
central = data.rfind(b"PK\x01\x02")
(flags,) = struct.unpack_from("<H", data, central + 8)
struct.pack_into("<H", data, central + 8, flags | 0x800)
name_at = central + 46
data[name_at : name_at + 4] = b"\xff\xfe\xfd\xfc"
with open(path, "wb") as handle:
    handle.write(bytes(data))

try:
    print("zipfile", zipfile.ZipFile(path).namelist())
except Exception as exc:
    print("zipfile", type(exc).__name__)
try:
    importer = zipimport.zipimporter(path)
    print("zipimport", importer.find_spec("good") is not None)
except Exception as exc:
    print("zipimport", type(exc).__name__)
