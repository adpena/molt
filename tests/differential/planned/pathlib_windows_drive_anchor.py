"""Purpose: differential coverage for PureWindowsPath drive/anchor semantics."""

from pathlib import PureWindowsPath

p = PureWindowsPath("\\\\server\\share\\path")
print("drive", p.drive)
print("anchor", p.anchor)
print("parts", p.parts)
