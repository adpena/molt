from pathlib import Path

from tests.wasm_linked_runner import (
    build_wasm_linked,
    require_wasm_toolchain,
    run_cpython_reference,
    run_wasm_linked,
)

# Dict views are not sequences and tuple.index raises on a miss: the
# dictkeys/dictvalues/dictitems_as_sequence tables in Objects/dictobject.c have
# no sq_item, so Objects/abstract.c:PyObject_GetItem reports "'dict_keys'
# object is not subscriptable", and Objects/tupleobject.c:tuple_index_impl
# raises ValueError. The program prints what CPython prints; CPython is the
# oracle.
PROGRAM = """\
def report(probe):
    try:
        probe()
    except (TypeError, ValueError) as exc:
        print(type(exc).__name__, exc)

lst = [1, 2, 3]
lst.append(4)
print(lst[0])
print(lst[-1])
print(len(lst[1:3]))
print(lst.pop())
print(lst.pop(0))
d = {1: 10, 2: 20}
print(d.get(1))
print(d.get(3))
print(d.get(3, 99))
d[3] = 30
ks = d.keys()
vs = d.values()
print(len(ks))
print(list(ks)[1])
print(list(vs)[2])
report(lambda: ks[1])
report(lambda: vs[2])
lst2 = [1, 2, 3]
lst2.extend([4, 5])
lst2.insert(1, 99)
lst2.remove(2)
print(lst2[0])
print(lst2[1])
print(len(lst2))
lst2.remove(99)
print(len(lst2))
t = (1, 2, 1)
print(t.count(1))
print(t.index(2))
report(lambda: t.index(9))
d2 = {1: 10, 2: 20}
print(d2.pop(1))
print(d2.pop(3, 99))
items = d2.items()
print(len(items))
print(list(items)[0][0])
print(list(items)[0][1])
report(lambda: items[0])
total = 0
for x in [1, 2, 3]:
    total = total + x
print(total)
acc = 0
for x in (4, 5):
    acc = acc + x
print(acc)
d3 = {7: 70, 8: 80}
sumk = 0
for x in d3.keys():
    sumk = sumk + x
print(sumk)
sumv = 0
for x in d3.values():
    sumv = sumv + x
print(sumv)
"""


def test_wasm_list_dict_ops_parity(tmp_path: Path) -> None:
    require_wasm_toolchain()

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "list_dict_ops.py"
    src.write_text(PROGRAM, encoding="utf-8")

    expected = run_cpython_reference(src)
    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)
    assert run.returncode == 0, run.stderr
    assert expected.stdout.strip()
    assert run.stdout == expected.stdout
