"""Purpose: differential coverage for PureWindowsPath semantics."""

from pathlib import PureWindowsPath

p = PureWindowsPath("C:\\Users\\Alice\\file.txt")
print("drive", p.drive)
print("parts", p.parts)
print("suffix", p.suffix)
print("stem", p.stem)
print("parent", p.parent)
print("with_suffix", p.with_suffix(".md"))
