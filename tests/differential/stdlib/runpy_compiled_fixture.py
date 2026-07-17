"""Compiler-admitted module body for runpy/importlib execution differentials."""

import sys


value = 11
seed_seen = globals().get("seed")
name_seen = __name__
package_seen = __package__
file_seen = __file__
spec_seen = __spec__
cached_seen = __cached__
loader_seen = __loader__
argv0_seen = sys.argv[0]
sys_modules_seen = __name__ in sys.modules
sys_modules_value_seen = sys.modules.get(__name__)
if globals().get("capture_globals"):
    globals_seen = globals()
if globals().get("raise_from_runpy"):
    raise RuntimeError("runpy compiled fixture failure")
