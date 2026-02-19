"""Purpose: assert CPython version-gated import behavior for imghdr."""

import importlib
import sys
import warnings

warnings.filterwarnings(
    "ignore",
    category=DeprecationWarning,
    message="'imghdr' is deprecated and slated for removal in Python 3.13",
)

if sys.version_info >= (3, 13):
    try:
        importlib.import_module("imghdr")
    except ModuleNotFoundError:
        print("imghdr_absent", tuple(sys.version_info[:3]))
    else:
        raise AssertionError("imghdr must be absent for Python >= 3.13")
else:
    mod = importlib.import_module("imghdr")
    assert hasattr(mod, "what")
    assert callable(mod.what)
    assert hasattr(mod, "tests")
    assert isinstance(mod.tests, list)
    print("imghdr_present", tuple(sys.version_info[:3]))
