"""Purpose: importing intrinsic-backed types should initialize its bootstrap surface."""

import types


print(types.__name__)
print(isinstance(types.NoneType, type))
print(types.ModuleType.__name__)
