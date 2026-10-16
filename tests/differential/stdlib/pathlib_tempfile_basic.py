# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for pathlib tempfile basic."""

import tempfile
from pathlib import Path


tmpdir = Path(tempfile.gettempdir())
path = tmpdir / "molt_tmp_pathlib.txt"
print("joinpath", str(tmpdir.joinpath("molt_tmp_pathlib.txt")) == str(path))
print("rtruediv", str(str(tmpdir) / Path("molt_tmp_pathlib.txt")) == str(path))

if path.exists():
    path.unlink()

path.write_text("hello")
print(path.read_text())
print(path.exists())
path.unlink()


class ProtocolBuffer:
    def __init__(self, data):
        self.data = bytearray(data)
        self.events = []
        self.view = None

    def __buffer__(self, flags):
        self.events.append(("get", flags))
        self.view = memoryview(self.data)
        return self.view

    def __release_buffer__(self, view):
        self.events.append(("release", view is self.view))
        self.view = None

source = ProtocolBuffer(b"protocol")
print("buffer-path-write", path.write_bytes(source), source.events)
print("buffer-path-content", path.read_bytes())
source.data.extend(b"!")
print("buffer-path-unpinned", len(source.data))
path.write_bytes(b"old")
try:
    path.write_bytes(memoryview(b"abcdef")[::2])
except Exception as error:
    print("buffer-path-noncontiguous", type(error).__name__, path.read_bytes())
path.unlink()
