from __future__ import annotations

import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import builtins
import importlib.util
import bisect as _host_bisect
import random as _host_random
import sys
import types

class _HandleBox:
    next_id = 0
    states = {{}}


def _new_handle(seed=None):
    _HandleBox.next_id += 1
    handle = _HandleBox.next_id
    rng = _host_random.Random()
    if seed is not None:
        rng.seed(seed)
    _HandleBox.states[handle] = rng
    return handle


builtins._molt_intrinsics = {{
    "molt_random_new": lambda: _new_handle(),
    "molt_random_seed": lambda handle, seed, version=2: _HandleBox.states[handle].seed(seed, version),
    "molt_random_random": lambda handle: _HandleBox.states[handle].random(),
    "molt_random_getrandbits": lambda handle, k: _HandleBox.states[handle].getrandbits(k),
    "molt_random_randbelow": lambda handle, n: _HandleBox.states[handle].randrange(n),
    "molt_random_getstate": lambda handle: _HandleBox.states[handle].getstate(),
    "molt_random_setstate": lambda handle, state: _HandleBox.states[handle].setstate(state),
    "molt_random_shuffle": lambda handle, x: _HandleBox.states[handle].shuffle(x),
    "molt_random_gauss": lambda handle, mu, sigma: _HandleBox.states[handle].gauss(mu, sigma),
    "molt_random_uniform": lambda handle, a, b: _HandleBox.states[handle].uniform(a, b),
    "molt_random_triangular": lambda handle, low, high, mode: _HandleBox.states[handle].triangular(low, high, mode),
    "molt_random_expovariate": lambda handle, lambd: _HandleBox.states[handle].expovariate(lambd),
    "molt_random_normalvariate": lambda handle, mu, sigma: _HandleBox.states[handle].normalvariate(mu, sigma),
    "molt_random_lognormvariate": lambda handle, mu, sigma: _HandleBox.states[handle].lognormvariate(mu, sigma),
    "molt_random_vonmisesvariate": lambda handle, mu, kappa: _HandleBox.states[handle].vonmisesvariate(mu, kappa),
    "molt_random_paretovariate": lambda handle, alpha: _HandleBox.states[handle].paretovariate(alpha),
    "molt_random_weibullvariate": lambda handle, alpha, beta: _HandleBox.states[handle].weibullvariate(alpha, beta),
    "molt_random_gammavariate": lambda handle, alpha, beta: _HandleBox.states[handle].gammavariate(alpha, beta),
    "molt_random_betavariate": lambda handle, alpha, beta: _HandleBox.states[handle].betavariate(alpha, beta),
    "molt_random_choices": lambda handle, population, cum_weights, k: _HandleBox.states[handle].choices(population, cum_weights=cum_weights, k=k) if cum_weights is not None else _HandleBox.states[handle].choices(population, k=k),
    "molt_random_sample": lambda handle, population, k: _HandleBox.states[handle].sample(population, k),
    "molt_random_binomialvariate": lambda handle, n, p: sum(1 for _ in range(int(n)) if _HandleBox.states[handle].random() < p),
    "molt_random_randrange": lambda handle, start, stop, step: _HandleBox.states[handle].randrange(start, stop, step),
    "molt_random_randbytes": lambda handle, n: _HandleBox.states[handle].randbytes(n),
    "molt_math_log2": __import__('math').log2,
    "molt_math_floor": __import__('math').floor,
    "molt_math_fabs": __import__('math').fabs,
    "molt_math_sqrt": __import__('math').sqrt,
    "molt_math_lgamma": __import__('math').lgamma,
    "molt_math_log": __import__('math').log,
    "molt_math_isfinite": __import__('math').isfinite,
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


sys.modules["bisect"] = _host_bisect
_load_module("random", {str(STDLIB_ROOT / "random.py")!r})
_private = _load_module("_random", {str(STDLIB_ROOT / "_random.py")!r})

rows = [
    (name, type(value).__name__, bool(callable(value)))
    for name, value in sorted(_private.__dict__.items())
    if not name.startswith("_")
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

rng = _private.Random()
rng.seed(123)
checks = {{
    "shape": type(rng).__name__ == "Random",
    "behavior": round(rng.random(), 12) == 0.052363598851 and rng.getrandbits(8) == 22,
}}
for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def _run_probe() -> tuple[list[tuple[str, str, str]], dict[str, str]]:
    proc = subprocess.run(
        [sys.executable, "-c", _PROBE],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    rows: list[tuple[str, str, str]] = []
    checks: dict[str, str] = {}
    for line in proc.stdout.splitlines():
        prefix, *rest = line.split("|")
        if prefix == "ROW":
            rows.append((rest[0], rest[1], rest[2]))
        elif prefix == "CHECK":
            checks[rest[0]] = rest[1]
    return rows, checks


def test__random_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    assert rows == [("Random", "type", "True")]
    assert checks == {"behavior": "True", "shape": "True"}
