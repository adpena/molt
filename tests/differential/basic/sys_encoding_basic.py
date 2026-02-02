"""Purpose: differential coverage for sys encoding helpers."""

import sys

print(sys.getdefaultencoding())
print(sys.getfilesystemencoding())
print(isinstance(sys.argv, list))
print(isinstance(sys.modules, dict))
