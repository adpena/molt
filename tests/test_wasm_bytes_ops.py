import os
import shutil
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest

from tests.wasm_harness import write_wasm_runner

BYTES_HELPERS = textwrap.dedent(
    """\
    const bytesFromArray = (items, type) =>
      boxPtr({ type, data: Uint8Array.from(items) });
    const concatBytes = (left, right) => {
      const out = new Uint8Array(left.length + right.length);
      out.set(left, 0);
      out.set(right, left.length);
      return out;
    };
    const findBytes = (hay, needle) => {
      if (needle.length === 0) return 0;
      for (let i = 0; i + needle.length <= hay.length; i += 1) {
        let ok = true;
        for (let j = 0; j < needle.length; j += 1) {
          if (hay[i + j] !== needle[j]) {
            ok = false;
            break;
          }
        }
        if (ok) return i;
      }
      return -1;
    };
    const splitBytes = (hay, needle) => {
      const parts = [];
      let start = 0;
      let i = 0;
      while (i + needle.length <= hay.length) {
        let match = true;
        for (let j = 0; j < needle.length; j += 1) {
          if (hay[i + j] !== needle[j]) {
            match = false;
            break;
          }
        }
        if (match) {
          parts.push(hay.slice(start, i));
          i += needle.length;
          start = i;
        } else {
          i += 1;
        }
      }
      parts.push(hay.slice(start));
      return parts;
    };
    const replaceBytes = (hay, needle, repl) => {
      const out = [];
      let i = 0;
      while (i + needle.length <= hay.length) {
        let match = true;
        for (let j = 0; j < needle.length; j += 1) {
          if (hay[i + j] !== needle[j]) {
            match = false;
            break;
          }
        }
        if (match) {
          for (const b of repl) out.push(b);
          i += needle.length;
        } else {
          out.push(hay[i]);
          i += 1;
        }
      }
      for (; i < hay.length; i += 1) out.push(hay[i]);
      return Uint8Array.from(out);
    };
    """
)

BYTES_IMPORT_OVERRIDES = textwrap.dedent(
    """\
    print_obj: (val) => {
      if (isTag(val, TAG_INT)) {
        console.log(unboxInt(val).toString());
        return;
      }
      if (isTag(val, TAG_BOOL)) {
        console.log((val & 1n) === 1n ? 'True' : 'False');
        return;
      }
      if (isTag(val, TAG_NONE)) {
        console.log('None');
        return;
      }
      console.log(val.toString());
    },
    print_newline: () => console.log(''),
    alloc: () => 0n,
    async_sleep: () => 0n,
    block_on: () => 0n,
    add: (a, b) => {
      if (isTag(a, TAG_INT) && isTag(b, TAG_INT)) {
        return boxInt(unboxInt(a) + unboxInt(b));
      }
      if (isPtr(a) && isPtr(b)) {
        const left = getObj(a);
        const right = getObj(b);
        if (left && right && left.type === 'bytes' && right.type === 'bytes') {
          return boxPtr({ type: 'bytes', data: concatBytes(left.data, right.data) });
        }
        if (left && right && left.type === 'bytearray' && right.type === 'bytearray') {
          return boxPtr({ type: 'bytearray', data: concatBytes(left.data, right.data) });
        }
      }
      return boxNone();
    },
    vec_sum_int: () => boxNone(),
    vec_sum_int_trusted: () => boxNone(),
    vec_sum_int_range: () => boxNone(),
    vec_sum_int_range_trusted: () => boxNone(),
    vec_prod_int: () => boxNone(),
    vec_prod_int_trusted: () => boxNone(),
    vec_prod_int_range: () => boxNone(),
    vec_prod_int_range_trusted: () => boxNone(),
    vec_min_int: () => boxNone(),
    vec_min_int_trusted: () => boxNone(),
    vec_min_int_range: () => boxNone(),
    vec_min_int_range_trusted: () => boxNone(),
    vec_max_int: () => boxNone(),
    vec_max_int_trusted: () => boxNone(),
    vec_max_int_range: () => boxNone(),
    vec_max_int_range_trusted: () => boxNone(),
    sub: (a, b) => boxInt(unboxInt(a) - unboxInt(b)),
    mul: (a, b) => boxInt(unboxInt(a) * unboxInt(b)),
    lt: (a, b) => boxBool(unboxInt(a) < unboxInt(b)),
    eq: (a, b) => boxBool(a === b),
    is: (a, b) => boxBool(a === b),
    not: (val) => {
      if (isTag(val, TAG_BOOL)) return boxBool((val & 1n) !== 1n);
      if (isTag(val, TAG_INT)) return boxBool(unboxInt(val) === 0n);
      if (isPtr(val)) {
        const obj = getObj(val);
        if (obj && obj.type === 'list') return boxBool(obj.items.length === 0);
        if (obj && obj.type === 'bytes') return boxBool(obj.data.length === 0);
        if (obj && obj.type === 'bytearray') return boxBool(obj.data.length === 0);
      }
      return boxBool(true);
    },
    contains: (container, item) => {
      const list = getList(container);
      if (list) {
        return boxBool(list.items.some((val) => val === item));
      }
      const bytes = getBytes(container);
      if (bytes) {
        if (isTag(item, TAG_INT)) {
          const needle = Number(unboxInt(item));
          return boxBool(bytes.data.includes(needle));
        }
      }
      const bytearray = getBytearray(container);
      if (bytearray) {
        if (isTag(item, TAG_INT)) {
          const needle = Number(unboxInt(item));
          return boxBool(bytearray.data.includes(needle));
        }
      }
      return boxBool(false);
    },
    guard_type: (val, expected) => val,
    get_attr_generic: () => boxNone(),
    get_attr_object: () => boxNone(),
    set_attr_generic: () => boxNone(),
    set_attr_object: () => boxNone(),
    get_attr_name: () => boxNone(),
    get_attr_name_default: () => boxNone(),
    has_attr_name: () => boxBool(false),
    set_attr_name: () => boxNone(),
    is_truthy: (val) => {
      if (isTag(val, TAG_BOOL)) return (val & 1n) === 1n ? 1n : 0n;
      if (isTag(val, TAG_INT)) return unboxInt(val) !== 0n ? 1n : 0n;
      if (isPtr(val)) {
        const obj = getObj(val);
        if (obj && obj.type === 'list') return obj.items.length ? 1n : 0n;
        if (obj && obj.type === 'bytes') return obj.data.length ? 1n : 0n;
        if (obj && obj.type === 'bytearray') return obj.data.length ? 1n : 0n;
      }
      return 0n;
    },
    json_parse_scalar: () => 0,
    msgpack_parse_scalar: () => 0,
    cbor_parse_scalar: () => 0,
    string_from_bytes: (ptr, len, out) => {
      const view = new DataView(memory.buffer);
      const bytes = new Uint8Array(memory.buffer, Number(ptr), Number(len));
      const boxed = boxPtr({ type: 'string', data: bytes.slice() });
      view.setBigInt64(Number(out), boxed, true);
      return 0;
    },
    bytes_from_bytes: (ptr, len, out) => {
      const view = new DataView(memory.buffer);
      const bytes = new Uint8Array(memory.buffer, Number(ptr), Number(len));
      const boxed = boxPtr({ type: 'bytes', data: bytes.slice() });
      view.setBigInt64(Number(out), boxed, true);
      return 0;
    },
    memoryview_new: () => boxNone(),
    memoryview_tobytes: () => boxNone(),
    str_from_obj: () => boxNone(),
    len: (val) => {
      const list = getList(val);
      if (list) return boxInt(list.items.length);
      const bytes = getBytes(val);
      if (bytes) return boxInt(bytes.data.length);
      const ba = getBytearray(val);
      if (ba) return boxInt(ba.data.length);
      return boxInt(0);
    },
    slice: () => boxNone(),
    slice_new: (startBits, stopBits, stepBits) => {
      return boxPtr({ type: 'slice', startBits, stopBits, stepBits });
    },
    range_new: () => boxNone(),
    list_builder_new: () => boxPtr({ type: 'list_builder', items: [] }),
    list_builder_append: (builder, val) => {
      const obj = getObj(builder);
      if (obj) obj.items.push(val);
    },
    list_builder_finish: (builder) => {
      const obj = getObj(builder);
      if (!obj) return boxNone();
      return boxPtr({ type: 'list', items: obj.items.slice() });
    },
    tuple_builder_finish: () => boxNone(),
    list_append: () => boxNone(),
    list_pop: () => boxNone(),
    list_extend: () => boxNone(),
    list_insert: () => boxNone(),
    list_remove: () => boxNone(),
    list_count: () => boxNone(),
    list_index: () => boxNone(),
    dict_new: () => boxNone(),
    dict_set: () => boxNone(),
    dict_get: () => boxNone(),
    dict_pop: () => boxNone(),
    dict_keys: () => boxNone(),
    dict_values: () => boxNone(),
    dict_items: () => boxNone(),
    tuple_count: () => boxNone(),
    tuple_index: () => boxNone(),
    iter: () => boxNone(),
    iter_next: () => boxNone(),
    index: (obj, key) => {
      const list = getList(obj);
      if (!list) return boxNone();
      let idx = Number(unboxInt(key));
      if (idx < 0) idx += list.items.length;
      if (idx < 0 || idx >= list.items.length) return boxNone();
      return list.items[idx];
    },
    store_index: () => boxNone(),
    bytes_find: (hay, needle) => {
      const h = getBytes(hay);
      const n = getBytes(needle) || getBytearray(needle);
      if (!h || !n) return boxInt(-1);
      return boxInt(findBytes(h.data, n.data));
    },
    bytearray_find: (hay, needle) => {
      const h = getBytearray(hay);
      const n = getBytes(needle) || getBytearray(needle);
      if (!h || !n) return boxInt(-1);
      return boxInt(findBytes(h.data, n.data));
    },
    string_find: () => boxInt(-1),
    string_format: () => boxNone(),
    string_startswith: () => boxBool(false),
    string_endswith: () => boxBool(false),
    string_count: () => boxInt(0),
    string_join: () => boxNone(),
    string_split: () => boxNone(),
    bytes_split: (hay, needle) => {
      const h = getBytes(hay);
      const n = getBytes(needle) || getBytearray(needle);
      if (!h || !n) return boxNone();
      const parts = splitBytes(h.data, n.data).map((part) =>
        boxPtr({ type: 'bytes', data: part })
      );
      return boxPtr({ type: 'list', items: parts });
    },
    bytearray_split: (hay, needle) => {
      const h = getBytearray(hay);
      const n = getBytes(needle) || getBytearray(needle);
      if (!h || !n) return boxNone();
      const parts = splitBytes(h.data, n.data).map((part) =>
        boxPtr({ type: 'bytearray', data: part })
      );
      return boxPtr({ type: 'list', items: parts });
    },
    string_replace: () => boxNone(),
    bytes_replace: (hay, needle, repl) => {
      const h = getBytes(hay);
      const n = getBytes(needle) || getBytearray(needle);
      const r = getBytes(repl) || getBytearray(repl);
      if (!h || !n || !r) return boxNone();
      return boxPtr({ type: 'bytes', data: replaceBytes(h.data, n.data, r.data) });
    },
    bytearray_replace: (hay, needle, repl) => {
      const h = getBytearray(hay);
      const n = getBytes(needle) || getBytearray(needle);
      const r = getBytes(repl) || getBytearray(repl);
      if (!h || !n || !r) return boxNone();
      return boxPtr({ type: 'bytearray', data: replaceBytes(h.data, n.data, r.data) });
    },
    bytearray_from_obj: (src) => {
      const bytes = getBytes(src);
      if (!bytes) return boxNone();
      return boxPtr({ type: 'bytearray', data: bytes.data.slice() });
    },
    intarray_from_seq: () => boxNone(),
    buffer2d_new: () => boxNone(),
    buffer2d_get: () => boxNone(),
    buffer2d_set: () => boxNone(),
    buffer2d_matmul: () => boxNone(),
    dataclass_new: () => boxNone(),
    dataclass_get: () => boxNone(),
    dataclass_set: () => boxNone(),
    dataclass_set_class: () => boxNone(),
    class_new: () => boxNone(),
    classmethod_new: () => boxNone(),
    staticmethod_new: () => boxNone(),
    property_new: () => boxNone(),
    object_set_class: () => boxNone(),
    context_null: (val) => val,
    context_enter: (val) => val,
    context_exit: () => boxBool(false),
    context_unwind: () => boxBool(false),
    context_depth: () => boxInt(0),
    context_unwind_to: () => boxNone(),
    exception_push: () => boxNone(),
    exception_pop: () => boxNone(),
    exception_last: () => boxNone(),
    exception_new: () => boxNone(),
    exception_clear: () => boxNone(),
    exception_pending: () => 0n,
    exception_kind: () => boxNone(),
    exception_message: () => boxNone(),
    exception_set_cause: () => boxNone(),
    raise: () => boxNone(),
    context_closing: (val) => val,
    bridge_unavailable: () => boxNone(),
    file_open: () => boxNone(),
    file_read: () => boxNone(),
    file_write: () => boxNone(),
    file_close: () => boxNone(),
    stream_new: () => 0n,
    stream_send: () => 0n,
    stream_recv: () => 0n,
    stream_close: () => {},
    ws_connect: () => 0,
    ws_pair: () => 0,
    ws_send: () => 0n,
    ws_recv: () => 0n,
    ws_close: () => {},
    """
)


def test_wasm_bytes_ops_parity(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for wasm parity test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for wasm parity test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "bytes_ops.py"
    src.write_text(
        "b = b'one,two'\n"
        "print(len(b))\n"
        "print((b + b'!').find(b'two'))\n"
        "parts = b.split(b',')\n"
        "print(len(parts))\n"
        "print(len(parts[0]))\n"
        "print(len(parts[1]))\n"
        "print(b.replace(b'one', b'uno').find(b'uno'))\n"
        "ba = bytearray(b'one,two')\n"
        "print(len(ba))\n"
        "print(ba.find(b'two'))\n"
        "parts2 = ba.split(b',')\n"
        "print(len(parts2))\n"
        "print(len(parts2[0]))\n"
        "print(len(parts2[1]))\n"
        "print(ba.replace(b'two', b'dos').find(b'dos'))\n"
        "print((ba + bytearray(b'!')).find(b'!'))\n"
    )

    output_wasm = root / "output.wasm"
    existed = output_wasm.exists()

    runner = write_wasm_runner(
        tmp_path,
        "run_wasm_bytes.js",
        extra_js=BYTES_HELPERS,
        import_overrides=BYTES_IMPORT_OVERRIDES,
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
        assert run.stdout.strip() == "7\n4\n2\n3\n3\n0\n7\n4\n2\n3\n3\n4\n7"
    finally:
        if not existed and output_wasm.exists():
            output_wasm.unlink()
