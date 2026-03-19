from __future__ import annotations

import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import builtins
import importlib.util
import itertools as _host_itertools
import sys
import types


_HOST_MISSING = object()

builtins._molt_intrinsics = {{
    "molt_itertools_kwd_mark": lambda: _HOST_MISSING,
    "molt_itertools_chain": lambda iterables: _host_itertools.chain(*iterables),
    "molt_itertools_chain_from_iterable": _host_itertools.chain.from_iterable,
    "molt_itertools_islice": lambda iterable, start, stop, step: _host_itertools.islice(iterable, start, None if stop is _HOST_MISSING else stop, None if step is _HOST_MISSING else step),
    "molt_itertools_repeat": _host_itertools.repeat,
    "molt_itertools_count": _host_itertools.count,
    "molt_itertools_accumulate": lambda iterable, func, initial: _host_itertools.accumulate(iterable, func, initial=None if initial is _HOST_MISSING else initial),
    "molt_itertools_batched": lambda iterable, n, strict=False: _host_itertools.batched(iterable, n, strict=strict),
    "molt_itertools_product": lambda iterables, repeat=1: _host_itertools.product(*iterables, repeat=repeat),
    "molt_itertools_permutations": _host_itertools.permutations,
    "molt_itertools_groupby": _host_itertools.groupby,
    "molt_itertools_tee": _host_itertools.tee,
    "molt_itertools_zip_longest": lambda iterables, fillvalue=None: _host_itertools.zip_longest(*iterables, fillvalue=fillvalue),
    "molt_itertools_cycle": _host_itertools.cycle,
    "molt_itertools_pairwise": _host_itertools.pairwise,
    "molt_itertools_combinations": _host_itertools.combinations,
    "molt_itertools_combinations_with_replacement": _host_itertools.combinations_with_replacement,
    "molt_itertools_compress": _host_itertools.compress,
    "molt_itertools_dropwhile": _host_itertools.dropwhile,
    "molt_itertools_filterfalse": _host_itertools.filterfalse,
    "molt_itertools_starmap": _host_itertools.starmap,
    "molt_itertools_takewhile": _host_itertools.takewhile,
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


itertools = _load_module("molt_test__itertools", {str(STDLIB_ROOT / "itertools.py")!r})

checks = {{
    "behavior": (
        list(itertools.chain([1, 2], [3])) == [1, 2, 3]
        and list(itertools.chain.from_iterable([[1], [2, 3]])) == [1, 2, 3]
        and list(itertools.islice([0, 1, 2, 3], 1, 4, 2)) == [1, 3]
        and list(itertools.repeat("x", 2)) == ["x", "x"]
        and list(itertools.pairwise([1, 2, 3])) == [(1, 2), (2, 3)]
    ),
    "private_handles_hidden": (
        "_MOLT_CHAIN" not in itertools.__dict__
        and "_MOLT_CHAIN_FROM_ITERABLE" not in itertools.__dict__
        and "_MOLT_ISLICE" not in itertools.__dict__
        and "_MOLT_REPEAT" not in itertools.__dict__
        and "_MOLT_KWD_MARK" not in itertools.__dict__
        and "_MISSING" not in itertools.__dict__
        and "molt_itertools_chain" not in itertools.__dict__
        and "molt_itertools_islice" not in itertools.__dict__
        and "molt_itertools_cycle" not in itertools.__dict__
    ),
}}
for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_itertools_public_module_hides_intrinsic_handles() -> None:
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
    assert checks == {"behavior": "True", "private_handles_hidden": "True"}
