"""Purpose: an os.DirEntry caches its stat facts, as CPython does.

CPython caches lstat on the entry, returns that same object from stat() for a
non-symlink, fetches a symlink's followed stat once, and caches inode()
(Modules/posixmodule.c: DirEntry_get_lstat, os_DirEntry_stat_impl,
os_DirEntry_inode_impl). A file changed or removed after the first call does
not change what the entry reports.
"""

import os
import shutil
import tempfile

root = tempfile.mkdtemp(prefix="molt-direntry-cache-")
path = os.path.join(root, "file")
with open(path, "w") as stream:
    stream.write("a")
link = os.path.join(root, "link")
try:
    os.symlink(path, link)
    have_link = True
except OSError:
    have_link = False

entries = {entry.name: entry for entry in os.scandir(root)}
entry = entries["file"]
first = entry.stat()
with open(path, "a") as stream:
    stream.write("bbbb")
second = entry.stat()
print("stat is cached:", first is second, second.st_size)
print("non-symlink stat is the cached lstat:", entry.stat() is entry.stat(follow_symlinks=False))
print("a fresh os.stat sees the growth:", os.stat(path).st_size)
inode = entry.inode()
print("inode is cached and matches lstat:", entry.inode() == inode == os.lstat(path).st_ino)
if have_link:
    linked = entries["link"]
    followed = linked.stat()
    print("symlink stat follows once:", followed.st_size, linked.stat() is followed)
    print("symlink lstat differs from its stat:", linked.stat(follow_symlinks=False) is not followed)
os.remove(path)
print("cached stat survives removal:", entry.stat().st_size, entry.stat(follow_symlinks=False).st_size)
shutil.rmtree(root)
