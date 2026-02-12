# MOLT_META: platforms=windows
# MOLT_ENV: MOLT_CAPABILITIES=env.read
"""Purpose: differential coverage for windows platform basic."""

import msvcrt
import winreg
import ntpath
import nturl2path

print(hasattr(msvcrt, 'kbhit'))
print(hasattr(winreg, 'HKEY_CURRENT_USER'))
print(ntpath.join('C:\', 'tmp'))
print(nturl2path.pathname2url('C:\tmp'))
