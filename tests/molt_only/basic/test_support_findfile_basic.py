"""Purpose: differential coverage for test.support.findfile."""

from molt.stdlib.test import support
import os

abs_path = os.path.abspath(__file__)
print("abs", support.findfile(abs_path) == abs_path)

missing = "molt_missing_support_file_hopefully"
print("missing", support.findfile(missing) == missing)
