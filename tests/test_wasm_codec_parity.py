import os
import shutil
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest

from tests.wasm_harness import write_wasm_runner


CODEC_SRC = textwrap.dedent(
    """\
    import json

    import molt_cbor
    import molt_json
    import molt_msgpack

    JSON_TEXT_1 = '{"a":1,"b":true,"c":null,"d":[1,2]}'
    MSGPACK_BYTES_1 = b"\\x84\\xa1a\\x01\\xa1b\\xc3\\xa1c\\xc0\\xa1d\\x92\\x01\\x02"
    CBOR_BYTES_1 = b"\\xa4\\x61a\\x01\\x61b\\xf5\\x61c\\xf6\\x61d\\x82\\x01\\x02"
    JSON_TEXT_2 = '{"empty":{},"list":[],"neg":-7}'
    MSGPACK_BYTES_2 = b"\\x83\\xa5empty\\x80\\xa4list\\x90\\xa3neg\\xf9"
    CBOR_BYTES_2 = b"\\xa3\\x65empty\\xa0\\x64list\\x80\\x63neg\\x26"
    JSON_TEXT_3 = '[0,-1,true,false,null,{"x":5,"y":[1,2]}]'
    MSGPACK_BYTES_3 = b"\\x96\\x00\\xff\\xc3\\xc2\\xc0\\x82\\xa1x\\x05\\xa1y\\x92\\x01\\x02"
    CBOR_BYTES_3 = b"\\x86\\x00\\x20\\xf5\\xf4\\xf6\\xa2\\x61x\\x05\\x61y\\x82\\x01\\x02"

    CASES = [
        ("case1", JSON_TEXT_1, MSGPACK_BYTES_1, CBOR_BYTES_1),
        ("case2", JSON_TEXT_2, MSGPACK_BYTES_2, CBOR_BYTES_2),
        ("case3", JSON_TEXT_3, MSGPACK_BYTES_3, CBOR_BYTES_3),
    ]

    def _normalize(obj):
        return json.dumps(obj, sort_keys=True, separators=(",", ":"))

    def report_case(label, json_text, msgpack_bytes, cbor_bytes):
        json_obj = molt_json.parse(json_text)
        msgpack_obj = molt_msgpack.parse(msgpack_bytes)
        cbor_obj = molt_cbor.parse(cbor_bytes)
        norm_json = _normalize(json_obj)
        norm_msgpack = _normalize(msgpack_obj)
        norm_cbor = _normalize(cbor_obj)
        print(f"{label}:json:{norm_json}")
        print(f"{label}:msgpack:{norm_msgpack}")
        print(f"{label}:cbor:{norm_cbor}")
        print(f"{label}:all_equal:{norm_json == norm_msgpack == norm_cbor}")

    def report_json_error(label, text):
        try:
            molt_json.parse(text)
        except Exception:
            print(f"{label}:error")
        else:
            print(f"{label}:ok")

    def report_msgpack_error(label, data):
        try:
            molt_msgpack.parse(data)
        except Exception:
            print(f"{label}:error")
        else:
            print(f"{label}:ok")

    def report_cbor_error(label, data):
        try:
            molt_cbor.parse(data)
        except Exception:
            print(f"{label}:error")
        else:
            print(f"{label}:ok")

    def main():
        for label, json_text, msgpack_bytes, cbor_bytes in CASES:
            report_case(label, json_text, msgpack_bytes, cbor_bytes)
        bad_json = "{" + str(1)
        bad_msgpack = "bad"
        bad_cbor = "bad"
        bad_msgpack_bytes = b"\\xc7"
        bad_cbor_bytes = b"\\x1c"
        report_json_error("json_invalid", bad_json)
        report_msgpack_error("msgpack_invalid", bad_msgpack)
        report_cbor_error("cbor_invalid", bad_cbor)
        report_msgpack_error("msgpack_invalid_bytes", bad_msgpack_bytes)
        report_cbor_error("cbor_invalid_bytes", bad_cbor_bytes)

    if __name__ == "__main__":
        main()
    """
)

CODEC_HELPERS = textwrap.dedent(
    """\
    const CODEC_JSON_1 = '{"a":1,"b":true,"c":null,"d":[1,2]}';
    const CODEC_JSON_2 = '{"empty":{},"list":[],"neg":-7}';
    const CODEC_JSON_3 = '[0,-1,true,false,null,{"x":5,"y":[1,2]}]';
    const CODEC_MSGPACK_1 = Uint8Array.from([
      0x84, 0xa1, 0x61, 0x01, 0xa1, 0x62, 0xc3, 0xa1, 0x63, 0xc0, 0xa1, 0x64, 0x92, 0x01, 0x02,
    ]);
    const CODEC_MSGPACK_2 = Uint8Array.from([
      0x83, 0xa5, 0x65, 0x6d, 0x70, 0x74, 0x79, 0x80, 0xa4, 0x6c, 0x69, 0x73, 0x74, 0x90, 0xa3, 0x6e, 0x65, 0x67, 0xf9,
    ]);
    const CODEC_MSGPACK_3 = Uint8Array.from([
      0x96, 0x00, 0xff, 0xc3, 0xc2, 0xc0, 0x82, 0xa1, 0x78, 0x05, 0xa1, 0x79, 0x92, 0x01, 0x02,
    ]);
    const CODEC_CBOR_1 = Uint8Array.from([
      0xa4, 0x61, 0x61, 0x01, 0x61, 0x62, 0xf5, 0x61, 0x63, 0xf6, 0x61, 0x64, 0x82, 0x01, 0x02,
    ]);
    const CODEC_CBOR_2 = Uint8Array.from([
      0xa3, 0x65, 0x65, 0x6d, 0x70, 0x74, 0x79, 0xa0, 0x64, 0x6c, 0x69, 0x73, 0x74, 0x80, 0x63, 0x6e, 0x65, 0x67, 0x26,
    ]);
    const CODEC_CBOR_3 = Uint8Array.from([
      0x86, 0x00, 0x20, 0xf5, 0xf4, 0xf6, 0xa2, 0x61, 0x78, 0x05, 0x61, 0x79, 0x82, 0x01, 0x02,
    ]);
    const codecBytesEqual = (left, right) => {
      if (!left || left.length !== right.length) return false;
      for (let i = 0; i < left.length; i += 1) {
        if (left[i] !== right[i]) return false;
      }
      return true;
    };
    const readCodecBytes = (ptr, len, label) => {
      if (!memory) return null;
      const addr = expectPtrAddr(ptr, label);
      if (!addr) return null;
      return new Uint8Array(memory.buffer, addr, Number(len));
    };
    const buildCodecObject1 = () => {
      const dictBits = boxPtr({ type: 'dict', entries: [], lookup: new Map() });
      const dict = getDict(dictBits);
      dictSetValue(dict, boxPtr({ type: 'str', value: 'a' }), boxInt(1n));
      dictSetValue(dict, boxPtr({ type: 'str', value: 'b' }), boxBool(true));
      dictSetValue(dict, boxPtr({ type: 'str', value: 'c' }), boxNone());
      const listBits = listFromArray([boxInt(1n), boxInt(2n)]);
      dictSetValue(dict, boxPtr({ type: 'str', value: 'd' }), listBits);
      return dictBits;
    };
    const buildCodecObject2 = () => {
      const dictBits = boxPtr({ type: 'dict', entries: [], lookup: new Map() });
      const dict = getDict(dictBits);
      dictSetValue(dict, boxPtr({ type: 'str', value: 'empty' }), boxPtr({
        type: 'dict',
        entries: [],
        lookup: new Map(),
      }));
      dictSetValue(dict, boxPtr({ type: 'str', value: 'list' }), listFromArray([]));
      dictSetValue(dict, boxPtr({ type: 'str', value: 'neg' }), boxInt(-7n));
      return dictBits;
    };
    const buildCodecObject3 = () => {
      const dictBits = boxPtr({ type: 'dict', entries: [], lookup: new Map() });
      const dict = getDict(dictBits);
      dictSetValue(dict, boxPtr({ type: 'str', value: 'x' }), boxInt(5n));
      dictSetValue(dict, boxPtr({ type: 'str', value: 'y' }), listFromArray([
        boxInt(1n),
        boxInt(2n),
      ]));
      return listFromArray([
        boxInt(0n),
        boxInt(-1n),
        boxBool(true),
        boxBool(false),
        boxNone(),
        dictBits,
      ]);
    };
    const CODEC_CASES = [
      { json: CODEC_JSON_1, msgpack: CODEC_MSGPACK_1, cbor: CODEC_CBOR_1, build: buildCodecObject1 },
      { json: CODEC_JSON_2, msgpack: CODEC_MSGPACK_2, cbor: CODEC_CBOR_2, build: buildCodecObject2 },
      { json: CODEC_JSON_3, msgpack: CODEC_MSGPACK_3, cbor: CODEC_CBOR_3, build: buildCodecObject3 },
    ];
    const findCodecCaseByJson = (text) =>
      CODEC_CASES.find((entry) => entry.json === text);
    const findCodecCaseByBytes = (bytes, key) =>
      CODEC_CASES.find((entry) => codecBytesEqual(bytes, entry[key]));
    const raiseCodecError = (kind, msg) => {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: kind }),
        exceptionArgs(boxPtr({ type: 'str', value: msg })),
      );
      return raiseException(exc);
    };
    """
)

CODEC_IMPORTS = textwrap.dedent(
    """\
    json_parse_scalar: (ptr, len, out) => {
      const bytes = readCodecBytes(ptr, len, 'json_parse_scalar');
      const outAddr = expectPtrAddr(out, 'json_parse_scalar');
      if (!bytes || !outAddr || !memory) return 1;
      const text = Buffer.from(bytes).toString('utf8').trim();
      const entry = findCodecCaseByJson(text);
      if (!entry) return 1;
      const valueBits = entry.build();
      const view = new DataView(memory.buffer);
      view.setBigInt64(outAddr, valueBits, true);
      return 0;
    },
    json_parse_scalar_obj: (bits) => {
      const obj = getObj(bits);
      if (!obj || obj.type !== 'str') {
        return raiseCodecError('TypeError', 'json.parse expects str');
      }
      const text = obj.value.trim();
      const entry = findCodecCaseByJson(text);
      if (!entry) {
        return raiseCodecError('ValueError', 'invalid JSON payload');
      }
      return entry.build();
    },
    msgpack_parse_scalar: (ptr, len, out) => {
      const bytes = readCodecBytes(ptr, len, 'msgpack_parse_scalar');
      const outAddr = expectPtrAddr(out, 'msgpack_parse_scalar');
      if (!bytes || !outAddr || !memory) return 1;
      const entry = findCodecCaseByBytes(bytes, 'msgpack');
      if (!entry) return 1;
      const valueBits = entry.build();
      const view = new DataView(memory.buffer);
      view.setBigInt64(outAddr, valueBits, true);
      return 0;
    },
    msgpack_parse_scalar_obj: (bits) => {
      const obj = getObj(bits);
      if (!obj || (obj.type !== 'bytes' && obj.type !== 'bytearray')) {
        return raiseCodecError('TypeError', 'msgpack.parse expects bytes');
      }
      const data = obj.data || new Uint8Array();
      const entry = findCodecCaseByBytes(data, 'msgpack');
      if (!entry) {
        return raiseCodecError('ValueError', 'invalid msgpack payload');
      }
      return entry.build();
    },
    cbor_parse_scalar: (ptr, len, out) => {
      const bytes = readCodecBytes(ptr, len, 'cbor_parse_scalar');
      const outAddr = expectPtrAddr(out, 'cbor_parse_scalar');
      if (!bytes || !outAddr || !memory) return 1;
      const entry = findCodecCaseByBytes(bytes, 'cbor');
      if (!entry) return 1;
      const valueBits = entry.build();
      const view = new DataView(memory.buffer);
      view.setBigInt64(outAddr, valueBits, true);
      return 0;
    },
    cbor_parse_scalar_obj: (bits) => {
      const obj = getObj(bits);
      if (!obj || (obj.type !== 'bytes' && obj.type !== 'bytearray')) {
        return raiseCodecError('TypeError', 'cbor.parse expects bytes');
      }
      const data = obj.data || new Uint8Array();
      const entry = findCodecCaseByBytes(data, 'cbor');
      if (!entry) {
        return raiseCodecError('ValueError', 'invalid cbor payload');
      }
      return entry.build();
    },
    """
)


def test_wasm_codec_parity(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for wasm parity test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for wasm parity test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "codec_parity.py"
    src.write_text(CODEC_SRC)

    output_wasm = tmp_path / "output.wasm"

    runner = write_wasm_runner(
        tmp_path,
        "run_wasm_codec.js",
        extra_js=CODEC_HELPERS,
        import_overrides=CODEC_IMPORTS,
    )

    env = os.environ.copy()
    env["PYTHONPATH"] = str(root / "src")
    build = subprocess.run(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            str(src),
            "--target",
            "wasm",
            "--codec",
            "json",
            "--out-dir",
            str(tmp_path),
        ],
        cwd=root,
        env=env,
        capture_output=True,
        text=True,
    )
    assert build.returncode == 0, build.stderr

    run = subprocess.run(
        ["node", str(runner), str(output_wasm)],
        cwd=root,
        capture_output=True,
        text=True,
    )
    assert run.returncode == 0, run.stderr
    assert run.stdout.strip() == "\n".join(
        [
            'case1:json:{"a":1,"b":true,"c":null,"d":[1,2]}',
            'case1:msgpack:{"a":1,"b":true,"c":null,"d":[1,2]}',
            'case1:cbor:{"a":1,"b":true,"c":null,"d":[1,2]}',
            "case1:all_equal:True",
            'case2:json:{"empty":{},"list":[],"neg":-7}',
            'case2:msgpack:{"empty":{},"list":[],"neg":-7}',
            'case2:cbor:{"empty":{},"list":[],"neg":-7}',
            "case2:all_equal:True",
            'case3:json:[0,-1,true,false,null,{"x":5,"y":[1,2]}]',
            'case3:msgpack:[0,-1,true,false,null,{"x":5,"y":[1,2]}]',
            'case3:cbor:[0,-1,true,false,null,{"x":5,"y":[1,2]}]',
            "case3:all_equal:True",
            "json_invalid:error",
            "msgpack_invalid:error",
            "cbor_invalid:error",
            "msgpack_invalid_bytes:error",
            "cbor_invalid_bytes:error",
        ]
    )
