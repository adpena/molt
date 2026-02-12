# MOLT_ENV: MOLT_CAPABILITIES=env.read,env.write
"""Purpose: lock runtime-owned os.environ len/contains/popitem/clear behavior."""

import os


KEY = "MOLT_ENV_INTRINSIC_OPS_KEY"


snapshot = os.environ.copy()
try:
    os.environ.pop(KEY, None)
    before_len = len(os.environ)
    os.environ[KEY] = "value"
    print(KEY in os.environ)
    print(len(os.environ) >= before_len)

    popped_key, popped_value = os.environ.popitem()
    print(isinstance(popped_key, str))
    print(isinstance(popped_value, str))

    os.environ.clear()
    print(len(os.environ))
finally:
    os.environ.clear()
    os.environ.update(snapshot)
