"""Purpose: differential coverage for os.path basics."""

import os

print(os.path.basename("/tmp/file.txt"))
print(os.path.dirname("/tmp/file.txt"))
print(os.path.join("/tmp", "file.txt"))
