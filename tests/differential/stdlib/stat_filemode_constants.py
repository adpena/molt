"""Purpose: differential coverage for stat constants + filemode."""

import stat
import sys

modes = [
    stat.S_IFREG | stat.S_IRUSR | stat.S_IWUSR | stat.S_IRGRP | stat.S_IROTH,
    stat.S_IFDIR
    | stat.S_IRWXU
    | stat.S_IRGRP
    | stat.S_IXGRP
    | stat.S_IROTH
    | stat.S_IXOTH,
    stat.S_IFLNK | stat.S_IRWXU,
    stat.S_IFREG | stat.S_ISUID | stat.S_IXUSR,
    stat.S_IFREG | stat.S_ISGID | stat.S_IXGRP,
    stat.S_IFDIR | stat.S_ISVTX | stat.S_IXOTH,
    stat.S_IFWHT,
]

for mode in modes:
    print(mode, stat.filemode(mode))

print(stat.S_ISDOOR(stat.S_IFDOOR), stat.S_ISPORT(stat.S_IFPORT))
print(stat.S_ISWHT(stat.S_IFWHT), stat.S_ISSOCK(stat.S_IFSOCK))
print(stat.S_ENFMT == stat.S_ISGID, stat.S_IREAD == stat.S_IRUSR)
print(stat.S_IWRITE == stat.S_IWUSR, stat.S_IEXEC == stat.S_IXUSR)
print(stat.UF_NODUMP, stat.UF_APPEND, stat.SF_APPEND, stat.SF_SNAPSHOT)
print(
    stat.FILE_ATTRIBUTE_READONLY,
    stat.FILE_ATTRIBUTE_DIRECTORY,
    stat.FILE_ATTRIBUTE_ARCHIVE,
)

expect_313 = sys.version_info >= (3, 13)
print(
    hasattr(stat, "SF_DATALESS"),
    hasattr(stat, "SF_FIRMLINK"),
    hasattr(stat, "SF_RESTRICTED"),
    hasattr(stat, "SF_SUPPORTED"),
    hasattr(stat, "SF_SYNTHETIC"),
    expect_313,
)
print(
    hasattr(stat, "UF_DATAVAULT"),
    hasattr(stat, "UF_TRACKED"),
    hasattr(stat, "UF_SETTABLE"),
    hasattr(stat, "SF_SETTABLE"),
    expect_313,
)
