"""Purpose: differential coverage for input() builtin via stdin redirection."""
import io
import sys


if __name__ == "__main__":
    # input() exists and is callable
    print("input callable", callable(input))
    print("input type", type(input).__name__)

    # Redirect stdin to test input() reading
    original_stdin = sys.stdin

    # Basic string input
    sys.stdin = io.StringIO("hello\n")
    result = input()
    print("basic input", repr(result))

    # input() with prompt (prompt goes to stdout but we capture the return)
    sys.stdin = io.StringIO("world\n")
    result = input()
    print("input value", repr(result))

    # input strips trailing newline
    sys.stdin = io.StringIO("trailing\n")
    result = input()
    print("strip newline", repr(result))
    print("no newline in result", "\n" not in result)

    # input with empty string
    sys.stdin = io.StringIO("\n")
    result = input()
    print("empty input", repr(result))

    # input with spaces
    sys.stdin = io.StringIO("  spaces  \n")
    result = input()
    print("spaces input", repr(result))

    # input with multiple lines (only reads first)
    sys.stdin = io.StringIO("first\nsecond\n")
    r1 = input()
    r2 = input()
    print("multi line1", repr(r1))
    print("multi line2", repr(r2))

    # input at EOF raises EOFError
    sys.stdin = io.StringIO("")
    try:
        input()
        print("eof should not reach")
    except EOFError:
        print("eof error", "EOFError")

    # input with unicode
    sys.stdin = io.StringIO("caf\u00e9\n")
    result = input()
    print("unicode input", repr(result))

    # input with numbers (returns string)
    sys.stdin = io.StringIO("42\n")
    result = input()
    print("numeric input type", type(result).__name__)
    print("numeric input value", repr(result))

    # Restore stdin
    sys.stdin = original_stdin
