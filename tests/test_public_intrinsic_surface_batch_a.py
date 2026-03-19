from __future__ import annotations

import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import builtins
import functools as _host_functools
import importlib.util
import sys
import types


class _Closing:
    def __init__(self, thing):
        self._thing = thing

    def __enter__(self):
        return self._thing

    def __exit__(self, exc_type, exc, tb):
        close = getattr(self._thing, "close", None)
        if close is not None:
            close()
        return False


class _RandomState:
    next_handle = 0
    values = {{}}


def _random_new():
    _RandomState.next_handle += 1
    handle = _RandomState.next_handle
    _RandomState.values[handle] = 0
    return handle


def _random_seed(handle, value, _version):
    _RandomState.values[handle] = 0 if value is None else int(value)


def _random_random(handle):
    return float(_RandomState.values[handle]) / 10.0


def _random_getrandbits(handle, k):
    return (int(_RandomState.values[handle]) + 1) & ((1 << int(k)) - 1)


def _random_randbelow(handle, n):
    return int(_RandomState.values[handle]) % int(n)


def _random_getstate(handle):
    return ("state", _RandomState.values[handle])


def _random_setstate(handle, state):
    _RandomState.values[handle] = int(state[1])


def _random_shuffle(_handle, seq):
    seq.reverse()


def _random_const(_handle, *args):
    return float(len(args))


def _random_choices(_handle, population, cum_weights, k):
    base = population[0] if population else None
    return [base for _ in range(int(k))]


def _random_sample(_handle, population, k):
    return list(population)[: int(k)]


def _random_randrange(handle, start, stop, step):
    if stop is None:
        return int(_RandomState.values[handle]) % int(start)
    return int(start)


def _random_randbytes(_handle, n):
    return b"x" * int(n)


def _struct_pack(fmt, values):
    return (str(fmt) + ":" + ",".join(str(v) for v in values)).encode("utf-8")


def _struct_unpack(fmt, buffer):
    data = bytes(buffer).decode("utf-8")
    _fmt, payload = data.split(":", 1)
    if not payload:
        return ()
    return tuple(int(part) for part in payload.split(","))


def _struct_calcsize(_fmt):
    return 4


def _struct_pack_into(buffer, offset, data):
    mv = memoryview(buffer)
    offset = int(offset)
    mv[offset : offset + len(data)] = data


def _struct_unpack_from(fmt, buffer, offset):
    return _struct_unpack(fmt, bytes(buffer)[int(offset) :])


def _struct_iter_unpack(fmt, buffer):
    return [_struct_unpack(fmt, buffer)]


def _functools_update_wrapper(wrapper, wrapped, assigned, updated):
    for name in assigned:
        if hasattr(wrapped, name):
            setattr(wrapper, name, getattr(wrapped, name))
    for name in updated:
        target = getattr(wrapper, name, None)
        source = getattr(wrapped, name, None)
        if hasattr(target, "update") and hasattr(source, "items"):
            target.update(source)
    wrapper.__wrapped__ = wrapped
    return wrapper


def _functools_wraps(wrapped, assigned, updated):
    def _decorator(wrapper):
        return _functools_update_wrapper(wrapper, wrapped, assigned, updated)

    return _decorator


def _functools_partial(func, args, kwargs):
    def _wrapped(*more_args, **more_kwargs):
        merged = dict(kwargs)
        merged.update(more_kwargs)
        return func(*args, *more_args, **merged)

    return _wrapped


def _functools_reduce(function, iterable, initializer):
    iterator = iter(iterable)
    if initializer is _KW_MARK:
        acc = next(iterator)
    else:
        acc = initializer
    for item in iterator:
        acc = function(acc, item)
    return acc


def _functools_lru_cache(_maxsize, _typed):
    def _decorator(fn):
        return fn

    return _decorator


def _sd_new(func):
    return {{"default": func, "registry": {{object: func}}}}


def _sd_register(handle, cls, func):
    handle["registry"][cls] = func


def _sd_call(handle, args, _kwargs):
    if args:
        return handle["registry"].get(type(args[0]), handle["default"])
    return handle["default"]


def _sd_dispatch(handle, cls):
    return handle["registry"].get(cls, handle["default"])


def _sd_registry(handle):
    return dict(handle["registry"])


def _sd_drop(_handle):
    return None


_KW_MARK = object()
_SLEEPS = []

builtins._molt_intrinsics = {{
    "molt_time_monotonic": lambda: 1.25,
    "molt_time_monotonic_ns": lambda: 125,
    "molt_time_perf_counter": lambda: 2.5,
    "molt_time_perf_counter_ns": lambda: 250,
    "molt_time_time": lambda: 3.5,
    "molt_time_time_ns": lambda: 350,
    "molt_time_process_time": lambda: 4.5,
    "molt_time_process_time_ns": lambda: 450,
    "molt_time_localtime": lambda secs=None: (2026, 3, 18, 12, 0, 0, 2, 77, 0),
    "molt_time_gmtime": lambda secs=None: (2026, 3, 18, 18, 0, 0, 2, 77, 0),
    "molt_time_strftime": lambda fmt, tup: f"{{fmt}}:{{tup[0]}}",
    "molt_time_timezone": lambda: 0,
    "molt_time_daylight": lambda: 0,
    "molt_time_altzone": lambda: 0,
    "molt_time_tzname": lambda: ("UTC", "UTC"),
    "molt_time_asctime": lambda tup: f"ASC:{{tup[0]}}",
    "molt_time_mktime": lambda tup: 123.0,
    "molt_time_timegm": lambda tup: 456,
    "molt_time_get_clock_info": lambda name: (name, "stub", 0.01, True, False),
    "molt_async_sleep": lambda delay, result=None: ("sleep", float(delay), result),
    "molt_block_on": lambda fut: _SLEEPS.append(fut),
    "molt_capabilities_trusted": lambda: True,
    "molt_capabilities_has": lambda name: True,
    "molt_context_null": lambda value=None: value,
    "molt_contextlib_closing": lambda thing: _Closing(thing),
    "molt_contextlib_aclosing_enter": lambda thing: thing,
    "molt_contextlib_aclosing_exit": lambda thing: None,
    "molt_contextlib_abstract_enter": lambda self: self,
    "molt_contextlib_abstract_aenter": lambda self: self,
    "molt_contextlib_abstract_subclasshook": lambda cls: NotImplemented,
    "molt_contextlib_abstract_async_subclasshook": lambda cls: NotImplemented,
    "molt_contextlib_contextdecorator_call": lambda cm, func, args, kwargs: func(*args, **kwargs),
    "molt_contextlib_chdir_enter": lambda path: path,
    "molt_contextlib_chdir_exit": lambda state: None,
    "molt_contextlib_asyncgen_cm_new": lambda func, args, kwargs: (func, args, kwargs),
    "molt_contextlib_asyncgen_cm_drop": lambda handle: None,
    "molt_contextlib_asyncgen_cm_aenter": lambda handle: None,
    "molt_contextlib_asyncgen_cm_aexit": lambda handle, exc_type, exc, tb: False,
    "molt_contextlib_asyncgen_enter": lambda gen: next(gen),
    "molt_contextlib_asyncgen_exit": lambda gen, exc_type, exc, tb: False,
    "molt_contextlib_generator_enter": lambda gen: next(gen),
    "molt_contextlib_generator_exit": lambda gen, exc_type, exc, tb: False,
    "molt_contextlib_suppress_match": lambda exc, types: isinstance(exc, types),
    "molt_contextlib_redirect_enter": lambda target, stream_name: None,
    "molt_contextlib_redirect_exit": lambda state: None,
    "molt_contextlib_exitstack_new": lambda: [],
    "molt_contextlib_exitstack_drop": lambda handle: None,
    "molt_contextlib_exitstack_push": lambda handle, exit_fn: handle.append(exit_fn) or exit_fn,
    "molt_contextlib_exitstack_push_callback": lambda handle, callback, args, kwargs: handle.append((callback, args, kwargs)),
    "molt_contextlib_exitstack_pop": lambda handle: handle.pop(),
    "molt_contextlib_exitstack_pop_all": lambda handle: list(handle),
    "molt_contextlib_exitstack_exit": lambda handle, exc_type, exc, tb: False,
    "molt_contextlib_exitstack_enter_context": lambda handle, cm: cm.__enter__(),
    "molt_contextlib_async_exitstack_push_callback": lambda handle, callback, args, kwargs: handle.append((callback, args, kwargs)),
    "molt_contextlib_async_exitstack_push_exit": lambda handle, exit_fn: handle.append(exit_fn),
    "molt_contextlib_async_exitstack_enter_context": lambda handle, cm: cm.__enter__(),
    "molt_contextlib_async_exitstack_exit": lambda handle, exc_type, exc, tb: False,
    "molt_random_new": _random_new,
    "molt_random_seed": _random_seed,
    "molt_random_random": _random_random,
    "molt_random_getrandbits": _random_getrandbits,
    "molt_random_randbelow": _random_randbelow,
    "molt_random_getstate": _random_getstate,
    "molt_random_setstate": _random_setstate,
    "molt_random_shuffle": _random_shuffle,
    "molt_random_gauss": _random_const,
    "molt_random_uniform": _random_const,
    "molt_random_triangular": _random_const,
    "molt_random_expovariate": _random_const,
    "molt_random_normalvariate": _random_const,
    "molt_random_lognormvariate": _random_const,
    "molt_random_vonmisesvariate": _random_const,
    "molt_random_paretovariate": _random_const,
    "molt_random_weibullvariate": _random_const,
    "molt_random_gammavariate": _random_const,
    "molt_random_betavariate": _random_const,
    "molt_random_choices": _random_choices,
    "molt_random_sample": _random_sample,
    "molt_random_binomialvariate": _random_const,
    "molt_random_randrange": _random_randrange,
    "molt_random_randbytes": _random_randbytes,
    "molt_math_log2": lambda x: 1.0,
    "molt_math_floor": lambda x: int(x),
    "molt_math_fabs": lambda x: abs(x),
    "molt_math_sqrt": lambda x: float(x) ** 0.5,
    "molt_math_lgamma": lambda x: 0.0,
    "molt_math_log": lambda x, base=None: 0.0,
    "molt_math_isfinite": lambda x: True,
    "molt_struct_pack": _struct_pack,
    "molt_struct_unpack": _struct_unpack,
    "molt_struct_calcsize": _struct_calcsize,
    "molt_struct_pack_into": _struct_pack_into,
    "molt_struct_unpack_from": _struct_unpack_from,
    "molt_struct_iter_unpack": _struct_iter_unpack,
    "molt_functools_kwd_mark": lambda: _KW_MARK,
    "molt_functools_update_wrapper": _functools_update_wrapper,
    "molt_functools_wraps": _functools_wraps,
    "molt_functools_partial": _functools_partial,
    "molt_functools_reduce": _functools_reduce,
    "molt_functools_lru_cache": _functools_lru_cache,
    "molt_functools_singledispatch_new": _sd_new,
    "molt_functools_singledispatch_register": _sd_register,
    "molt_functools_singledispatch_call": _sd_call,
    "molt_functools_singledispatch_dispatch": _sd_dispatch,
    "molt_functools_singledispatch_registry": _sd_registry,
    "molt_functools_singledispatch_drop": _sd_drop,
    "molt_functools_cmp_to_key": _host_functools.cmp_to_key,
    "molt_functools_total_ordering": _host_functools.total_ordering,
}}

_intrinsics_mod = types.ModuleType("_intrinsics")


def _require_intrinsic(name, namespace=None):
    intrinsics = getattr(builtins, "_molt_intrinsics", {{}})
    if name in intrinsics:
        value = intrinsics[name]
        if namespace is not None:
            namespace[name] = value
        return value
    raise RuntimeError(f"intrinsic unavailable: {{name}}")


_intrinsics_mod.require_intrinsic = _require_intrinsic
sys.modules["_intrinsics"] = _intrinsics_mod


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


time_mod = _load_module("molt_test_time", {str(STDLIB_ROOT / "time.py")!r})
contextlib_mod = _load_module("molt_test_contextlib", {str(STDLIB_ROOT / "contextlib.py")!r})
random_mod = _load_module("molt_test_random", {str(STDLIB_ROOT / "random.py")!r})
struct_mod = _load_module("molt_test_struct", {str(STDLIB_ROOT / "struct.py")!r})
functools_mod = _load_module("molt_test_functools", {str(STDLIB_ROOT / "functools.py")!r})


class _Closer:
    def __init__(self):
        self.closed = False

    def close(self):
        self.closed = True


closer = _Closer()
with contextlib_mod.closing(closer) as value:
    entered = value is closer

rng = random_mod.Random(7)
buf = bytearray(b"xxxxxxxx")
struct_mod.pack_into("ii", buf, 0, 1, 2)

def _adder(a, b):
    return a + b

partial_add = functools_mod.partial(_adder, 4)

checks = {{
    "time": (
        time_mod.monotonic() == 1.25
        and time_mod.time() == 3.5
        and time_mod.get_clock_info("time").implementation == "stub"
        and "molt_time_monotonic" not in time_mod.__dict__
        and "molt_async_sleep" not in time_mod.__dict__
        and "molt_capabilities_has" not in time_mod.__dict__
    ),
    "contextlib": (
        contextlib_mod.nullcontext("x") == "x"
        and entered is True
        and closer.closed is True
        and "molt_context_null" not in contextlib_mod.__dict__
        and "molt_contextlib_closing" not in contextlib_mod.__dict__
    ),
    "random": (
        rng.getrandbits(3) == 0
        and rng.randrange(10, 20) == 10
        and rng.randbytes(3) == b"xxx"
        and "molt_random_new" not in random_mod.__dict__
        and "molt_math_log2" not in random_mod.__dict__
    ),
    "struct": (
        struct_mod.calcsize("ii") == 4
        and struct_mod.unpack("ii", struct_mod.pack("ii", 1, 2)) == (1, 2)
        and bytes(buf).startswith(b"ii:1,2")
        and "molt_struct_pack" not in struct_mod.__dict__
        and "molt_struct_unpack" not in struct_mod.__dict__
    ),
    "functools": (
        partial_add(5) == 9
        and functools_mod.reduce(_adder, [1, 2, 3], 0) == 6
        and "molt_functools_partial" not in functools_mod.__dict__
        and "molt_functools_cmp_to_key" not in functools_mod.__dict__
    ),
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_a() -> None:
    proc = subprocess.run(
        [sys.executable, "-c", _PROBE],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    checks: dict[str, str] = {}
    for line in proc.stdout.splitlines():
        prefix, *rest = line.split("|")
        if prefix == "CHECK":
            checks[rest[0]] = rest[1]
    assert checks == {
        "contextlib": "True",
        "functools": "True",
        "random": "True",
        "struct": "True",
        "time": "True",
    }
