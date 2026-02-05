"""Purpose: validate test.support.import_helper basics."""

from molt.stdlib.test import import_helper
import sys
import unittest

mod = import_helper.import_module("math")
print(mod.__name__)

fresh = import_helper.import_fresh_module("math")
print(fresh is sys.modules["math"])

try:
    import_helper.import_module("_molt_missing_module_hopefully")
except unittest.SkipTest as exc:
    print("skip", str(exc)[:40])
