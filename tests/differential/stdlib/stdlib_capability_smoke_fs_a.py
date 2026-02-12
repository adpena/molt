# MOLT_ENV: MOLT_CAPABILITIES=fs.read,env.read
"""Purpose: stdlib import smoke for fs-capability modules (A)."""

import compileall
import dbm
import filecmp
import fileinput
import genericpath
import linecache
import mailbox
import modulefinder
import netrc
import pkgutil

modules = [
    compileall,
    dbm,
    filecmp,
    fileinput,
    genericpath,
    linecache,
    mailbox,
    modulefinder,
    netrc,
    pkgutil,
]
print([mod.__name__ for mod in modules])
