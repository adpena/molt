"""Purpose: differential coverage for invoke_ffi on non-allowlisted os call."""

import os

cwd = os.getcwd()
print(type(cwd).__name__)
print(cwd.startswith("/"))
print(len(cwd) > 0)
