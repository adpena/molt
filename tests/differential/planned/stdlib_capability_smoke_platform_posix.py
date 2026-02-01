# MOLT_META: platforms=posix
"""Purpose: stdlib import smoke for posix-only modules."""

import fcntl
import grp
import posix
import posixpath
import pty
import pwd
import resource
import syslog
import termios
import tty
import curses

modules = [
    fcntl,
    grp,
    posix,
    posixpath,
    pty,
    pwd,
    resource,
    syslog,
    termios,
    tty,
    curses,
]
print([mod.__name__ for mod in modules])
