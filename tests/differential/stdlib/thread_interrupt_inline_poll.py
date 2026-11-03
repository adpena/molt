# MOLT_ENV: MOLT_CAPABILITIES=signal.raise
# MOLT_META: backends=native,wasm
"""Simulated SIGINT uses the same producer without requiring host OS signals."""

import _thread

try:
    _thread.interrupt_main()
    total = 0
    for i in range(1000):
        total += i
    print("interrupt missing", total)
except KeyboardInterrupt:
    print("KeyboardInterrupt at compiled safepoint")
