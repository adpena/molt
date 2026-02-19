"""Purpose: differential coverage for quopri API and helper behavior."""

import io
import quopri


def main() -> None:
    payload = b"Hello=world\t \nSecond line.\nThird_line with space "
    encoded = quopri.encodestring(payload, quotetabs=True)
    print("encode", encoded)
    print("decode_roundtrip", quopri.decodestring(encoded) == payload)

    header_encoded = quopri.encodestring(b"a b_c", header=True)
    print("header_encode", header_encoded)
    print("header_decode", quopri.decodestring(b"a_b=5Fc", header=True))

    print("needsquoting", quopri.needsquoting(b"=", False, False))
    print("quote", quopri.quote(b"A"))
    print("ishex", quopri.ishex(b"F"), quopri.ishex(b"g"))
    print("unhex", quopri.unhex(b"4F"))

    in_fp = io.BytesIO(payload)
    out_fp = io.BytesIO()
    quopri.encode(in_fp, out_fp, True)
    file_encoded = out_fp.getvalue()
    print("file_encode", file_encoded)

    in_fp2 = io.BytesIO(file_encoded)
    out_fp2 = io.BytesIO()
    quopri.decode(in_fp2, out_fp2)
    print("file_decode_roundtrip", out_fp2.getvalue() == payload)

    long_line = b"A" * 90
    long_encoded = quopri.encodestring(long_line)
    print("soft_break", b"=\n" in long_encoded)


if __name__ == "__main__":
    main()
