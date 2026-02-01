# MOLT_META: platforms=posix
"""Purpose: stdlib import smoke for readline (posix)."""

import readline

modules = [
    readline,
]
print([mod.__name__ for mod in modules])
