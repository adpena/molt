"""Purpose: differential coverage for package __all__ child import star."""
# ruff: noqa: F403, F405

import os
import sys

sys.path.insert(0, os.path.dirname(__file__))

from import_star_pkg_all_child import *

print(f"child_name={child.__name__}")
print(f"child_value={child.VALUE}")

try:
    from import_star_pkg_all_missing import *
except BaseException as exc:
    print(f"missing_type={type(exc).__name__}")
    print(f"missing_msg={exc}")
else:
    print("missing_type=NO_ERROR")
