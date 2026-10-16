"""Purpose: differential coverage for PureWindowsPath semantics."""

from pathlib import PureWindowsPath

p = PureWindowsPath("C:\\Users\\Alice\\file.txt")
print("drive", p.drive)
print("parts", p.parts)
print("suffix", p.suffix)
print("stem", p.stem)
print("parent", p.parent)
print("with_suffix", p.with_suffix(".md"))

from pathlib import PurePosixPath

# Pure flavors obey their lexical contracts on every host.
for segments in (
    ("C:/a", "b", "..", "c"),
    ("C:/a", "/b"),
    ("C:/a", "D:b"),
    ("C:/a", "c:b"),
    ("//server/share", "leaf"),
    ("//?/UNC/server/share", "leaf"),
    ("C:", "leaf"),
    ("a", "", ".", "b"),
):
    value = PureWindowsPath(*segments)
    print("windows-joined", str(value), value.parts, value.drive, value.root)
    print("windows-parent", str(value.parent), type(value.parent) is PureWindowsPath)
    print("windows-derived", str(value.with_name("new.txt")), str(value.with_suffix(".xz")))
print("foreign-constructor", PurePosixPath(PureWindowsPath("C:/a/b")))
print("posix-backslash", PurePosixPath("a\\b").as_posix(), PurePosixPath("a\\b").parts)
print("windows-relative", PureWindowsPath("C:/A/B/c").relative_to("c:/a"))
print("windows-is-relative", PureWindowsPath("C:/A/B").is_relative_to("c:/a"))
print("windows-parents", [str(p) for p in PureWindowsPath("C:/a/b/c").parents])
