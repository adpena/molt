"""Purpose: differential coverage for module metadata."""

import os
import sys

sys.path.insert(0, os.path.dirname(__file__))

import module_meta_pkg


def _tail(path: str) -> str:
    return path.rstrip("/").split("/")[-1]


print(f"main_package={__package__}")
print(f"main_file={os.path.basename(__file__)}")
print(f"pkg_file={os.path.basename(module_meta_pkg.__file__)}")
print(f"pkg_package={module_meta_pkg.__package__}")
print(f"pkg_path_len={len(module_meta_pkg.__path__)}")
print(f"pkg_path_tail={_tail(module_meta_pkg.__path__[0])}")
