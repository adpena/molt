# MOLT_ENV: MOLT_CAPABILITIES=env.read,env.write MOLT_ENV_PRELOAD=seed
"""Purpose: ensure mapping snapshots stay coherent while putenv changes only the process."""

import os


PRELOAD = "MOLT_ENV_PRELOAD"
MUT_KEY = "MOLT_ENV_INTRINSIC_SYNC_KEY"


print(os.getenv(PRELOAD))
print(PRELOAD in os.environ)
print(os.environ.copy().get(PRELOAD))
print(PRELOAD in list(os.environ.keys()))

os.putenv(MUT_KEY, "one")
print(os.getenv(MUT_KEY))
print(MUT_KEY in os.environ.copy())
print(MUT_KEY in list(os.environ))
print(MUT_KEY in os.environ)

os.unsetenv(MUT_KEY)
print(os.getenv(MUT_KEY) is None)
print(MUT_KEY in os.environ)

os.environ[MUT_KEY] = "two"
print(os.getenv(MUT_KEY))
print(dict(os.environ.items()).get(MUT_KEY))
os.putenv(MUT_KEY, "process-three")
print(os.environ[MUT_KEY])
print(os.environ.copy()[MUT_KEY])
os.unsetenv(MUT_KEY)
print(os.environ[MUT_KEY])
print(os.environ.copy()[MUT_KEY])

del os.environ[MUT_KEY]
print(os.getenv(MUT_KEY) is None)
print(MUT_KEY in os.environ)


# The independent CPython oracle preserves insertion order, including overwrite,
# removal/reinsertion and live mapping views. Print only these synthetic keys.
ORDER_Z = "__MOLT_ENV_ORDER_Z__"
ORDER_A = "__MOLT_ENV_ORDER_A__"
ORDER_B = "__MOLT_ENV_ORDER_B__"
order_keys = (ORDER_Z, ORDER_A, ORDER_B)
for key in order_keys:
    os.environ.pop(key, None)
os.environ[ORDER_Z] = "first"
os.environ[ORDER_A] = "second"
keys_view = os.environ.keys()
items_view = os.environ.items()
values_view = os.environ.values()
iterator = iter(os.environ)
os.environ[ORDER_B] = "third"
os.environ[ORDER_Z] = "updated"
print([key for key in iterator if key in order_keys])
print([key for key in keys_view if key in order_keys])
print([(key, value) for key, value in items_view if key in order_keys])
print("updated" in values_view)
del os.environ[ORDER_Z]
os.environ[ORDER_Z] = "reinserted"
print([key for key in os.environ.copy() if key in order_keys])

# _Environ inherits MutableMapping.popitem: first insertion, rather than dict's
# last insertion or a lexically greatest key. Restore every ambient entry.
original = os.environ.copy()
try:
    os.environ.clear()
    os.environ[ORDER_A] = "first"
    os.environ[ORDER_Z] = "second"
    os.environ[ORDER_B] = "middle"
    print(os.environ.popitem())
    print(list(os.environ.items()))
    iterator = iter(os.environ)
    os.environ[ORDER_B] = "third"
    print(next(iterator))
    del os.environ[ORDER_B]
    os.environ[ORDER_Z] = "late"
    print(list(iterator))
finally:
    os.environ.clear()
    os.environ.update(original)
    for key in order_keys:
        os.environ.pop(key, None)

if os.name != "nt":
    class EnvOperand:
        def __init__(self, raw):
            self.raw = raw
            self.calls = 0

        def __fspath__(self):
            self.calls += 1
            return self.raw

    raw_key = EnvOperand(b"__MOLT_ENV_RAW_\xff__")
    raw_value = EnvOperand(b"raw\xfe")
    os.putenv(raw_key, raw_value)
    print(raw_key.calls, raw_value.calls)
    print(os.getenv(os.fsdecode(raw_key.raw)) is None)
    os.unsetenv(raw_key)
    print(raw_key.calls)

    invalid_key = EnvOperand(b"bad\0key")
    unused_value = EnvOperand(b"unused")
    try:
        os.putenv(invalid_key, unused_value)
    except ValueError as error:
        print(type(error).__name__, invalid_key.calls, unused_value.calls)
    invalid_value = EnvOperand(b"bad\0value")
    valid_key = EnvOperand(b"__MOLT_ENV_FAILURE_ORDER__")
    try:
        os.putenv(valid_key, invalid_value)
    except ValueError as error:
        print(type(error).__name__, valid_key.calls, invalid_value.calls)
    wrong_key = EnvOperand(42)
    try:
        os.putenv(wrong_key, unused_value)
    except TypeError as error:
        print(type(error).__name__, wrong_key.calls, unused_value.calls)


# CPython is the independent error-family oracle. No exception text or ambient
# data is printed; exact type/errno and mapping preservation remain observable.
ERROR_KEY = "__MOLT_ENV_ERROR_FAMILY__"
error_original = os.environ.get(ERROR_KEY)
error_cases = [
    ("empty", "", "value"),
    ("equals", "A=B", "value"),
    ("nul-key", "bad\0key", "value"),
    ("nul-value", ERROR_KEY, "bad\0value"),
    ("key-type", 42, "value"),
    ("value-type", ERROR_KEY, 42),
]
for label, key, value in error_cases:
    for operation in ("putenv", "unsetenv", "set", "delete", "pop", "setdefault"):
        os.environ.pop(ERROR_KEY, None)
        before = os.environ.copy()
        try:
            if operation == "putenv":
                os.putenv(key, value)
            elif operation == "unsetenv":
                os.unsetenv(key)
            elif operation == "set":
                os.environ[key] = value
            elif operation == "delete":
                del os.environ[key]
            elif operation == "pop":
                os.environ.pop(key)
            else:
                os.environ.setdefault(key, value)
        except Exception as error:
            print(label, operation, type(error).__name__, getattr(error, "errno", None), os.environ.copy() == before)
        else:
            print(label, operation, "ok", None, os.environ.copy() == before)
        finally:
            os.environ.pop(ERROR_KEY, None)
            os.unsetenv(ERROR_KEY)
if error_original is not None:
    os.environ[ERROR_KEY] = error_original


# The encoded Python objects are the storage contract, not their OS conversion.
if os.name != "nt":
    callbacks = []
    class Encoded:
        def __init__(self, raw, label):
            self.raw, self.label = raw, label
        def __fspath__(self):
            callbacks.append("fspath:" + self.label)
            return self.raw
        def __hash__(self):
            callbacks.append("hash:" + self.label)
            return hash(self.raw)
        def __eq__(self, other):
            return self.raw == (other.raw if isinstance(other, Encoded) else other)
        def decode(self, encoding, errors):
            callbacks.append("decode:" + self.label)
            return self.raw.decode(encoding, errors)
    class Text(str):
        def __new__(cls, text, encoded):
            value = str.__new__(cls, text)
            value.encoded = encoded
            return value
        def encode(self, encoding, errors):
            callbacks.append("encode:" + self.encoded.label)
            return self.encoded
    encoded_key = Encoded(b"__MOLT_ENV_EXACT_OBJECT__", "key")
    encoded_value = Encoded(b"exact-value", "value")
    key = Text("__MOLT_ENV_EXACT_OBJECT__", encoded_key)
    value = Text("exact-value", encoded_value)
    os.environ[key] = value
    print(callbacks)
    callbacks.clear()
    print(os.environ[key], callbacks)
    callbacks.clear()
    del os.environ[key]
    print(callbacks)
