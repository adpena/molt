import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest


def test_wasm_if_else_parity(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for wasm parity test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for wasm parity test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "if_else.py"
    src.write_text("x = 1\nif x < 2:\n    print(1)\nelse:\n    print(2)\n")

    output_wasm = root / "output.wasm"
    existed = output_wasm.exists()

    runner = tmp_path / "run_wasm_if.js"
    runner.write_text(
        "const fs = require('fs');\n"
        "const wasmPath = process.argv[2];\n"
        "const wasmBuffer = fs.readFileSync(wasmPath);\n"
        "const QNAN = 0x7ff8000000000000n;\n"
        "const TAG_INT = 0x0001000000000000n;\n"
        "const TAG_BOOL = 0x0002000000000000n;\n"
        "const TAG_NONE = 0x0003000000000000n;\n"
        "const TAG_MASK = 0x0007000000000000n;\n"
        "const INT_SIGN_BIT = 1n << 46n;\n"
        "const INT_WIDTH = 47n;\n"
        "const INT_MASK = (1n << INT_WIDTH) - 1n;\n"
        "const isTag = (val, tag) => (val & (QNAN | TAG_MASK)) === (QNAN | tag);\n"
        "const unboxInt = (val) => {\n"
        "  let v = val & INT_MASK;\n"
        "  if ((v & INT_SIGN_BIT) !== 0n) {\n"
        "    v = v - (1n << INT_WIDTH);\n"
        "  }\n"
        "  return v;\n"
        "};\n"
        "const boxInt = (n) => {\n"
        "  const v = BigInt(n) & INT_MASK;\n"
        "  return QNAN | TAG_INT | v;\n"
        "};\n"
        "const boxBool = (b) => QNAN | TAG_BOOL | (b ? 1n : 0n);\n"
        "const imports = {\n"
        "  molt_runtime: {\n"
        "    print_obj: (val) => {\n"
        "      if (isTag(val, TAG_INT)) {\n"
        "        console.log(unboxInt(val).toString());\n"
        "        return;\n"
        "      }\n"
        "      if (isTag(val, TAG_BOOL)) {\n"
        "        console.log((val & 1n) === 1n ? 'True' : 'False');\n"
        "        return;\n"
        "      }\n"
        "      if (isTag(val, TAG_NONE)) {\n"
        "        console.log('None');\n"
        "        return;\n"
        "      }\n"
        "      console.log(val.toString());\n"
        "    },\n"
        "    print_newline: () => console.log(''),\n"
        "    alloc: () => 0n,\n"
        "    async_sleep: () => 0n,\n"
        "    block_on: () => 0n,\n"
        "    add: (a, b) => boxInt(unboxInt(a) + unboxInt(b)),\n"
        "    vec_sum_int: () => 0n,\n"
        "    sub: (a, b) => boxInt(unboxInt(a) - unboxInt(b)),\n"
        "    mul: (a, b) => boxInt(unboxInt(a) * unboxInt(b)),\n"
        "    lt: (a, b) => boxBool(unboxInt(a) < unboxInt(b)),\n"
        "    eq: (a, b) => boxBool(a === b),\n"
        "    guard_type: (val, expected) => val,\n"
        "    is_truthy: (val) => {\n"
        "      if (isTag(val, TAG_BOOL)) {\n"
        "        return (val & 1n) === 1n ? 1n : 0n;\n"
        "      }\n"
        "      if (isTag(val, TAG_INT)) {\n"
        "        return unboxInt(val) !== 0n ? 1n : 0n;\n"
        "      }\n"
        "      return 0n;\n"
        "    },\n"
        "    json_parse_scalar: () => 0,\n"
        "    msgpack_parse_scalar: () => 0,\n"
        "    cbor_parse_scalar: () => 0,\n"
        "    string_from_bytes: () => 0,\n"
        "    bytes_from_bytes: () => 0,\n"
        "    str_from_obj: () => 0n,\n"
        "    len: () => 0n,\n"
        "    slice: () => 0n,\n"
        "    slice_new: () => 0n,\n"
        "    range_new: () => 0n,\n"
        "    list_builder_new: () => 0n,\n"
        "    list_builder_append: () => {},\n"
        "    list_builder_finish: () => 0n,\n"
        "    tuple_builder_finish: () => 0n,\n"
        "    list_append: () => 0n,\n"
        "    list_pop: () => 0n,\n"
        "    list_extend: () => 0n,\n"
        "    list_insert: () => 0n,\n"
        "    list_remove: () => 0n,\n"
        "    list_count: () => 0n,\n"
        "    list_index: () => 0n,\n"
        "    dict_new: () => 0n,\n"
        "    dict_set: () => 0n,\n"
        "    dict_get: () => 0n,\n"
        "    dict_pop: () => 0n,\n"
        "    dict_keys: () => 0n,\n"
        "    dict_values: () => 0n,\n"
        "    dict_items: () => 0n,\n"
        "    tuple_count: () => 0n,\n"
        "    tuple_index: () => 0n,\n"
        "    iter: () => 0n,\n"
        "    iter_next: () => 0n,\n"
        "    index: () => 0n,\n"
        "    store_index: () => 0n,\n"
        "    bytes_find: () => 0n,\n"
        "    bytearray_find: () => 0n,\n"
        "    string_find: () => 0n,\n"
        "    string_format: () => 0n,\n"
        "    string_startswith: () => 0n,\n"
        "    string_endswith: () => 0n,\n"
        "    string_count: () => 0n,\n"
        "    string_join: () => 0n,\n"
        "    string_split: () => 0n,\n"
        "    bytes_split: () => 0n,\n"
        "    bytearray_split: () => 0n,\n"
        "    string_replace: () => 0n,\n"
        "    bytes_replace: () => 0n,\n"
        "    bytearray_replace: () => 0n,\n"
        "    bytearray_from_obj: () => 0n,\n"
        "    buffer2d_new: () => 0n,\n"
        "    buffer2d_get: () => 0n,\n"
        "    buffer2d_set: () => 0n,\n"
        "    buffer2d_matmul: () => 0n,\n"
        "    dataclass_new: () => 0n,\n"
        "    dataclass_get: () => 0n,\n"
        "    dataclass_set: () => 0n,\n"
        "    stream_new: () => 0n,\n"
        "    stream_send: () => 0n,\n"
        "    stream_recv: () => 0n,\n"
        "    stream_close: () => {},\n"
        "    ws_connect: () => 0,\n"
        "    ws_pair: () => 0,\n"
        "    ws_send: () => 0n,\n"
        "    ws_recv: () => 0n,\n"
        "    ws_close: () => {},\n"
        "  },\n"
        "};\n"
        "WebAssembly.instantiate(wasmBuffer, imports)\n"
        "  .then((mod) => mod.instance.exports.molt_main())\n"
        "  .catch((err) => {\n"
        "    console.error(err);\n"
        "    process.exit(1);\n"
        "  });\n"
    )

    env = os.environ.copy()
    env["PYTHONPATH"] = str(root / "src")
    build = subprocess.run(
        [sys.executable, "-m", "molt.cli", "build", str(src), "--target", "wasm"],
        cwd=root,
        env=env,
        capture_output=True,
        text=True,
    )
    assert build.returncode == 0, build.stderr

    try:
        run = subprocess.run(
            ["node", str(runner), str(output_wasm)],
            cwd=root,
            capture_output=True,
            text=True,
        )
        assert run.returncode == 0, run.stderr
        assert run.stdout.strip() == "1"
    finally:
        if not existed and output_wasm.exists():
            output_wasm.unlink()
