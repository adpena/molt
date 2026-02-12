# MOLT_ENV: MOLT_CAPABILITIES=tty
"""Purpose: differential coverage for code basic."""

import code

console = code.InteractiveConsole()
print(hasattr(console, 'push'))
