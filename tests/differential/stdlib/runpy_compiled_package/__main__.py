"""Compiler-admitted package entry point for runpy differentials."""

import sys


entry = 42
name_seen = __name__
package_seen = __package__
spec_seen = __spec__
argv0_seen = sys.argv[0]
path0_seen = sys.path[0]
sys_modules_seen = __name__ in sys.modules
