# MOLT_ENV: MOLT_CAPABILITIES=rand
import os

print("len0", len(os.urandom(0)))
print("len3", len(os.urandom(3)))
print("type", type(os.urandom(1)).__name__)

try:
    os.urandom(-1)
except Exception as exc:
    print("neg", type(exc).__name__, str(exc))

try:
    os.urandom(3.5)
except Exception as exc:
    print("float", type(exc).__name__, str(exc))
