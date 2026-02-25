"""Purpose: differential coverage for os.walk, os.scandir,
os.get_terminal_size."""

import os
import shutil
import tempfile

# Create a temp dir structure
base = tempfile.mkdtemp()
os.makedirs(os.path.join(base, "sub1"))
os.makedirs(os.path.join(base, "sub2"))
with open(os.path.join(base, "file1.txt"), "w") as f:
    f.write("hello")
with open(os.path.join(base, "sub1", "file2.txt"), "w") as f:
    f.write("world")
# walk
entries = list(os.walk(base))
print("walk count:", len(entries))
print("walk root dirs:", sorted(entries[0][1]))
print("walk root files:", entries[0][2])
# scandir
scan = sorted([e.name for e in os.scandir(base)])
print("scandir:", scan)
# get_terminal_size
try:
    ts = os.get_terminal_size()
    print("terminal_size type:", type(ts).__name__)
except OSError:
    print("terminal_size type: OSError (no terminal)")
# cleanup
shutil.rmtree(base)
