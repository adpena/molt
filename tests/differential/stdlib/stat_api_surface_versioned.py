"""Purpose: assert stat API/version gating across CPython 3.12/3.13/3.14."""

import stat
import sys

HAS_313_CONSTS = sys.version_info >= (3, 13)
HAS_WINDOWS_CONSTS = sys.platform.startswith("win")
VERSIONED_313_NAMES = [
    "UF_SETTABLE",
    "UF_TRACKED",
    "UF_DATAVAULT",
    "SF_SETTABLE",
    "SF_RESTRICTED",
    "SF_FIRMLINK",
    "SF_DATALESS",
    "SF_SUPPORTED",
    "SF_SYNTHETIC",
]
WINDOWS_NAMES = [
    "IO_REPARSE_TAG_APPEXECLINK",
    "IO_REPARSE_TAG_MOUNT_POINT",
    "IO_REPARSE_TAG_SYMLINK",
]

for name in VERSIONED_313_NAMES:
    has = hasattr(stat, name)
    assert has == HAS_313_CONSTS, (name, has, HAS_313_CONSTS)

for name in WINDOWS_NAMES:
    has = hasattr(stat, name)
    assert has == HAS_WINDOWS_CONSTS, (name, has, HAS_WINDOWS_CONSTS)

if HAS_313_CONSTS:
    expected_313 = {
        "UF_SETTABLE": 0x0000FFFF,
        "UF_TRACKED": 0x00000040,
        "UF_DATAVAULT": 0x00000080,
        "SF_SETTABLE": 0x3FFF0000,
        "SF_RESTRICTED": 0x00080000,
        "SF_FIRMLINK": 0x00800000,
        "SF_DATALESS": 0x40000000,
        "SF_SUPPORTED": 0x009F0000,
        "SF_SYNTHETIC": 0xC0000000,
    }
    for name, value in expected_313.items():
        assert getattr(stat, name) == value, (name, getattr(stat, name), value)

if HAS_WINDOWS_CONSTS:
    expected_windows = {
        "IO_REPARSE_TAG_APPEXECLINK": 0x8000001B,
        "IO_REPARSE_TAG_MOUNT_POINT": 0xA0000003,
        "IO_REPARSE_TAG_SYMLINK": 0xA000000C,
    }
    for name, value in expected_windows.items():
        assert getattr(stat, name) == value, (name, getattr(stat, name), value)

# Avoid relying on dir() ordering/contents and str helper methods to keep this
# robust across runtimes.
def _is_upper_const_name(name: object) -> bool:
    if not isinstance(name, str):
        return False
    if name and name[0] == "_":
        return False
    has_alpha = False
    for ch in name:
        if "a" <= ch <= "z":
            return False
        if "A" <= ch <= "Z":
            has_alpha = True
    return has_alpha


upper_int_items = sorted(
    (name, value)
    for name, value in stat.__dict__.items()
    if _is_upper_const_name(name) and isinstance(value, int)
)
upper_int_count = len(upper_int_items)
expected_count = 68
if HAS_WINDOWS_CONSTS:
    expected_count += len(WINDOWS_NAMES)
if HAS_313_CONSTS:
    expected_count += len(VERSIONED_313_NAMES)
assert upper_int_count == expected_count, (upper_int_count, expected_count)
print("upper_int_count", upper_int_count)
print("upper_int_items", upper_int_items)

required_exports = {
    "filemode",
    "S_IFMT",
    "S_IMODE",
    "S_ISDIR",
    "S_ISREG",
    "S_ISCHR",
    "S_ISBLK",
    "S_ISFIFO",
    "S_ISLNK",
    "S_ISSOCK",
    "S_ISDOOR",
    "S_ISPORT",
    "S_ISWHT",
}
for name in sorted(required_exports):
    assert hasattr(stat, name), name
assert not hasattr(stat, "__all__"), "__all__ should not be defined on stat"
print("versioned_313_presence", {name: hasattr(stat, name) for name in VERSIONED_313_NAMES})

mode = stat.S_IFREG | stat.S_ISUID | stat.S_IXUSR
assert stat.filemode(mode) == "---s------", stat.filemode(mode)
print("filemode_check", stat.filemode(mode))
