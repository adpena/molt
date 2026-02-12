# MOLT_ENV: MOLT_CAPABILITIES=tty
"""Purpose: differential coverage for cmd basic."""

import cmd

class Mini(cmd.Cmd):
    prompt = ''

obj = Mini()
print(isinstance(obj, cmd.Cmd))
