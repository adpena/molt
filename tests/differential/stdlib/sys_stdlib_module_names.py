"""Purpose: differential coverage for sys.stdlib_module_names and
sys.builtin_module_names."""

import sys

print("stdlib_module_names type:", type(sys.stdlib_module_names).__name__)
print("os in stdlib:", "os" in sys.stdlib_module_names)
print("sys in stdlib:", "sys" in sys.stdlib_module_names)
print("builtin_module_names type:", type(sys.builtin_module_names).__name__)
