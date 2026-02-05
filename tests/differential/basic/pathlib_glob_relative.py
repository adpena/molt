# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for pathlib glob relative."""

from pathlib import Path
import tempfile


tmpdir = Path(tempfile.gettempdir()) / "molt_pathlib"
tmpdir.mkdir(exist_ok=True)

(tmpdir / "a.txt").write_text("a")
(tmpdir / "b.log").write_text("b")

files = sorted(p.name for p in tmpdir.glob("*.txt"))
print(files)

print((tmpdir / "a.txt").relative_to(tmpdir))

for child in tmpdir.iterdir():
    child.unlink()
tmpdir.rmdir()
