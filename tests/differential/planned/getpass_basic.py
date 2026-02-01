# MOLT_ENV: MOLT_CAPABILITIES=tty
"""Purpose: differential coverage for getpass basic."""

import getpass

# getuser reads env/user db depending on platform; avoid interactive prompts
print(isinstance(getpass.getuser(), str))
