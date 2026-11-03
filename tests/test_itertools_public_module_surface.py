from __future__ import annotations

import sys
from pathlib import Path

from tests.surface_process_guard import run_surface_test_process


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import importlib.util
import itertools as _host_itertools
import sys
from tests.stdlib_intrinsic_registry import install_registry


install_registry({{
    "molt_itertools_accumulate_type": lambda: _host_itertools.accumulate,
    "molt_itertools_batched_type": lambda: _host_itertools.batched,
    "molt_itertools_chain_type": lambda: _host_itertools.chain,
    "molt_itertools_combinations_type": lambda: _host_itertools.combinations,
    "molt_itertools_combinations_with_replacement_type": lambda: _host_itertools.combinations_with_replacement,
    "molt_itertools_compress_type": lambda: _host_itertools.compress,
    "molt_itertools_count_type": lambda: _host_itertools.count,
    "molt_itertools_cycle_type": lambda: _host_itertools.cycle,
    "molt_itertools_dropwhile_type": lambda: _host_itertools.dropwhile,
    "molt_itertools_filterfalse_type": lambda: _host_itertools.filterfalse,
    "molt_itertools_groupby_type": lambda: _host_itertools.groupby,
    "molt_itertools_islice_type": lambda: _host_itertools.islice,
    "molt_itertools_pairwise_type": lambda: _host_itertools.pairwise,
    "molt_itertools_permutations_type": lambda: _host_itertools.permutations,
    "molt_itertools_product_type": lambda: _host_itertools.product,
    "molt_itertools_repeat_type": lambda: _host_itertools.repeat,
    "molt_itertools_starmap_type": lambda: _host_itertools.starmap,
    "molt_itertools_takewhile_type": lambda: _host_itertools.takewhile,
    "molt_itertools_zip_longest_type": lambda: _host_itertools.zip_longest,
    "molt_itertools_tee": _host_itertools.tee,
}})


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
        and isinstance(itertools.repeat("x", 1), itertools.repeat)
        and type(itertools.repeat("x", 1)) is itertools.repeat
        and list(itertools.pairwise([1, 2, 3])) == [(1, 2), (2, 3)]
    ),
    "private_handles_hidden": (
        "_MOLT_CHAIN" not in itertools.__dict__
        and "_MOLT_CHAIN_FROM_ITERABLE" not in itertools.__dict__
        and "_MOLT_ISLICE" not in itertools.__dict__
        and "_MOLT_REPEAT_TYPE" not in itertools.__dict__
        and "_MOLT_KWD_MARK" not in itertools.__dict__
        and "_MISSING" not in itertools.__dict__
        and "molt_itertools_chain_type" not in itertools.__dict__
        and "molt_itertools_islice" not in itertools.__dict__
        and "molt_itertools_cycle" not in itertools.__dict__
    ),
}}
for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_itertools_public_module_hides_intrinsic_handles() -> None:
    proc = run_surface_test_process(
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
