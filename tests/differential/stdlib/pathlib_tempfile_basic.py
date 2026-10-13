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
