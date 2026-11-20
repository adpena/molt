"""PEP 649 annotate functions publish their argument as `format` (CPython
lowers it as the hidden `.format`), with `(format, /)` as the signature and
a source comprehension variable named `format` kept apart."""
# MOLT_META: min_py=3.14

import inspect


class Box[T]:
    item: T


def f(x: int, format: str) -> None:
    pass


class Plain:
    y: int
    z: list[int] = [format for format in range(1)]


for fn in (Box.__annotate__, f.__annotate__, Plain.__annotate__):
    code = fn.__code__
    print(code.co_argcount, code.co_posonlyargcount, code.co_varnames[:1], str(inspect.signature(fn)))
    print(sorted(fn(1)))
print(Plain.z)
