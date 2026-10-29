# MOLT_META: expect_fail=molt expect_fail_reason=open_finding finding=HF-207 expect_molt_returncode=1 expect_molt_stdout_sha256=183ea8d7980abdabbfa61eda9872ad62293a08ef79c9323f8f545d0788e3a589
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
