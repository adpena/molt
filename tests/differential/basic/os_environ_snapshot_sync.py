# MOLT_ENV: MOLT_CAPABILITIES=env.read,env.write MOLT_ENV_PRELOAD=seed
"""Purpose: ensure os.environ reads/writes are fully intrinsic-backed and synchronized."""

import os


PRELOAD = "MOLT_ENV_PRELOAD"
MUT_KEY = "MOLT_ENV_INTRINSIC_SYNC_KEY"


print(os.getenv(PRELOAD))
print(PRELOAD in os.environ)
print(os.environ.copy().get(PRELOAD))
print(PRELOAD in list(os.environ.keys()))

os.putenv(MUT_KEY, "one")
print(os.getenv(MUT_KEY))
print(os.environ[MUT_KEY])
print(MUT_KEY in os.environ)

os.unsetenv(MUT_KEY)
print(os.getenv(MUT_KEY) is None)
print(MUT_KEY in os.environ)

os.environ[MUT_KEY] = "two"
print(os.getenv(MUT_KEY))
print(dict(os.environ.items()).get(MUT_KEY))

del os.environ[MUT_KEY]
print(os.getenv(MUT_KEY) is None)
print(MUT_KEY in os.environ)
