"""Purpose: differential coverage for intrinsic-backed stat constants/helpers."""

import stat

mode = stat.S_IFDIR | stat.S_IRWXU | stat.S_IRGRP | stat.S_IXGRP | stat.S_ISUID

print(callable(stat.S_IFMT), callable(stat.S_IMODE))
print(stat.S_IFMT(mode) == stat.S_IFDIR)
print(stat.S_IMODE(mode) == (stat.S_IRWXU | stat.S_IRGRP | stat.S_IXGRP | stat.S_ISUID))
print(stat.S_ISDIR(mode), stat.S_ISREG(mode))
print(stat.S_ISREG(stat.S_IFREG), stat.S_ISDIR(stat.S_IFREG))
print(
    stat.S_ISLNK(stat.S_IFLNK),
    stat.S_ISCHR(stat.S_IFCHR),
    stat.S_ISBLK(stat.S_IFBLK),
    stat.S_ISFIFO(stat.S_IFIFO),
    stat.S_ISSOCK(stat.S_IFSOCK),
)
print(stat.ST_MODE, stat.ST_INO, stat.ST_DEV, stat.ST_NLINK, stat.ST_UID, stat.ST_GID)
