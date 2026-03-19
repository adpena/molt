from __future__ import annotations

import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import base64 as _host_base64
import bisect as _host_bisect
import builtins
import importlib.util
import sys
import types


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


_bisect_mod = types.ModuleType("_bisect")
_bisect_mod.bisect_left = lambda a, x, lo=0, hi=None, key=None: _host_bisect.bisect_left(a, x, lo, len(a) if hi is None else hi)
_bisect_mod.bisect_right = lambda a, x, lo=0, hi=None, key=None: _host_bisect.bisect_right(a, x, lo, len(a) if hi is None else hi)
_bisect_mod.insort_left = lambda a, x, lo=0, hi=None, key=None: a.insert(_host_bisect.bisect_left(a, x, lo, len(a) if hi is None else hi), x)
_bisect_mod.insort_right = lambda a, x, lo=0, hi=None, key=None: a.insert(_host_bisect.bisect_right(a, x, lo, len(a) if hi is None else hi), x)
sys.modules["_bisect"] = _bisect_mod


def _heapify(heap):
    heap.sort()


def _heappush(heap, item):
    heap.append(item)
    heap.sort()


def _heappop(heap):
    return heap.pop(0)


def _heapreplace(heap, item):
    first = heap.pop(0)
    heap.append(item)
    heap.sort()
    return first


def _heappushpop(heap, item):
    heap.append(item)
    heap.sort()
    return heap.pop(0)


def _heapify_max(heap):
    heap.sort(reverse=True)


def _heappop_max(heap):
    return heap.pop(0)


def _nsmallest(n, iterable, key=None):
    return sorted(iterable, key=key)[: int(n)]


def _nlargest(n, iterable, key=None):
    return sorted(iterable, key=key, reverse=True)[: int(n)]


def _merge(iterables, key, reverse):
    values = []
    for iterable in iterables:
        values.extend(iterable)
    return sorted(values, key=key, reverse=reverse)


builtins._molt_intrinsics = {{
    "molt_base64_b64encode": lambda s, altchars=None: _host_base64.b64encode(bytes(s), altchars),
    "molt_base64_b64decode": lambda s, altchars=None, validate=False: _host_base64.b64decode(s, altchars=altchars, validate=validate),
    "molt_base64_standard_b64encode": lambda s: _host_base64.standard_b64encode(bytes(s)),
    "molt_base64_standard_b64decode": lambda s: _host_base64.standard_b64decode(s),
    "molt_base64_urlsafe_b64encode": lambda s: _host_base64.urlsafe_b64encode(bytes(s)),
    "molt_base64_urlsafe_b64decode": lambda s: _host_base64.urlsafe_b64decode(s),
    "molt_base64_b32encode": lambda s: _host_base64.b32encode(bytes(s)),
    "molt_base64_b32decode": lambda s, casefold=False, map01=None: _host_base64.b32decode(s, casefold=casefold, map01=map01),
    "molt_base64_b32hexencode": lambda s: _host_base64.b32hexencode(bytes(s)),
    "molt_base64_b32hexdecode": lambda s, casefold=False: _host_base64.b32hexdecode(s, casefold=casefold),
    "molt_base64_b16encode": lambda s: _host_base64.b16encode(bytes(s)),
    "molt_base64_b16decode": lambda s, casefold=False: _host_base64.b16decode(s, casefold=casefold),
    "molt_base64_a85encode": lambda b, foldspaces=False, wrapcol=0, pad=False, adobe=False: _host_base64.a85encode(bytes(b), foldspaces=foldspaces, wrapcol=wrapcol, pad=pad, adobe=adobe),
    "molt_base64_a85decode": lambda b, foldspaces=False, adobe=False: _host_base64.a85decode(b, foldspaces=foldspaces, adobe=adobe),
    "molt_base64_b85encode": lambda b, pad=False: _host_base64.b85encode(bytes(b), pad=pad),
    "molt_base64_b85decode": lambda b: _host_base64.b85decode(b),
    "molt_base64_encodebytes": lambda b: _host_base64.encodebytes(bytes(b)),
    "molt_base64_decodebytes": lambda b: _host_base64.decodebytes(b),
    "molt_bisect_left": lambda *args, **kwargs: 0,
    "molt_stdlib_probe": lambda: None,
    "molt_cancel_token_get_current": lambda: 1,
    "molt_statistics_mean": lambda data: sum(data) / len(data),
    "molt_statistics_fmean": lambda data: float(sum(data) / len(data)),
    "molt_statistics_stdev": lambda data, xbar=None: 1.0,
    "molt_statistics_variance": lambda data, xbar=None: 1.0,
    "molt_statistics_pvariance": lambda data, mu=None: 1.0,
    "molt_statistics_pstdev": lambda data, mu=None: 1.0,
    "molt_statistics_median": lambda data: sorted(data)[len(data) // 2],
    "molt_statistics_median_low": lambda data: sorted(data)[(len(data) - 1) // 2],
    "molt_statistics_median_high": lambda data: sorted(data)[len(data) // 2],
    "molt_statistics_median_grouped": lambda data, interval=1: float(sorted(data)[len(data) // 2]),
    "molt_statistics_mode": lambda data: list(data)[0],
    "molt_statistics_multimode": lambda data: [list(data)[0]],
    "molt_statistics_quantiles": lambda data, n=4, method="exclusive": [2.0],
    "molt_statistics_harmonic_mean": lambda data, weights=None: 1.0,
    "molt_statistics_geometric_mean": lambda data: 1.0,
    "molt_statistics_covariance": lambda x, y: 1.0,
    "molt_statistics_correlation": lambda x, y: 1.0,
    "molt_statistics_linear_regression": lambda x, y, proportional=False: (1.0, 2.0),
    "molt_statistics_normal_dist_new": lambda mu, sigma: (mu, sigma),
    "molt_statistics_normal_dist_samples": lambda mu, sigma, n, seed, fn: [mu for _ in range(int(n))],
    "molt_statistics_normal_dist_pdf": lambda mu, sigma, x: 0.5,
    "molt_statistics_normal_dist_cdf": lambda mu, sigma, x: 0.5,
    "molt_statistics_normal_dist_inv_cdf": lambda mu, sigma, p: mu,
    "molt_statistics_normal_dist_zscore": lambda mu, sigma, x: 0.0,
    "molt_statistics_normal_dist_overlap": lambda mu1, sigma1, mu2, sigma2: 1.0,
    "molt_heapq_heapify": _heapify,
    "molt_heapq_heappush": _heappush,
    "molt_heapq_heappop": _heappop,
    "molt_heapq_heapreplace": _heapreplace,
    "molt_heapq_heappushpop": _heappushpop,
    "molt_heapq_heapify_max": _heapify_max,
    "molt_heapq_heappop_max": _heappop_max,
    "molt_heapq_nsmallest": _nsmallest,
    "molt_heapq_nlargest": _nlargest,
    "molt_heapq_merge": _merge,
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


base64_mod = _load_module("molt_test_base64", {str(STDLIB_ROOT / "base64.py")!r})
bisect_mod = _load_module("molt_test_bisect", {str(STDLIB_ROOT / "bisect.py")!r})
contextvars_mod = _load_module("molt_test_contextvars", {str(STDLIB_ROOT / "contextvars.py")!r})
statistics_mod = _load_module("molt_test_statistics", {str(STDLIB_ROOT / "statistics.py")!r})
heapq_mod = _load_module("molt_test_heapq", {str(STDLIB_ROOT / "heapq.py")!r})

cv = contextvars_mod.ContextVar("x", default=1)
token = cv.set(5)
heap = [3, 1]
heapq_mod.heapify(heap)
heapq_mod.heappush(heap, 2)

checks = {{
    "base64": (
        base64_mod.b64encode(b"hi") == b"aGk="
        and base64_mod.b64decode(b"aGk=") == b"hi"
        and "molt_base64_b64encode" not in base64_mod.__dict__
        and "molt_base64_b64decode" not in base64_mod.__dict__
    ),
    "bisect": (
        bisect_mod.bisect_left([1, 3, 5], 4) == 2
        and "molt_bisect_left" not in bisect_mod.__dict__
    ),
    "contextvars": (
        cv.get() == 5
        and contextvars_mod.copy_context().get(cv) == 5
        and (cv.reset(token) is None)
        and cv.get() == 1
        and "molt_stdlib_probe" not in contextvars_mod.__dict__
        and "molt_cancel_token_get_current" not in contextvars_mod.__dict__
    ),
    "statistics": (
        statistics_mod.mean([1, 2, 3]) == 2
        and statistics_mod.NormalDist(1.0, 2.0).mean == 1.0
        and "molt_statistics_mean" not in statistics_mod.__dict__
        and "molt_statistics_normal_dist_new" not in statistics_mod.__dict__
    ),
    "heapq": (
        heap == [1, 2, 3]
        and heapq_mod.heappop(heap) == 1
        and list(heapq_mod.merge([3, 1], [2])) == [1, 2, 3]
        and "molt_heapq_heapify" not in heapq_mod.__dict__
        and "molt_heapq_merge" not in heapq_mod.__dict__
    ),
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_b() -> None:
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
        "base64": "True",
        "bisect": "True",
        "contextvars": "True",
        "heapq": "True",
        "statistics": "True",
    }
