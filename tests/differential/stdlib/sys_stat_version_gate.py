"""Purpose: regression for sys-version propagation into version-gated stat exports."""

import stat
import sys

version_triplet = tuple(sys.version_info[:3])
expect_313 = sys.version_info >= (3, 13)
actual_313 = hasattr(stat, "SF_DATALESS")

assert actual_313 == expect_313, (version_triplet, actual_313, expect_313)

if expect_313:
    assert stat.UF_SETTABLE == 0x0000FFFF
    assert stat.UF_TRACKED == 0x00000040
    assert stat.UF_DATAVAULT == 0x00000080
    assert stat.SF_SETTABLE == 0x3FFF0000
    assert stat.SF_RESTRICTED == 0x00080000
    assert stat.SF_FIRMLINK == 0x00800000
    assert stat.SF_DATALESS == 0x40000000
    assert stat.SF_SUPPORTED == 0x009F0000
    assert stat.SF_SYNTHETIC == 0xC0000000

print("sys_version_triplet", version_triplet)
print("expect_313", expect_313)
print("actual_313", actual_313)
