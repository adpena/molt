"""Purpose: differential coverage for sys modules main."""

import sys


print("__main__" in sys.modules)
print(sys.modules["__main__"].__name__)
print(sys.modules.get("sys") is sys)
