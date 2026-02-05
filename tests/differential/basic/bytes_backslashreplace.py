"""Purpose: parity coverage for backslashreplace error handler."""

if __name__ == "__main__":
    print(b"\xff".decode("ascii", "backslashreplace"))
    print(b"\xff\xfe".decode("utf-8", "backslashreplace"))
    print(b"\x00\xd8".decode("utf-16-le", "backslashreplace"))
    print("\u00e9".encode("ascii", "backslashreplace"))
    print("\u6c49".encode("latin-1", "backslashreplace"))
