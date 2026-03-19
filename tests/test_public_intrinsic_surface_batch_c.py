from __future__ import annotations

import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import base64 as _host_base64
import builtins
import importlib.util
import operator as _host_operator
import sys
import types


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


_operator_mod = types.ModuleType("_operator")
_names = [
    "abs","add","and_","attrgetter","concat","contains","countOf","delitem","eq",
    "floordiv","ge","getitem","gt","iadd","iand","iconcat","ifloordiv","ilshift",
    "imatmul","imod","imul","index","inv","invert","ior","ipow","irshift","is_",
    "is_not","isub","itemgetter","itruediv","ixor","le","length_hint","lshift","lt",
    "matmul","methodcaller","mod","mul","ne","neg","not_","or_","pos","pow","rshift",
    "setitem","sub","truediv","truth","xor",
]
for _name in _names:
    if hasattr(_host_operator, _name):
        setattr(_operator_mod, _name, getattr(_host_operator, _name))
    else:
        setattr(_operator_mod, _name, lambda *args, **kwargs: None)
sys.modules["_operator"] = _operator_mod


builtins._molt_intrinsics = {{
    "molt_math_isfinite": lambda x: True,
    "molt_math_isinf": lambda x: False,
    "molt_math_isnan": lambda x: False,
    "molt_math_sqrt": lambda x: float(x) ** 0.5,
    "molt_math_log2": lambda x: 3.0,
    "molt_math_log10": lambda x: 1.0,
    "molt_math_log1p": lambda x: 1.0,
    "molt_math_exp": lambda x: 1.0,
    "molt_math_expm1": lambda x: 0.0,
    "molt_math_sin": lambda x: 0.0,
    "molt_math_cos": lambda x: 1.0,
    "molt_math_acos": lambda x: 0.0,
    "molt_math_tan": lambda x: 0.0,
    "molt_math_asin": lambda x: 0.0,
    "molt_math_atan": lambda x: 0.0,
    "molt_math_atan2": lambda y, x: 0.0,
    "molt_math_sinh": lambda x: 0.0,
    "molt_math_cosh": lambda x: 1.0,
    "molt_math_tanh": lambda x: 0.0,
    "molt_math_asinh": lambda x: 0.0,
    "molt_math_acosh": lambda x: 0.0,
    "molt_math_atanh": lambda x: 0.0,
    "molt_math_lgamma": lambda x: 0.0,
    "molt_math_gamma": lambda x: 1.0,
    "molt_math_erf": lambda x: 0.0,
    "molt_math_erfc": lambda x: 1.0,
    "molt_math_fabs": lambda x: abs(x),
    "molt_math_copysign": lambda x, y: abs(x) if y >= 0 else -abs(x),
    "molt_math_floor": lambda x: int(x),
    "molt_math_ceil": lambda x: int(x) if int(x) == x else int(x) + 1,
    "molt_math_trunc": lambda x: int(x),
    "molt_math_fmod": lambda x, y: x % y,
    "molt_math_modf": lambda x: (0.5, int(x)),
    "molt_math_frexp": lambda x: (0.5, 1),
    "molt_math_ldexp": lambda x, i: x * (2 ** i),
    "molt_math_fsum": lambda xs: float(sum(xs)),
    "molt_math_factorial": lambda n: 120,
    "molt_math_comb": lambda n, k: 10,
    "molt_math_perm": lambda n, k=None: 20,
    "molt_math_degrees": lambda x: x * 10,
    "molt_math_radians": lambda x: x / 10,
    "molt_math_dist": lambda p, q: 5.0,
    "molt_math_isqrt": lambda x: int(int(x) ** 0.5),
    "molt_math_nextafter": lambda x, y: y,
    "molt_math_ulp": lambda x: 0.25,
    "molt_math_remainder": lambda x, y: 0.0,
    "molt_math_log": lambda x: 2.0,
    "molt_math_fma": lambda x, y, z: x * y + z,
    "molt_math_isclose": lambda a, b, rel_tol, abs_tol: abs(a - b) <= max(rel_tol * max(abs(a), abs(b)), abs_tol),
    "molt_math_prod": lambda iterable, start: start * 6,
    "molt_math_gcd": lambda ints: 6,
    "molt_math_lcm": lambda ints: 12,
    "molt_math_hypot": lambda coords: 5.0,
    "molt_uuid_getnode": lambda: 0xAABBCCDDEEFF,
    "molt_uuid_uuid1_bytes": lambda node, clock_seq: bytes.fromhex("12345678123412348123abcdef123456"),
    "molt_uuid_uuid3_bytes": lambda ns, name: bytes.fromhex("33333333123412348123abcdef123456"),
    "molt_uuid_uuid4_bytes": lambda: bytes.fromhex("44444444123442348123abcdef123456"),
    "molt_uuid_uuid5_bytes": lambda ns, name: bytes.fromhex("55555555123452348123abcdef123456"),
    "molt_capabilities_has": lambda name: True,
    "molt_binascii_a2b_base64": lambda s, strict_mode=False: _host_base64.b64decode(s),
    "molt_binascii_b2a_base64": lambda b, newline=True: _host_base64.b64encode(bytes(b)) + (b"\\n" if newline else b""),
    "molt_binascii_a2b_hex": lambda s: bytes.fromhex(bytes(s).decode() if not isinstance(s, str) else s),
    "molt_binascii_b2a_hex": lambda b, sep=None, bytes_per_sep=1: bytes(bytes(b).hex(), "ascii"),
    "molt_binascii_a2b_qp": lambda s, header=False: bytes(s),
    "molt_binascii_b2a_qp": lambda b, quotetabs=False, istext=True, header=False: bytes(b),
    "molt_binascii_a2b_uu": lambda s: bytes(s),
    "molt_binascii_b2a_uu": lambda b, backtick=False: bytes(b),
    "molt_binascii_crc32": lambda data, value=0: 123,
    "molt_binascii_crc_hqx": lambda data, value: 321,
    "molt_copy_copy": lambda obj: obj.copy() if hasattr(obj, "copy") else obj,
    "molt_copy_deepcopy": lambda obj, handle: (obj.copy() if hasattr(obj, "copy") else obj),
    "molt_copy_memo_new": lambda: 1,
    "molt_copy_memo_drop": lambda handle: None,
    "molt_copy_error": lambda msg: None,
    "molt_copy_replace": lambda obj, changes: obj,
    "molt_operator_truth": lambda value=True: bool(value),
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


math_mod = _load_module("molt_test_math", {str(STDLIB_ROOT / "math.py")!r})
uuid_mod = _load_module("molt_test_uuid", {str(STDLIB_ROOT / "uuid.py")!r})
binascii_mod = _load_module("molt_test_binascii", {str(STDLIB_ROOT / "binascii.py")!r})
copy_mod = _load_module("molt_test_copy", {str(STDLIB_ROOT / "copy.py")!r})
operator_mod = _load_module("molt_test_operator", {str(STDLIB_ROOT / "operator.py")!r})


class _Box:
    def __init__(self, value):
        self.value = value

    def copy(self):
        return _Box(self.value)


box = _Box(1)
replaced = copy_mod.replace(box, value=9)

checks = {{
    "math": (
        math_mod.sqrt(9) == 3.0
        and math_mod.gcd(12, 18) == 6
        and math_mod.prod([2, 3], start=2) == 12
        and "molt_math_sqrt" not in math_mod.__dict__
    ),
    "uuid": (
        uuid_mod.getnode() == 0xAABBCCDDEEFF
        and str(uuid_mod.uuid4()).startswith("44444444-1234-4234-8123-")
        and "molt_uuid_getnode" not in uuid_mod.__dict__
    ),
    "binascii": (
        binascii_mod.hexlify(b"ab") == b"6162"
        and binascii_mod.unhexlify("6162") == b"ab"
        and "molt_binascii_a2b_hex" not in binascii_mod.__dict__
        and "molt_capabilities_has" not in binascii_mod.__dict__
    ),
    "copy": (
        isinstance(copy_mod.copy(box), _Box)
        and isinstance(replaced, _Box)
        and replaced.value == 9
        and "molt_copy_copy" not in copy_mod.__dict__
    ),
    "operator": (
        operator_mod.add(2, 3) == 5
        and operator_mod.truth(1) is True
        and "molt_operator_truth" not in operator_mod.__dict__
    ),
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_c() -> None:
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
        "binascii": "True",
        "copy": "True",
        "math": "True",
        "operator": "True",
        "uuid": "True",
    }
