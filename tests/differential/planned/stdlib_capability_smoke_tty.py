"""Purpose: stdlib import smoke for tty/interactive helpers."""

import bdb
import cmd
import code
import pdb
import rlcompleter
import getpass

modules = [
    bdb,
    cmd,
    code,
    pdb,
    rlcompleter,
    getpass,
]
print([mod.__name__ for mod in modules])
