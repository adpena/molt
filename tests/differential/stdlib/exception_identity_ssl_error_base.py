"""Purpose: ssl.SSLError derives from OSError, as in CPython."""

import ssl


def main():
    print("issubclass OSError", issubclass(ssl.SSLError, OSError))
    print("module", ssl.SSLError.__module__)
    error = ssl.SSLError(1, "bad record")
    print("errno", error.errno, "strerror", error.strerror, "args", error.args)
    for name in ("SSLWantReadError", "SSLCertVerificationError", "SSLZeroReturnError"):
        cls = getattr(ssl, name, None)
        if cls is None:
            print(name, "missing")
            continue
        print(name, issubclass(cls, ssl.SSLError), issubclass(cls, OSError))
    try:
        raise ssl.SSLError(2, "handshake")
    except OSError as exc:
        print("caught as OSError", type(exc).__name__, exc.errno)


if __name__ == "__main__":
    main()
