from __future__ import annotations

import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
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


builtins._molt_intrinsics = {{
    "molt_capabilities_has": lambda name: True,
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

modules = {{
    "lib2to3.__main__": _load_module("molt_test_lib2to3___main__", {str(STDLIB_ROOT / "lib2to3" / "__main__.py")!r}),
    "lib2to3.btm_matcher": _load_module("molt_test_lib2to3_btm_matcher", {str(STDLIB_ROOT / "lib2to3" / "btm_matcher.py")!r}),
    "lib2to3.btm_utils": _load_module("molt_test_lib2to3_btm_utils", {str(STDLIB_ROOT / "lib2to3" / "btm_utils.py")!r}),
    "lib2to3.fixer_base": _load_module("molt_test_lib2to3_fixer_base", {str(STDLIB_ROOT / "lib2to3" / "fixer_base.py")!r}),
    "lib2to3.fixer_util": _load_module("molt_test_lib2to3_fixer_util", {str(STDLIB_ROOT / "lib2to3" / "fixer_util.py")!r}),
    "lib2to3.main": _load_module("molt_test_lib2to3_main", {str(STDLIB_ROOT / "lib2to3" / "main.py")!r}),
    "lib2to3.patcomp": _load_module("molt_test_lib2to3_patcomp", {str(STDLIB_ROOT / "lib2to3" / "patcomp.py")!r}),
    "lib2to3.pygram": _load_module("molt_test_lib2to3_pygram", {str(STDLIB_ROOT / "lib2to3" / "pygram.py")!r}),
    "lib2to3.pytree": _load_module("molt_test_lib2to3_pytree", {str(STDLIB_ROOT / "lib2to3" / "pytree.py")!r}),
    "lib2to3.refactor": _load_module("molt_test_lib2to3_refactor", {str(STDLIB_ROOT / "lib2to3" / "refactor.py")!r}),
    "lib2to3.pgen2": _load_module("molt_test_lib2to3_pgen2", {str(STDLIB_ROOT / "lib2to3" / "pgen2" / "__init__.py")!r}),
    "lib2to3.pgen2.conv": _load_module("molt_test_lib2to3_pgen2_conv", {str(STDLIB_ROOT / "lib2to3" / "pgen2" / "conv.py")!r}),
    "lib2to3.pgen2.driver": _load_module("molt_test_lib2to3_pgen2_driver", {str(STDLIB_ROOT / "lib2to3" / "pgen2" / "driver.py")!r}),
    "lib2to3.pgen2.grammar": _load_module("molt_test_lib2to3_pgen2_grammar", {str(STDLIB_ROOT / "lib2to3" / "pgen2" / "grammar.py")!r}),
    "lib2to3.pgen2.literals": _load_module("molt_test_lib2to3_pgen2_literals", {str(STDLIB_ROOT / "lib2to3" / "pgen2" / "literals.py")!r}),
    "lib2to3.pgen2.parse": _load_module("molt_test_lib2to3_pgen2_parse", {str(STDLIB_ROOT / "lib2to3" / "pgen2" / "parse.py")!r}),
    "lib2to3.pgen2.pgen": _load_module("molt_test_lib2to3_pgen2_pgen", {str(STDLIB_ROOT / "lib2to3" / "pgen2" / "pgen.py")!r}),
    "lib2to3.pgen2.token": _load_module("molt_test_lib2to3_pgen2_token", {str(STDLIB_ROOT / "lib2to3" / "pgen2" / "token.py")!r}),
    "lib2to3.pgen2.tokenize": _load_module("molt_test_lib2to3_pgen2_tokenize", {str(STDLIB_ROOT / "lib2to3" / "pgen2" / "tokenize.py")!r}),
    "ensurepip.__main__": _load_module("molt_test_ensurepip___main__", {str(STDLIB_ROOT / "ensurepip" / "__main__.py")!r}),
}}

checks = {{}}
for name, module in modules.items():
    try:
        getattr(module, "sentinel")
    except RuntimeError as exc:
        checks[name] = (
            "only an intrinsic-first stub is available" in str(exc)
            and "molt_capabilities_has" not in module.__dict__
        )
    else:
        checks[name] = False

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_stub_surface_batch_al() -> None:
    proc = subprocess.run(
        [sys.executable, "-c", _PROBE],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    checks: dict[str, str] = {}
    for line in proc.stdout.splitlines():
        if line.startswith("CHECK|"):
            _, key, value = line.split("|", 2)
            checks[key] = value
    assert checks == {
        "ensurepip.__main__": "True",
        "lib2to3.__main__": "True",
        "lib2to3.btm_matcher": "True",
        "lib2to3.btm_utils": "True",
        "lib2to3.fixer_base": "True",
        "lib2to3.fixer_util": "True",
        "lib2to3.main": "True",
        "lib2to3.patcomp": "True",
        "lib2to3.pgen2": "True",
        "lib2to3.pgen2.conv": "True",
        "lib2to3.pgen2.driver": "True",
        "lib2to3.pgen2.grammar": "True",
        "lib2to3.pgen2.literals": "True",
        "lib2to3.pgen2.parse": "True",
        "lib2to3.pgen2.pgen": "True",
        "lib2to3.pgen2.token": "True",
        "lib2to3.pgen2.tokenize": "True",
        "lib2to3.pygram": "True",
        "lib2to3.pytree": "True",
        "lib2to3.refactor": "True",
    }
