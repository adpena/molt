# MOLT_ENV: MOLT_CODEC=json
"""Purpose: differential coverage for codec parity."""

import json

import molt_cbor
import molt_json
import molt_msgpack


JSON_TEXT_1 = '{"a":1,"b":true,"c":null,"d":[1,2]}'
MSGPACK_BYTES_1 = b"\x84\xa1a\x01\xa1b\xc3\xa1c\xc0\xa1d\x92\x01\x02"
CBOR_BYTES_1 = b"\xa4\x61a\x01\x61b\xf5\x61c\xf6\x61d\x82\x01\x02"
JSON_TEXT_2 = '{"empty":{},"list":[],"neg":-7}'
MSGPACK_BYTES_2 = b"\x83\xa5empty\x80\xa4list\x90\xa3neg\xf9"
CBOR_BYTES_2 = b"\xa3\x65empty\xa0\x64list\x80\x63neg\x26"
JSON_TEXT_3 = '[0,-1,true,false,null,{"x":5,"y":[1,2]}]'
MSGPACK_BYTES_3 = b"\x96\x00\xff\xc3\xc2\xc0\x82\xa1x\x05\xa1y\x92\x01\x02"
CBOR_BYTES_3 = b"\x86\x00\x20\xf5\xf4\xf6\xa2\x61x\x05\x61y\x82\x01\x02"

CASES = [
    ("case1", JSON_TEXT_1, MSGPACK_BYTES_1, CBOR_BYTES_1),
    ("case2", JSON_TEXT_2, MSGPACK_BYTES_2, CBOR_BYTES_2),
    ("case3", JSON_TEXT_3, MSGPACK_BYTES_3, CBOR_BYTES_3),
]


def _normalize(obj):
    return json.dumps(obj, sort_keys=True, separators=(",", ":"))


def _report_case(label, json_text, msgpack_bytes, cbor_bytes):
    parsed_json = molt_json.parse(json_text)
    parsed_msgpack = molt_msgpack.parse(msgpack_bytes)
    parsed_cbor = molt_cbor.parse(cbor_bytes)
    norm_json = _normalize(parsed_json)
    norm_msgpack = _normalize(parsed_msgpack)
    norm_cbor = _normalize(parsed_cbor)
    print(f"{label}:json:{norm_json}")
    print(f"{label}:msgpack:{norm_msgpack}")
    print(f"{label}:cbor:{norm_cbor}")
    print(f"{label}:all_equal:{norm_json == norm_msgpack == norm_cbor}")


def _report_json_error(label, text):
    try:
        molt_json.parse(text)
    except Exception:
        print(f"{label}:error")
    else:
        print(f"{label}:ok")


def _report_msgpack_error(label, data):
    try:
        molt_msgpack.parse(data)
    except Exception:
        print(f"{label}:error")
    else:
        print(f"{label}:ok")


def _report_cbor_error(label, data):
    try:
        molt_cbor.parse(data)
    except Exception:
        print(f"{label}:error")
    else:
        print(f"{label}:ok")


def main():
    for label, json_text, msgpack_bytes, cbor_bytes in CASES:
        _report_case(label, json_text, msgpack_bytes, cbor_bytes)
    bad_json = "{" + str(1)
    bad_msgpack = "bad"
    bad_cbor = "bad"
    bad_msgpack_bytes = b"\xc7"
    bad_cbor_bytes = b"\x1c"
    _report_json_error("json_invalid", bad_json)
    _report_msgpack_error("msgpack_invalid", bad_msgpack)
    _report_cbor_error("cbor_invalid", bad_cbor)
    _report_msgpack_error("msgpack_invalid_bytes", bad_msgpack_bytes)
    _report_cbor_error("cbor_invalid_bytes", bad_cbor_bytes)


if __name__ == "__main__":
    main()
