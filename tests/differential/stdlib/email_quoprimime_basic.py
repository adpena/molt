"""Purpose: differential coverage for email.quoprimime basics."""

import email.quoprimime as qm


def main() -> None:
    print("header_encode", qm.header_encode(b"Hello World"))
    print("header_decode", qm.header_decode("Hello_World=21"))
    print("body_encode", qm.body_encode("A line with space \\n", maxlinelen=20))
    print("body_decode", qm.body_decode("A line with space=20\\n"))
    print("body_length", qm.body_length(b"A=\\n"))
    print("header_length", qm.header_length(b"A_"))
    print("quote", qm.quote("A"))
    print("unquote", qm.unquote("=41"))
    print("check", qm.body_check(ord("=")), qm.header_check(ord("_")))


if __name__ == "__main__":
    main()
