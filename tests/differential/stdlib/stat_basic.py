"""Purpose: differential coverage for stat basics."""

import stat

mode_dir = stat.S_IFDIR | 0o755
mode_file = stat.S_IFREG | 0o644

print(stat.S_ISDIR(mode_dir), stat.S_ISREG(mode_dir))
print(stat.S_ISREG(mode_file), stat.S_ISDIR(mode_file))
print(stat.S_ISREG(stat.S_IFREG))
