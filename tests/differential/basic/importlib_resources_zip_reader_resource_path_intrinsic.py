# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: zip resource reader path parity for importlib.machinery loaders."""

import importlib.machinery
import os
import tempfile
import zipfile


root = tempfile.mkdtemp(prefix="molt_zip_reader_path_")
archive = os.path.join(root, "pkg.zip")
with zipfile.ZipFile(archive, "w") as zf:
    zf.writestr("pkg/__init__.py", "VALUE = 1\n")
    zf.writestr("pkg/data.txt", "payload\n")

loader = importlib.machinery.ZipSourceLoader("pkg", archive, "pkg/__init__.py")
reader = loader.get_resource_reader("pkg")
if reader is None:
    raise RuntimeError("zip source loader did not provide a resource reader")

raised = False
try:
    reader.resource_path("data.txt")
except FileNotFoundError:
    raised = True

with reader.open_resource("data.txt") as handle:
    payload = handle.read()

print(raised)
print(payload == b"payload\n")
print(reader.is_resource("data.txt"))
