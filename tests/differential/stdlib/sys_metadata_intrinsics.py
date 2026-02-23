"""Purpose: validate intrinsic-backed sys metadata parity shape/invariants."""

import sys


impl = sys.implementation
impl_version = tuple(impl.version)

assert isinstance(sys.hexversion, int) and not isinstance(sys.hexversion, bool)
assert isinstance(sys.api_version, int) and not isinstance(sys.api_version, bool)
assert isinstance(sys.abiflags, str)

assert hasattr(impl, "name")
assert hasattr(impl, "cache_tag")
assert hasattr(impl, "version")
assert hasattr(impl, "hexversion")

assert isinstance(impl.name, str) and len(impl.name) > 0
assert isinstance(impl.cache_tag, str) and len(impl.cache_tag) > 0
assert isinstance(impl.hexversion, int) and not isinstance(impl.hexversion, bool)

assert len(impl_version) == 5
assert all(
    isinstance(impl_version[idx], int) and not isinstance(impl_version[idx], bool)
    for idx in (0, 1, 2, 4)
)
assert isinstance(impl_version[3], str) and len(impl_version[3]) > 0

assert impl.hexversion == sys.hexversion
assert impl_version == tuple(sys.version_info)
expected_cache_tag_suffix = f"{sys.version_info[0]}{sys.version_info[1]}"
assert impl.cache_tag.endswith(expected_cache_tag_suffix)

print("ok")
