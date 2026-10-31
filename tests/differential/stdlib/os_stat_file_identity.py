"""Purpose: os.stat reports a file's identity and link count, as CPython does.

CPython's samestat compares (st_ino, st_dev). Hard links share that pair and
count each other in st_nlink; a distinct file differs. On Windows CPython reads
the pair from the volume serial and the 128-bit file ID, while a DirEntry's own
stat comes from the directory listing and reports zero identity unless it
follows a symlink. The output is relational, so it is the same on every host
where the program runs under both interpreters.
"""

import os
import shutil
import tempfile

root = tempfile.mkdtemp(prefix="molt-stat-identity-")
first = os.path.join(root, "first")
linked = os.path.join(root, "linked")
other = os.path.join(root, "other")
for path in (first, other):
    with open(path, "w") as stream:
        stream.write("a")
os.link(first, linked)

a, b, c = os.stat(first), os.stat(linked), os.stat(other)
print("hard link same file:", os.path.samefile(first, linked))
print("distinct file differs:", os.path.samefile(first, other))
print("samestat pairs:", os.path.samestat(a, b), os.path.samestat(a, c))
print("link counts:", a.st_nlink, b.st_nlink, c.st_nlink)
print("identity known:", a.st_ino != 0, a.st_ino == b.st_ino, a.st_dev == c.st_dev)
with open(first) as left, open(linked) as right, open(other) as third:
    print(
        "sameopenfile:",
        os.path.sameopenfile(left.fileno(), right.fileno()),
        os.path.sameopenfile(left.fileno(), third.fileno()),
    )

shutil.copyfile(other, first)
with open(linked) as stream:
    print("copy onto an existing distinct file reaches its links:", stream.read())

entries = {entry.name: entry for entry in os.scandir(root)}
entry = entries["first"]
print("inode is the lstat index:", entry.inode() == os.lstat(first).st_ino)
listed = entry.stat(follow_symlinks=False)
expected = 0 if os.name == "nt" else os.lstat(first).st_ino
print("entry stat identity:", listed.st_ino == expected, listed.st_nlink == (0 if os.name == "nt" else 2))

shutil.rmtree(root)
