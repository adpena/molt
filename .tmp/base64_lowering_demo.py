import base64


def _roundtrip(name, enc, dec, data):
    encoded = enc(data)
    decoded = dec(encoded)
    if decoded != data:
        raise AssertionError(f"{name} roundtrip failed: {decoded!r} != {data!r}")
    return encoded


def main():
    data = b"hello world"
    _roundtrip("b64", base64.b64encode, base64.b64decode, data)
    _roundtrip("urlsafe", base64.urlsafe_b64encode, base64.urlsafe_b64decode, data)
    _roundtrip("b16", base64.b16encode, base64.b16decode, data)
    _roundtrip("b32", base64.b32encode, base64.b32decode, data)
    _roundtrip("b32hex", base64.b32hexencode, base64.b32hexdecode, data)
    _roundtrip("b85", base64.b85encode, base64.b85decode, data)
    _roundtrip("a85", base64.a85encode, base64.a85decode, data)
    try:
        z85encode = base64.z85encode
        z85decode = base64.z85decode
    except AttributeError:
        pass
    else:
        _roundtrip("z85", z85encode, z85decode, data)

    encoded = base64.encodebytes(data)
    if base64.decodebytes(encoded) != data:
        raise AssertionError("encodebytes/decodebytes failed")
    print("base64 lowering OK")


if __name__ == "__main__":
    main()
