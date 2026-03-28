import sys
import os.path

# Chained attribute access
print(sys.version_info.major)
print(sys.version_info.minor)

# Module attribute used in expression
x = sys.maxsize
print(x > 0)

# os.path submodule attribute
print(os.path.sep)

# Module attribute as function call
import math
print(math.floor(3.7))
print(math.ceil(3.2))
print(math.sqrt(16.0))
