"""Purpose: differential coverage for functools.lru_cache cache_info/clear."""

import functools

@functools.lru_cache(maxsize=2)
def f(x):
    return x + 1

print(f(1))
print(f(2))
print(f(1))
print(f.cache_info())

info = f.cache_info()
print("tuple", isinstance(info, tuple), len(info), info == (1, 2, 2, 2))
print("indices", info[0], info[-1], info[1:3])
print("hash/order", hash(info) == hash(tuple(info)), info < (2, 0, 2, 0))
print("names", info._fields, info._asdict(), info._replace(hits=3))
print("make", type(info)._make([1, 2, 2, 2]) == info)
try:
    info.hits = 1
except AttributeError:
    print("immutable", True)

@functools.lru_cache
def direct(value):
    return value

print("shared type", type(direct.cache_info()) is type(info))
print("metadata", direct.__name__, direct.__wrapped__(7), direct.cache_parameters())

original_info = functools._CacheInfo
configured = functools.lru_cache(maxsize=3)
calls = []
sentinel = ValueError("cache info callback")

def custom_info(*values):
    calls.append(values)
    raise sentinel

functools._CacheInfo = custom_info
captured = configured(lambda value: value)
functools._CacheInfo = original_info
try:
    captured.cache_info()
except ValueError as exc:
    print("captured callback", exc is sentinel, calls)
print("old wrapper", f.cache_info() == info)

class TypedFailure:
    def __bool__(self):
        raise sentinel

try:
    functools.lru_cache(typed=TypedFailure())(1)
except ValueError as exc:
    print("typed before callable", exc is sentinel)

f.cache_clear()
print(f.cache_info())
