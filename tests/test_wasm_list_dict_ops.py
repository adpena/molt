import os
import shutil
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest

from tests.wasm_harness import write_wasm_runner

LIST_DICT_IMPORT_OVERRIDES = textwrap.dedent(
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
    add: (a, b) => boxInt(unboxInt(a) + unboxInt(b)),
    vec_sum_int: (seqBits, accBits) => {
      const list = getList(seqBits);
      if (!list || !isTag(accBits, TAG_INT)) {
        return listFromArray([boxNone(), boxBool(false)]);
      }
      let sum = unboxInt(accBits);
      for (const item of list.items) {
        if (!isTag(item, TAG_INT)) {
          return listFromArray([boxInt(sum), boxBool(false)]);
        }
        sum += unboxInt(item);
      }
      return listFromArray([boxInt(sum), boxBool(true)]);
    },
    vec_sum_int_trusted: (seqBits, accBits) => {
      const list = getList(seqBits);
      if (!list || !isTag(accBits, TAG_INT)) {
        return listFromArray([boxNone(), boxBool(false)]);
      }
      let sum = unboxInt(accBits);
      for (const item of list.items) {
        if (!isTag(item, TAG_INT)) {
          return listFromArray([boxInt(sum), boxBool(false)]);
        }
        sum += unboxInt(item);
      }
      return listFromArray([boxInt(sum), boxBool(true)]);
    },
    vec_sum_int_range: (seqBits, accBits, startBits) => {
      const list = getList(seqBits);
      if (!list || !isTag(accBits, TAG_INT) || !isTag(startBits, TAG_INT)) {
        return listFromArray([boxNone(), boxBool(false)]);
      }
      const start = Number(unboxInt(startBits));
      if (start < 0) {
        return listFromArray([boxInt(unboxInt(accBits)), boxBool(false)]);
      }
      let sum = unboxInt(accBits);
      for (let i = start; i < list.items.length; i += 1) {
        const item = list.items[i];
        if (!isTag(item, TAG_INT)) {
          return listFromArray([boxInt(sum), boxBool(false)]);
        }
        sum += unboxInt(item);
      }
      return listFromArray([boxInt(sum), boxBool(true)]);
    },
    vec_sum_int_range_trusted: (seqBits, accBits, startBits) => {
      const list = getList(seqBits);
      if (!list || !isTag(accBits, TAG_INT) || !isTag(startBits, TAG_INT)) {
        return listFromArray([boxNone(), boxBool(false)]);
      }
      const start = Number(unboxInt(startBits));
      if (start < 0) {
        return listFromArray([boxInt(unboxInt(accBits)), boxBool(false)]);
      }
      let sum = unboxInt(accBits);
      for (let i = start; i < list.items.length; i += 1) {
        const item = list.items[i];
        if (!isTag(item, TAG_INT)) {
          return listFromArray([boxInt(sum), boxBool(false)]);
        }
        sum += unboxInt(item);
      }
      return listFromArray([boxInt(sum), boxBool(true)]);
    },
    vec_prod_int: () => listFromArray([boxNone(), boxBool(false)]),
    vec_prod_int_trusted: () => listFromArray([boxNone(), boxBool(false)]),
    vec_prod_int_range: () => listFromArray([boxNone(), boxBool(false)]),
    vec_prod_int_range_trusted: () => listFromArray([boxNone(), boxBool(false)]),
    vec_min_int: () => listFromArray([boxNone(), boxBool(false)]),
    vec_min_int_trusted: () => listFromArray([boxNone(), boxBool(false)]),
    vec_min_int_range: () => listFromArray([boxNone(), boxBool(false)]),
    vec_min_int_range_trusted: () => listFromArray([boxNone(), boxBool(false)]),
    vec_max_int: () => listFromArray([boxNone(), boxBool(false)]),
    vec_max_int_trusted: () => listFromArray([boxNone(), boxBool(false)]),
    vec_max_int_range: () => listFromArray([boxNone(), boxBool(false)]),
    vec_max_int_range_trusted: () => listFromArray([boxNone(), boxBool(false)]),
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
        if (obj && obj.type === 'dict') return boxBool(obj.map.size === 0);
      }
      return boxBool(true);
    },
    contains: (container, item) => {
      const list = getList(container);
      if (list) {
        return boxBool(list.items.some((val) => val === item));
      }
      const dict = getDict(container);
      if (dict) {
        return boxBool(dict.map.has(item));
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
      if (isTag(val, TAG_BOOL)) {
        return (val & 1n) === 1n ? 1n : 0n;
      }
      if (isTag(val, TAG_INT)) {
        return unboxInt(val) !== 0n ? 1n : 0n;
      }
      if (isTag(val, TAG_NONE)) {
        return 0n;
      }
      if (isPtr(val)) {
        const obj = getObj(val);
        if (obj && obj.type === 'list') return obj.items.length ? 1n : 0n;
        if (obj && obj.type === 'dict') return obj.map.size ? 1n : 0n;
        if (obj && obj.type === 'dict_keys_view') return obj.dict.map.size ? 1n : 0n;
        if (obj && obj.type === 'dict_values_view') return obj.dict.map.size ? 1n : 0n;
        if (obj && obj.type === 'dict_items_view') return obj.dict.map.size ? 1n : 0n;
        if (obj && obj.type === 'iter') return 1n;
      }
      return 0n;
    },
    json_parse_scalar: () => 0,
    msgpack_parse_scalar: () => 0,
    cbor_parse_scalar: () => 0,
    string_from_bytes: () => 0,
    bytes_from_bytes: () => 0,
    memoryview_new: () => boxNone(),
    memoryview_tobytes: () => boxNone(),
    str_from_obj: () => 0n,
    len: (val) => {
      const list = getList(val);
      if (list) return boxInt(list.items.length);
      const dict = getDict(val);
      if (dict) return boxInt(dict.map.size);
      const view = getObj(val);
      if (view && (view.type === 'dict_keys_view' || view.type === 'dict_values_view' || view.type === 'dict_items_view')) {
        return boxInt(view.dict.map.size);
      }
      return boxInt(0);
    },
    slice: (obj, startBits, endBits) => {
      const list = getList(obj);
      if (!list) return boxNone();
      const len = list.items.length;
      let start = isTag(startBits, TAG_NONE) ? 0 : Number(unboxInt(startBits));
      let end = isTag(endBits, TAG_NONE) ? len : Number(unboxInt(endBits));
      if (start < 0) start += len;
      if (end < 0) end += len;
      if (start < 0) start = 0;
      if (end > len) end = len;
      if (end < start) end = start;
      return listFromArray(list.items.slice(start, end));
    },
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
      return listFromArray(obj.items.slice());
    },
    tuple_builder_finish: (builder) => {
      const obj = getObj(builder);
      if (!obj) return boxNone();
      return listFromArray(obj.items.slice());
    },
    list_append: (listBits, val) => {
      const list = getList(listBits);
      if (!list) return boxNone();
      list.items.push(val);
      return boxNone();
    },
    list_pop: (listBits, idxBits) => {
      const list = getList(listBits);
      if (!list || list.items.length === 0) return boxNone();
      let idx;
      if (isTag(idxBits, TAG_NONE)) {
        idx = list.items.length - 1;
      } else if (isTag(idxBits, TAG_INT)) {
        idx = Number(unboxInt(idxBits));
      } else {
        return boxNone();
      }
      if (idx < 0) idx += list.items.length;
      if (idx < 0 || idx >= list.items.length) return boxNone();
      return list.items.splice(idx, 1)[0];
    },
    list_extend: (listBits, otherBits) => {
      const list = getList(listBits);
      if (!list) return boxNone();
      const otherList = getList(otherBits);
      if (otherList) {
        list.items.push(...otherList.items);
        return boxNone();
      }
      const view = getObj(otherBits);
      if (view && (view.type === 'dict_keys_view' || view.type === 'dict_values_view' || view.type === 'dict_items_view')) {
        const keys = Array.from(view.dict.map.keys());
        const values = Array.from(view.dict.map.values());
        for (let i = 0; i < keys.length; i += 1) {
          if (view.type === 'dict_items_view') {
            list.items.push(listFromArray([keys[i], values[i]]));
          } else if (view.type === 'dict_keys_view') {
            list.items.push(keys[i]);
          } else {
            list.items.push(values[i]);
          }
        }
      }
      return boxNone();
    },
    list_insert: (listBits, idxBits, valBits) => {
      const list = getList(listBits);
      if (!list) return boxNone();
      let idx = Number(unboxInt(idxBits));
      if (idx < 0) idx += list.items.length;
      if (idx < 0) idx = 0;
      if (idx > list.items.length) idx = list.items.length;
      list.items.splice(idx, 0, valBits);
      return boxNone();
    },
    list_remove: (listBits, valBits) => {
      const list = getList(listBits);
      if (!list) return boxNone();
      const idx = list.items.findIndex((v) => v === valBits);
      if (idx >= 0) list.items.splice(idx, 1);
      return boxNone();
    },
    list_count: (listBits, valBits) => {
      const list = getList(listBits);
      if (!list) return boxInt(0);
      let count = 0;
      for (const item of list.items) {
        if (item === valBits) count += 1;
      }
      return boxInt(count);
    },
    list_index: (listBits, valBits) => {
      const list = getList(listBits);
      if (!list) return boxNone();
      const idx = list.items.indexOf(valBits);
      return idx >= 0 ? boxInt(idx) : boxNone();
    },
    dict_new: () => boxPtr({ type: 'dict', map: new Map() }),
    dict_set: (dictBits, keyBits, valBits) => {
      const dict = getDict(dictBits);
      if (!dict) return boxNone();
      dict.map.set(keyBits, valBits);
      return dictBits;
    },
    dict_get: (dictBits, keyBits, defaultBits) => {
      const dict = getDict(dictBits);
      if (!dict) return defaultBits;
      return dict.map.has(keyBits) ? dict.map.get(keyBits) : defaultBits;
    },
    dict_pop: (dictBits, keyBits, defaultBits, hasDefaultBits) => {
      const dict = getDict(dictBits);
      const hasDefault = unboxInt(hasDefaultBits) !== 0n;
      if (!dict) return hasDefault ? defaultBits : boxNone();
      if (dict.map.has(keyBits)) {
        const val = dict.map.get(keyBits);
        dict.map.delete(keyBits);
        return val;
      }
      return hasDefault ? defaultBits : boxNone();
    },
    dict_keys: (dictBits) => {
      const dict = getDict(dictBits);
      if (!dict) return boxNone();
      return boxPtr({ type: 'dict_keys_view', dict });
    },
    dict_values: (dictBits) => {
      const dict = getDict(dictBits);
      if (!dict) return boxNone();
      return boxPtr({ type: 'dict_values_view', dict });
    },
    dict_items: (dictBits) => {
      const dict = getDict(dictBits);
      if (!dict) return boxNone();
      return boxPtr({ type: 'dict_items_view', dict });
    },
    tuple_count: (tupleBits, valBits) => {
      const list = getList(tupleBits);
      if (!list) return boxInt(0);
      let count = 0;
      for (const item of list.items) {
        if (item === valBits) count += 1;
      }
      return boxInt(count);
    },
    tuple_index: (tupleBits, valBits) => {
      const list = getList(tupleBits);
      if (!list) return boxNone();
      const idx = list.items.indexOf(valBits);
      return idx >= 0 ? boxInt(idx) : boxNone();
    },
    iter: (objBits) => {
      const obj = getObj(objBits);
      if (obj && obj.type === 'dict') {
        const view = { type: 'dict_keys_view', dict: obj };
        return boxPtr({ type: 'iter', target: view, idx: 0 });
      }
      return boxPtr({ type: 'iter', target: obj, idx: 0 });
    },
    iter_next: (iterBits) => {
      const iter = getObj(iterBits);
      if (!iter || iter.type !== 'iter') return listFromArray([boxNone(), boxBool(true)]);
      const target = iter.target;
      let items = [];
      if (target && target.type === 'list') {
        items = target.items;
      } else if (target && (target.type === 'dict_keys_view' || target.type === 'dict_values_view' || target.type === 'dict_items_view')) {
        const keys = Array.from(target.dict.map.keys());
        const values = Array.from(target.dict.map.values());
        for (let i = 0; i < keys.length; i += 1) {
          if (target.type === 'dict_items_view') {
            items.push(listFromArray([keys[i], values[i]]));
          } else if (target.type === 'dict_keys_view') {
            items.push(keys[i]);
          } else {
            items.push(values[i]);
          }
        }
      }
      if (iter.idx >= items.length) {
        return listFromArray([boxNone(), boxBool(true)]);
      }
      const value = items[iter.idx];
      iter.idx += 1;
      return listFromArray([value, boxBool(false)]);
    },
    index: (obj, key) => {
      const list = getList(obj);
      if (list) {
        let idx = Number(unboxInt(key));
        if (idx < 0) idx += list.items.length;
        if (idx < 0 || idx >= list.items.length) return boxNone();
        return list.items[idx];
      }
      const dict = getDict(obj);
      if (dict) {
        return dict.map.has(key) ? dict.map.get(key) : boxNone();
      }
      const view = getObj(obj);
      if (view && (view.type === 'dict_keys_view' || view.type === 'dict_values_view' || view.type === 'dict_items_view')) {
        const keys = Array.from(view.dict.map.keys());
        const values = Array.from(view.dict.map.values());
        let idx = Number(unboxInt(key));
        if (idx < 0) idx += keys.length;
        if (idx < 0 || idx >= keys.length) return boxNone();
        if (view.type === 'dict_items_view') {
          return listFromArray([keys[idx], values[idx]]);
        }
        return view.type === 'dict_keys_view' ? keys[idx] : values[idx];
      }
      return boxNone();
    },
    store_index: (obj, key, val) => {
      const list = getList(obj);
      if (list) {
        let idx = Number(unboxInt(key));
        if (idx < 0) idx += list.items.length;
        if (idx < 0 || idx >= list.items.length) return boxNone();
        list.items[idx] = val;
        return obj;
      }
      const dict = getDict(obj);
      if (dict) {
        dict.map.set(key, val);
        return obj;
      }
      return boxNone();
    },
    bytes_find: () => boxInt(-1),
    bytearray_find: () => boxInt(-1),
    string_find: () => boxInt(-1),
    string_format: () => boxNone(),
    string_startswith: () => boxBool(false),
    string_endswith: () => boxBool(false),
    string_count: () => boxInt(0),
    string_join: () => boxNone(),
    string_split: () => boxNone(),
    bytes_split: () => boxNone(),
    bytearray_split: () => boxNone(),
    string_replace: () => boxNone(),
    bytes_replace: () => boxNone(),
    bytearray_replace: () => boxNone(),
    bytearray_from_obj: () => boxNone(),
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


def test_wasm_list_dict_ops_parity(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for wasm parity test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for wasm parity test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "list_dict_ops.py"
    src.write_text(
        "lst = [1, 2, 3]\n"
        "lst.append(4)\n"
        "print(lst[0])\n"
        "print(lst[-1])\n"
        "print(len(lst[1:3]))\n"
        "print(lst.pop())\n"
        "print(lst.pop(0))\n"
        "d = {1: 10, 2: 20}\n"
        "print(d.get(1))\n"
        "print(d.get(3))\n"
        "print(d.get(3, 99))\n"
        "d[3] = 30\n"
        "ks = d.keys()\n"
        "vs = d.values()\n"
        "print(len(ks))\n"
        "print(ks[1])\n"
        "print(vs[2])\n"
        "lst2 = [1, 2, 3]\n"
        "lst2.extend([4, 5])\n"
        "lst2.insert(1, 99)\n"
        "lst2.remove(2)\n"
        "print(lst2[0])\n"
        "print(lst2[1])\n"
        "print(len(lst2))\n"
        "lst2.remove(99)\n"
        "print(len(lst2))\n"
        "t = (1, 2, 1)\n"
        "print(t.count(1))\n"
        "print(t.index(2))\n"
        "print(t.index(9))\n"
        "d2 = {1: 10, 2: 20}\n"
        "print(d2.pop(1))\n"
        "print(d2.pop(3, 99))\n"
        "items = d2.items()\n"
        "print(len(items))\n"
        "print(items[0][0])\n"
        "print(items[0][1])\n"
        "total = 0\n"
        "for x in [1, 2, 3]:\n"
        "    total = total + x\n"
        "print(total)\n"
        "acc = 0\n"
        "for x in (4, 5):\n"
        "    acc = acc + x\n"
        "print(acc)\n"
        "d3 = {7: 70, 8: 80}\n"
        "sumk = 0\n"
        "for x in d3.keys():\n"
        "    sumk = sumk + x\n"
        "print(sumk)\n"
        "sumv = 0\n"
        "for x in d3.values():\n"
        "    sumv = sumv + x\n"
        "print(sumv)\n"
    )

    output_wasm = root / "output.wasm"
    existed = output_wasm.exists()

    runner = write_wasm_runner(
        tmp_path,
        "run_wasm_list_dict.js",
        import_overrides=LIST_DICT_IMPORT_OVERRIDES,
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
        assert (
            run.stdout.strip()
            == "1\n4\n2\n4\n1\n10\nNone\n99\n3\n2\n30\n1\n99\n5\n4\n2\n1\nNone\n10\n99\n1\n2\n20\n6\n9\n15\n150"
        )
    finally:
        if not existed and output_wasm.exists():
            output_wasm.unlink()
