"""Purpose: importing intrinsic-backed sys/os modules should compile and execute cleanly."""

import os
import sys


print(sys.version_info[0])
print(os.path.basename("/tmp/example.txt"))
print(hasattr(sys, "flags"))
