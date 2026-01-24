"""Purpose: differential coverage for sys basic."""

import sys


print(isinstance(sys.argv, list))
print(isinstance(sys.path, list))
print(isinstance(sys.modules, dict))
print(sys.getrecursionlimit() > 0)
