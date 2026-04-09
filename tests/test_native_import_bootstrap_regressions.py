from __future__ import annotations

import json
import os
import struct
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SRC_DIR = ROOT / "src"
NATIVE_BOOTSTRAP_SESSION_ID = "pytest-native-bootstrap"
NATIVE_BUILD_TIMEOUT_SECS = 600


def _build_and_run(tmp_path: Path, source: str, name: str) -> subprocess.CompletedProcess[str]:
    return _build_and_run_with_env(
        tmp_path,
        source,
        name,
        session_id=NATIVE_BOOTSTRAP_SESSION_ID,
        cache_dir=ROOT / ".molt_cache",
    )


def _build_and_run_with_env(
    tmp_path: Path,
    source: str,
    name: str,
    *,
    session_id: str,
    cache_dir: Path,
) -> subprocess.CompletedProcess[str]:
    src_path = tmp_path / f"{name}.py"
    out_path = tmp_path / name
    src_path.write_text(source)

    env = os.environ.copy()
    env["PYTHONPATH"] = str(SRC_DIR)
    env["MOLT_SESSION_ID"] = session_id
    env["CARGO_TARGET_DIR"] = str(ROOT / "target")
    env["MOLT_DIFF_CARGO_TARGET_DIR"] = env["CARGO_TARGET_DIR"]
    env["MOLT_CACHE"] = str(cache_dir)
    env["MOLT_DIFF_ROOT"] = str(ROOT / "tmp" / "diff")
    env["MOLT_DIFF_TMPDIR"] = str(ROOT / "tmp")
    env["UV_CACHE_DIR"] = str(ROOT / ".uv-cache")
    env["TMPDIR"] = str(ROOT / "tmp")
    env["MOLT_BACKEND_DAEMON"] = "0"

    build = subprocess.run(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            str(src_path),
            "--target",
            "native",
            "--build-profile",
            "dev",
            "--output",
            str(out_path),
        ],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=NATIVE_BUILD_TIMEOUT_SECS,
    )
    assert build.returncode == 0, build.stdout + build.stderr

    run = subprocess.run(
        [str(out_path)],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=60,
    )
    return run


def _write_safetensors_fixture(path: Path, *, count: int) -> None:
    header: dict[str, dict[str, object]] = {}
    payload = bytearray()
    offset = 0
    for index in range(count):
        name = f"t{index}"
        values = [float(index) + 0.5]
        raw = struct.pack("<f", values[0])
        header[name] = {
            "dtype": "F32",
            "shape": [1],
            "data_offsets": [offset, offset + len(raw)],
        }
        payload.extend(raw)
        offset += len(raw)
    header_json = json.dumps(header, separators=(",", ":"), sort_keys=True).encode("utf-8")
    path.write_bytes(struct.pack("<Q", len(header_json)) + header_json + payload)


def test_native_or_short_circuit_preserves_truthy_left(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        "def left():\n    return 'X'\n\ndef f():\n    v = left() or 'Y'\n    print(v)\n\nf()\n",
        "or_truthy",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "X"


def test_native_or_short_circuit_preserves_falsy_fallback(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        "def left():\n    return ''\n\ndef f():\n    v = left() or 'Y'\n    print(v)\n\nf()\n",
        "or_falsy",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "Y"


def test_native_import_sys_is_clean(tmp_path: Path) -> None:
    run = _build_and_run(tmp_path, "import sys\nprint('ok')\n", "import_sys")
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "ok"


def test_native_false_guarded_raise_does_not_leak_pending_exception(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        "flag = False\nif flag:\n    raise RuntimeError('bad')\nprint('ok')\n",
        "false_guarded_raise",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "ok"


def test_native_import_json_is_clean(tmp_path: Path) -> None:
    run = _build_and_run(tmp_path, "import json\nprint('ok')\n", "import_json")
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "ok"


def test_native_import_os_is_clean(tmp_path: Path) -> None:
    run = _build_and_run(tmp_path, "import os\nprint('ok')\n", "import_os")
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "ok"


def test_native_os_env_snapshot_executes(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        "import os\nprint(type(os._molt_env_snapshot()).__name__)\n",
        "os_env_snapshot_executes",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "dict"


def test_native_os_listdir_executes(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        "import os\nprint(type(os.listdir('.')).__name__)\n",
        "os_listdir_executes",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "list"


def test_native_os_stat_executes(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        "import os\nprint(type(os.stat('.')).__name__)\n",
        "os_stat_executes",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "stat_result"


def test_native_import_typing_optional_is_clean(tmp_path: Path) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        "import typing\nfrom typing import Optional\nprint('ok')\n",
        "import_typing_optional",
        session_id="pytest-native-bootstrap-typing",
        cache_dir=ROOT / ".molt_cache-typing",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "ok"


def test_native_metaclass_subclass_of_base_metaclass_is_allowed(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "import abc\n"
            "class Base(metaclass=abc.ABCMeta):\n"
            "    pass\n"
            "class Meta(abc.ABCMeta):\n"
            "    pass\n"
            "class Derived(Base, metaclass=Meta):\n"
            "    pass\n"
            "print('ok')\n"
        ),
        "metaclass_subclass_allowed",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "ok"


def test_native_import_builtins_descriptor_types_are_bootstrapped(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "import builtins\n"
            "def _probe(self=None):\n"
            "    return self\n"
            "print(builtins.classmethod.__name__)\n"
            "print(type(builtins.classmethod(_probe)).__name__)\n"
            "print(builtins.staticmethod.__name__)\n"
            "print(type(builtins.staticmethod(_probe)).__name__)\n"
            "print(builtins.property.__name__)\n"
            "print(type(builtins.property()).__name__)\n"
        ),
        "import_builtins_descriptors",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "classmethod",
        "classmethod",
        "staticmethod",
        "staticmethod",
        "property",
        "property",
    ]


def test_native_repo_package_imports_include_molt_parent_package(tmp_path: Path) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "from molt.gpu.tensor import Tensor\n"
            "print('ok')\n"
        ),
        "import_molt_gpu_tensor",
        session_id="pytest-native-bootstrap-package-import",
        cache_dir=ROOT / ".molt_cache-package-import",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "ok"


def test_native_struct_pack_starargs_inside_function_remains_bound_as_tuple(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "import struct\n"
            "def f(data):\n"
            "    return struct.pack(f'{len(data)}d', *data)\n"
            "print(len(f([1.0])))\n"
            "print(len(f([2.0])))\n"
        ),
        "struct_pack_starargs_twice",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["8", "8"]


def test_native_load_safetensors_multi_entry_is_clean(tmp_path: Path) -> None:
    safetensors_path = tmp_path / "multi.safetensors"
    _write_safetensors_fixture(safetensors_path, count=160)
    run = _build_and_run_with_env(
        tmp_path,
        (
            "from molt.gpu.interop import load_safetensors\n"
            f"weights = load_safetensors({str(safetensors_path)!r})\n"
            "print(len(weights))\n"
            "print(type(weights['t0']).__name__)\n"
        ),
        "load_safetensors_multi_entry",
        session_id="pytest-native-bootstrap-safetensors",
        cache_dir=ROOT / ".molt_cache-safetensors",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["160", "Tensor"]


def test_native_tuple_loop_dynamic_unpack_list_retention_is_clean(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "import struct\n"
            "count = 1\n"
            "fmt_char = 'f'\n"
            "raw = bytes.fromhex('0000003f')\n"
            "items = [(f't{i}', {}) for i in range(160)]\n"
            "all_values = []\n"
            "for name, meta in items:\n"
            "    values = list(struct.unpack(f'<{count}{fmt_char}', raw))\n"
            "    all_values.append(values)\n"
            "print(len(all_values))\n"
            "print(all_values[0][0])\n"
        ),
        "tuple_loop_dynamic_unpack_list_retention",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["160", "0.5"]


def test_native_while_break_exits_after_first_iteration(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "n = 0\n"
            "while True:\n"
            "    n = n + 1\n"
            "    if n > 3:\n"
            "        raise RuntimeError(n)\n"
            "    break\n"
            "print(n)\n"
        ),
        "while_break_first_iteration",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "1"


def test_native_for_contains_early_return_is_clean(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "def contains(xs, value):\n"
            "    for v in xs:\n"
            "        if v == value:\n"
            "            return True\n"
            "    return False\n"
            "print(contains([1, 2, 3], 2))\n"
            "print(contains([1, 2, 3], 9))\n"
        ),
        "for_contains_early_return",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["True", "False"]


def test_native_nested_for_early_return_is_clean(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "def first_match(grid, target):\n"
            "    for row in grid:\n"
            "        for value in row:\n"
            "            if value == target:\n"
            "                return value\n"
            "    return -1\n"
            "print(first_match([[1, 2], [3, 4]], 3))\n"
            "print(first_match([[1, 2], [3, 4]], 9))\n"
        ),
        "nested_for_early_return",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["3", "-1"]


def test_native_nested_if_else_for_does_not_fallthrough_then_arm(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "def g(data):\n"
            "    flat = []\n"
            "    def walk(obj, depth):\n"
            "        if depth == 1:\n"
            "            flat.append(obj)\n"
            "        else:\n"
            "            for item in obj:\n"
            "                flat.append(item)\n"
            "    walk(data, 0)\n"
            "    print(len(flat))\n"
            "    print(type(flat[-1]).__name__)\n"
            "g([0.0] * 3)\n"
        ),
        "nested_if_else_for_no_then_fallthrough",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["3", "float"]


def test_native_dynamic_abc_slot_class_preserves_runtime_layout(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "import abc\n"
            "class Sized(metaclass=abc.ABCMeta):\n"
            "    __slots__ = ()\n"
            "    def __len__(self):\n"
            "        return 0\n"
            "class MappingView(Sized):\n"
            "    __slots__ = ('_mapping',)\n"
            "    def __init__(self, mapping):\n"
            "        self._mapping = mapping\n"
            "view = MappingView({'x': 1})\n"
            "print(type(view).__name__)\n"
            "print(view._mapping['x'])\n"
        ),
        "dynamic_abc_slot_layout",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["MappingView", "1"]


def test_native_safe_intrinsic_helper_with_tuple_subclass(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "from _intrinsics import require_intrinsic as _require_intrinsic\n"
            "def _return_version_info_default():\n"
            "    return (3, 12, 0, 'final', 0)\n"
            "def _return_empty_str():\n"
            "    return ''\n"
            "def _return_hexversion_default():\n"
            "    return 0x030C00F0\n"
            "def _safe_intrinsic(name, default=None, _ri=_require_intrinsic):\n"
            "    try:\n"
            "        fn = _ri(name)\n"
            "        if callable(fn):\n"
            "            return fn\n"
            "    except (RuntimeError, TypeError):\n"
            "        pass\n"
            "    if default is not None:\n"
            "        return default\n"
            "    return lambda *_a, **_k: None\n"
            "class version_info(tuple):\n"
            "    def __new__(cls, values):\n"
            "        return tuple.__new__(cls, values)\n"
            "_VersionInfoTuple = version_info\n"
            "_MOLT_SYS_VERSION = _safe_intrinsic('molt_sys_version', _return_empty_str)\n"
            "_MOLT_SYS_VERSION_INFO = _safe_intrinsic('molt_sys_version_info', _return_version_info_default)\n"
            "_MOLT_SYS_HEXVERSION = _safe_intrinsic('molt_sys_hexversion', _return_hexversion_default)\n"
            "def f():\n"
            "    g = globals()\n"
            "    version_text = _MOLT_SYS_VERSION() or '3.12.0 (molt)'\n"
            "    version_values = _MOLT_SYS_VERSION_INFO() or (3, 12, 0, 'final', 0)\n"
            "    hexversion_value = _MOLT_SYS_HEXVERSION() or 0x030C00F0\n"
            "    g['_raw_version_info'] = version_values\n"
            "    g['version_info'] = _VersionInfoTuple(version_values)\n"
            "    print(version_text)\n"
            "    print(version_values)\n"
            "    print(hexversion_value)\n"
            "    print(tuple(g['version_info']))\n"
            "f()\n"
        ),
        "safe_intrinsic_shape",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    lines = run.stdout.strip().splitlines()
    assert lines == [
        "3.12.0 (molt)",
        "(3, 12, 0, 'final', 0)",
        "51118320",
        "(3, 12, 0, 'final', 0)",
    ]


def test_native_nonempty_tuple_truthiness_is_true(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "def f():\n"
            "    v = (1,)\n"
            "    print(bool(v))\n"
            "    print(bool(()))\n"
            "    print(v or (2,))\n"
            "f()\n"
        ),
        "tuple_truthiness",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["True", "False", "(1,)"]


def test_native_tuple_if_merge_preserves_object_value(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "def f():\n"
            "    v = (1,)\n"
            "    if v:\n"
            "        x = v\n"
            "    else:\n"
            "        x = (2,)\n"
            "    print(type(x).__name__)\n"
            "    print(x)\n"
            "f()\n"
        ),
        "tuple_if_merge",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["tuple", "(1,)"]


def test_native_intrinsic_alias_preserves_namespace_compatible_signature(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "from _intrinsics import require_intrinsic as _ri\n"
            "_NS = globals()\n"
            "fn = _ri('molt_bootstrap_descriptor_types', _NS)\n"
            "print(type(fn).__name__)\n"
            "print(type(fn()).__name__)\n"
        ),
        "intrinsic_alias_signature",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "builtin_function_or_method",
        "tuple",
    ]
