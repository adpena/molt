# MOLT_META: platforms=linux,macos,freebsd
# MOLT_ENV: MOLT_CAPABILITIES=env.read
"""Purpose: differential coverage for posix platform basic."""

import fcntl
import termios
import tty
import resource
import syslog

print(hasattr(fcntl, 'LOCK_SH'))
print(hasattr(termios, 'TCSANOW'))
print(hasattr(tty, 'setraw'))
print(hasattr(resource, 'getrlimit'))
print(hasattr(syslog, 'LOG_INFO'))
