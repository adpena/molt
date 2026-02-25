"""Purpose: differential coverage for signal.strsignal, signal.valid_signals,
signal.getsignal, signal.Signals."""

import signal

# strsignal
s = signal.strsignal(signal.SIGINT)
print("strsignal SIGINT:", type(s).__name__)
print("strsignal SIGINT has value:", s is not None)
s2 = signal.strsignal(signal.SIGTERM)
print("strsignal SIGTERM:", type(s2).__name__)
# valid_signals
vs = signal.valid_signals()
print("valid_signals type:", type(vs).__name__)
print("SIGINT in valid:", signal.SIGINT in vs)
print("SIGTERM in valid:", signal.SIGTERM in vs)
# getsignal
h = signal.getsignal(signal.SIGINT)
print("getsignal SIGINT type:", type(h).__name__)
# Signals enum
print("Signals.SIGINT:", signal.Signals.SIGINT)
print("Signals.SIGTERM:", signal.Signals.SIGTERM)
