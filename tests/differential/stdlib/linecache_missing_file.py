"""Purpose: differential coverage for linecache missing file handling."""

import linecache

line = linecache.getline("missing_linecache_file.py", 1)
print(line == "")
