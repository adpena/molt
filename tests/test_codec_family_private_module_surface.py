from __future__ import annotations

import subprocess
import sys
from pathlib import Path

import pytest


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"


def _probe_source(module_name: str, module_path: Path) -> str:
    return f"""
import builtins
import importlib.util
import sys
import types

builtins._molt_intrinsics = {{
    "molt_capabilities_has": lambda _name=None: True,
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

spec = importlib.util.spec_from_file_location({module_name!r}, {str(module_path)!r})
assert spec is not None and spec.loader is not None
module = importlib.util.module_from_spec(spec)
sys.modules[{module_name!r}] = module
spec.loader.exec_module(module)

public_names = [
    name
    for name in sorted(dir(module))
    if not name.startswith("_") and name != "annotations"
]
for name in public_names:
    print(f"ROW|{{name}}")

codec = module.getcodec("utf-8")
encoded, enc_len = codec.encode("hello")
decoded, dec_len = codec.decode(encoded)
print("CHECK|" + str(
    "molt_capabilities_has" not in public_names
    and "getcodec" in public_names
    and encoded == b"hello"
    and enc_len == 5
    and decoded == "hello"
    and dec_len == 5
))
"""


def _run_probe(module_name: str, module_path: Path) -> tuple[list[str], bool]:
    proc = subprocess.run(
        [sys.executable, "-c", _probe_source(module_name, module_path)],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    public_names: list[str] = []
    behavior_ok = False
    for line in proc.stdout.splitlines():
        prefix, *rest = line.split("|", 1)
        if prefix == "ROW":
            public_names.append(rest[0])
        elif prefix == "CHECK":
            behavior_ok = rest[0] == "True"
    return public_names, behavior_ok


@pytest.mark.parametrize(
    "module_name,module_path",
    [
        ("_molt_private_codecs_jp", STDLIB_ROOT / "_codecs_jp.py"),
        ("_molt_private_codecs_kr", STDLIB_ROOT / "_codecs_kr.py"),
        ("_molt_private_codecs_cn", STDLIB_ROOT / "_codecs_cn.py"),
        ("_molt_private_codecs_tw", STDLIB_ROOT / "_codecs_tw.py"),
        ("_molt_private_codecs_hk", STDLIB_ROOT / "_codecs_hk.py"),
        ("_molt_private_codecs_iso2022", STDLIB_ROOT / "_codecs_iso2022.py"),
    ],
)
def test_codec_family_private_surfaces_are_anchor_free(
    module_name: str, module_path: Path
) -> None:
    public_names, behavior_ok = _run_probe(module_name, module_path)
    assert "molt_capabilities_has" not in public_names
    assert behavior_ok is True
