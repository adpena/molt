# MOLT_ENV: MOLT_CAPABILITIES=env.read
"""Purpose: differential coverage for os environ basic."""

import os


print("PATH" in os.environ)
print(isinstance(os.environ.get("PATH"), str))
