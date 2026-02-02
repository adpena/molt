# MOLT_ENV: MOLT_CAPABILITIES=process.signal
"""Purpose: differential coverage for signal basics."""

import signal

print(signal.SIGINT in signal.Signals)
handler = signal.getsignal(signal.SIGINT)
print(handler == signal.default_int_handler or handler == signal.SIG_DFL)

signal.signal(signal.SIGINT, signal.SIG_IGN)
print(signal.getsignal(signal.SIGINT) == signal.SIG_IGN)

# restore default
signal.signal(signal.SIGINT, signal.SIG_DFL)
print(signal.getsignal(signal.SIGINT) == signal.SIG_DFL)
