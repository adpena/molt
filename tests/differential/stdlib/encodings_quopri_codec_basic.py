"""Purpose: differential coverage for encodings.quopri_codec basics."""

import encodings.quopri_codec as qc


def main() -> None:
    print("encode", qc.quopri_encode(b"a b=\\n", "strict"))
    print("decode", qc.quopri_decode(b"a=20b=3D\\n", "strict"))
    print("inc_enc", qc.IncrementalEncoder().encode(b"a b=\\n"))
    print("inc_dec", qc.IncrementalDecoder().decode(b"a=20b=3D\\n"))
    reg = qc.getregentry()
    print("reg", reg.name, reg._is_text_encoding)


if __name__ == "__main__":
    main()
