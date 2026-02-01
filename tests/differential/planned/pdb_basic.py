# MOLT_ENV: MOLT_CAPABILITIES=tty
"""Purpose: differential coverage for pdb basic."""

import pdb

print(hasattr(pdb.Pdb(), 'set_trace'))
