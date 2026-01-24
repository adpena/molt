"""Purpose: differential coverage for signal/KeyboardInterrupt handling."""

try:
    import signal
except Exception as exc:
    print(type(exc).__name__, exc)
else:
    try:
        signal.raise_signal(signal.SIGINT)
    except Exception as exc:
        print(type(exc).__name__, exc)
