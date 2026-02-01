# MOLT_META: platforms=windows
"""Purpose: stdlib import smoke for windows-only modules."""

import msvcrt
import nt
import ntpath
import nturl2path
import winreg
import winsound

modules = [
    msvcrt,
    nt,
    ntpath,
    nturl2path,
    winreg,
    winsound,
]
print([mod.__name__ for mod in modules])
