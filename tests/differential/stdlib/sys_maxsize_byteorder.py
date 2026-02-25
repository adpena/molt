"""Purpose: differential coverage for sys.maxsize, sys.maxunicode,
sys.byteorder, sys.platform, sys.version, sys.version_info, sys.prefix,
sys.exec_prefix, sys.platlibdir, sys.copyright."""

import sys

print("maxsize type:", type(sys.maxsize).__name__)
print("maxsize > 0:", sys.maxsize > 0)
print("maxunicode:", sys.maxunicode)
print("byteorder:", sys.byteorder in ("little", "big"))
print("platform type:", type(sys.platform).__name__)
print("version type:", type(sys.version).__name__)
print("version_info type:", type(sys.version_info).__name__)
print("prefix type:", type(sys.prefix).__name__)
print("exec_prefix type:", type(sys.exec_prefix).__name__)
print("platlibdir:", sys.platlibdir)
print("copyright type:", type(sys.copyright).__name__)
