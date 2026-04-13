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


def _native_bootstrap_target_dirs(env: dict[str, str]) -> tuple[Path, Path]:
    default_target_dir = ROOT / "target"
    raw_target = env.get("CARGO_TARGET_DIR", "").strip()
    target_dir = Path(raw_target).expanduser() if raw_target else default_target_dir
    raw_diff_target = env.get("MOLT_DIFF_CARGO_TARGET_DIR", "").strip()
    diff_target_dir = (
        Path(raw_diff_target).expanduser() if raw_diff_target else target_dir
    )
    return target_dir, diff_target_dir


def test_native_bootstrap_target_dir_respects_explicit_env_override() -> None:
    env = {
        "CARGO_TARGET_DIR": "/tmp/molt-native-target",
        "MOLT_DIFF_CARGO_TARGET_DIR": "/tmp/molt-native-diff-target",
    }

    target_dir, diff_target_dir = _native_bootstrap_target_dirs(env)

    assert target_dir == Path("/tmp/molt-native-target")
    assert diff_target_dir == Path("/tmp/molt-native-diff-target")


def test_native_bootstrap_target_dir_defaults_to_repo_target() -> None:
    target_dir, diff_target_dir = _native_bootstrap_target_dirs({})

    assert target_dir == ROOT / "target"
    assert diff_target_dir == target_dir


def _build_and_run(tmp_path: Path, source: str, name: str) -> subprocess.CompletedProcess[str]:
    return _build_and_run_with_env(
        tmp_path,
        source,
        name,
        session_id=NATIVE_BOOTSTRAP_SESSION_ID,
        cache_dir=ROOT / ".molt_cache",
        backend="cranelift",
    )


def _build_and_run_with_env(
    tmp_path: Path,
    source: str,
    name: str,
    *,
    session_id: str,
    cache_dir: Path,
    backend: str,
    source_relpath: str | None = None,
    extra_files: dict[str, str] | None = None,
    extra_env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    src_path = tmp_path / source_relpath if source_relpath is not None else tmp_path / f"{name}.py"
    out_path = tmp_path / name
    src_path.parent.mkdir(parents=True, exist_ok=True)
    if extra_files:
        for rel_path, contents in extra_files.items():
            path = tmp_path / rel_path
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(contents)
    src_path.write_text(source)

    env = os.environ.copy()
    env["PYTHONPATH"] = str(SRC_DIR)
    env["MOLT_SESSION_ID"] = session_id
    target_dir, diff_target_dir = _native_bootstrap_target_dirs(env)
    target_dir.mkdir(parents=True, exist_ok=True)
    diff_target_dir.mkdir(parents=True, exist_ok=True)
    env["CARGO_TARGET_DIR"] = str(target_dir)
    env["MOLT_DIFF_CARGO_TARGET_DIR"] = str(diff_target_dir)
    env["MOLT_CACHE"] = str(cache_dir)
    env["MOLT_DIFF_ROOT"] = str(ROOT / "tmp" / "diff")
    env["MOLT_DIFF_TMPDIR"] = str(ROOT / "tmp")
    env["UV_CACHE_DIR"] = str(ROOT / ".uv-cache")
    env["TMPDIR"] = str(ROOT / "tmp")
    env["MOLT_BACKEND_DAEMON"] = "0"
    if extra_env:
        env.update(extra_env)

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
            "--backend",
            backend,
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


def _build_and_run_package_bootstrap(
    tmp_path: Path,
    source: str,
    name: str,
    *,
    cache_suffix: str,
) -> subprocess.CompletedProcess[str]:
    return _build_and_run_with_env(
        tmp_path,
        source,
        name,
        session_id=f"{NATIVE_BOOTSTRAP_SESSION_ID}-package-{cache_suffix}",
        cache_dir=ROOT / f".molt_cache-package-{cache_suffix}",
        backend="cranelift",
        source_relpath="pkg/main.py",
        extra_files={
            "pkg/__init__.py": "",
            "pkg/helper.py": (
                "import os\n"
                "import sys\n"
                "from .sibling import SIBLING\n"
                "\n"
                "class Helper:\n"
                "    def describe(self):\n"
                "        return 'helper-ok'\n"
                "\n"
                "def identify():\n"
                "    return (__name__, __package__, sys.__name__, os.__name__, SIBLING)\n"
                "\n"
                "def ping():\n"
                "    return SIBLING\n"
            ),
            "pkg/sibling.py": "SIBLING = 'sibling-ok'\n",
            "pkg/subpkg/__init__.py": "",
            "pkg/subpkg/leaf.py": (
                "import os\n"
                "import sys\n"
                "\n"
                "def describe_leaf():\n"
                "    return (__name__, __package__, sys.__name__, os.__name__)\n"
            ),
        },
        extra_env={"MOLT_MODULE_ROOTS": str(tmp_path)},
    )


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


def test_native_import_math_preserves_stdio_bootstrap(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        "print('PRE')\n__import__('math')\nprint('POST')\n",
        "import_math_stdio",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["PRE", "POST"]


def test_native_types_prepare_class_honors_callable_metaclass_prepare(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        "import types\n"
        "class MetaFactory:\n"
        "    def __prepare__(self, name, bases, **kw):\n"
        "        return {'sentinel': 'ok'}\n"
        "    def __call__(self, name, bases, namespace, **kw):\n"
        "        return type(name, bases, dict(namespace))\n"
        "meta, namespace, kwds = types.prepare_class('Box', (), {'metaclass': MetaFactory()})\n"
        "print(namespace['sentinel'])\n"
        "print(type(meta).__name__)\n"
        "print(kwds == {})\n",
        "types_prepare_callable_meta",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["ok", "MetaFactory", "True"]


def test_native_generic_alias_subclass_propagates_classcell(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        "from types import GenericAlias\n"
        "print('PRE')\n"
        "class _CallableGenericAlias(GenericAlias):\n"
        "    __slots__ = ()\n"
        "    def __new__(cls, origin, args):\n"
        "        return super().__new__(cls, origin, args)\n"
        "print('POST', _CallableGenericAlias(list, (int,)).__args__)\n",
        "generic_alias_classcell",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["PRE", "POST (<class 'int'>,)"]


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


def test_native_local_from_import_direct_call_executes(tmp_path: Path) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        "from helper import ping\nping()\n",
        "local_from_import_direct_call",
        session_id=f"{NATIVE_BOOTSTRAP_SESSION_ID}-from-import-local",
        cache_dir=ROOT / ".molt_cache-import-local",
        backend="cranelift",
        extra_files={
            "helper.py": "def ping():\n    print('ok')\n",
        },
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "ok"


def test_native_relative_from_import_direct_call_executes(tmp_path: Path) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        "from .helper import ping\nping()\n",
        "relative_from_import_direct_call",
        session_id=f"{NATIVE_BOOTSTRAP_SESSION_ID}-from-import-relative",
        cache_dir=ROOT / ".molt_cache-import-relative",
        backend="cranelift",
        source_relpath="pkg/main.py",
        extra_files={
            "pkg/__init__.py": "",
            "pkg/helper.py": "def ping():\n    print('ok')\n",
        },
        extra_env={"MOLT_MODULE_ROOTS": str(tmp_path)},
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "ok"


def test_native_package_entry_bootstrap_tracks_module_identity(
    tmp_path: Path,
) -> None:
    run = _build_and_run_package_bootstrap(
        tmp_path,
        (
            "import os\n"
            "import sys\n"
            "import pkg.helper as helper\n"
            "from pkg.helper import ping\n"
            "from .subpkg.leaf import describe_leaf\n"
            "from .sibling import SIBLING\n"
            "print(__name__)\n"
            "print(__package__)\n"
            "print(helper.identify())\n"
            "print(ping())\n"
            "print(describe_leaf())\n"
            "print(SIBLING)\n"
            "print(sys.__name__)\n"
            "print(os.__name__)\n"
        ),
        "package_entry_bootstrap_identity",
        cache_suffix="identity",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "__main__",
        "pkg",
        "('pkg.helper', 'pkg', 'sys', 'os', 'sibling-ok')",
        "sibling-ok",
        "('pkg.subpkg.leaf', 'pkg.subpkg', 'sys', 'os')",
        "sibling-ok",
        "sys",
        "os",
    ]


def test_native_package_entry_direct_import_and_from_import_bindings_are_resolved(
    tmp_path: Path,
) -> None:
    run = _build_and_run_package_bootstrap(
        tmp_path,
        (
            "import pkg.helper as helper\n"
            "from pkg.helper import ping\n"
            "print(helper.__name__)\n"
            "print(helper.__package__)\n"
            "print(ping())\n"
        ),
        "package_entry_import_bindings",
        cache_suffix="imports",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "pkg.helper",
        "pkg",
        "sibling-ok",
    ]


def test_native_package_entry_alias_imports_and_sys_modules_identity_are_resolved(
    tmp_path: Path,
) -> None:
    run = _build_and_run_package_bootstrap(
        tmp_path,
        (
            "import sys\n"
            "import pkg.helper as helper_alias\n"
            "from pkg.helper import Helper as HelperAlias\n"
            "from pkg.helper import ping as ping_alias\n"
            "print(helper_alias is sys.modules['pkg.helper'])\n"
            "print(HelperAlias is helper_alias.Helper)\n"
            "print(ping_alias is helper_alias.ping)\n"
            "print(HelperAlias().describe())\n"
            "print(ping_alias())\n"
        ),
        "package_entry_alias_identity",
        cache_suffix="aliases",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "True",
        "True",
        "True",
        "helper-ok",
        "sibling-ok",
    ]


def test_native_package_entry_alias_imports_preserve_os_and_sys_identity(
    tmp_path: Path,
) -> None:
    run = _build_and_run_package_bootstrap(
        tmp_path,
        (
            "import os as os_alias\n"
            "import sys as sys_alias\n"
            "import pkg.helper as helper_alias\n"
            "print(sys_alias is sys.modules['sys'])\n"
            "print(os_alias is sys.modules['os'])\n"
            "print(helper_alias is sys.modules['pkg.helper'])\n"
            "print(sys_alias.__name__)\n"
            "print(os_alias.__name__)\n"
            "print(helper_alias.__package__)\n"
        ),
        "package_entry_alias_imports_os_sys",
        cache_suffix="os-sys-aliases",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "True",
        "True",
        "True",
        "sys",
        "os",
        "pkg",
    ]


def test_native_package_entry_class_import_alias_preserves_metadata_and_identity(
    tmp_path: Path,
) -> None:
    run = _build_and_run_package_bootstrap(
        tmp_path,
        (
            "import sys\n"
            "import pkg.helper as helper_alias\n"
            "from pkg.helper import Helper as HelperAlias\n"
            "print(HelperAlias is helper_alias.Helper)\n"
            "print(HelperAlias is sys.modules['pkg.helper'].Helper)\n"
            "print(HelperAlias.__module__)\n"
            "print(HelperAlias.__qualname__)\n"
            "print(HelperAlias().describe())\n"
        ),
        "package_entry_class_import_alias_metadata",
        cache_suffix="class-alias",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "True",
        "True",
        "pkg.helper",
        "Helper",
        "helper-ok",
    ]


def test_native_package_entry_submodule_alias_import_preserves_package_identity(
    tmp_path: Path,
) -> None:
    run = _build_and_run_package_bootstrap(
        tmp_path,
        (
            "import sys\n"
            "import pkg.subpkg.leaf as leaf_alias\n"
            "from .subpkg.leaf import describe_leaf as describe_leaf_alias\n"
            "print(leaf_alias is sys.modules['pkg.subpkg.leaf'])\n"
            "print(leaf_alias.__name__)\n"
            "print(leaf_alias.__package__)\n"
            "print(describe_leaf_alias())\n"
        ),
        "package_entry_submodule_alias_identity",
        cache_suffix="submodule-alias",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "True",
        "pkg.subpkg.leaf",
        "pkg.subpkg",
        "('pkg.subpkg.leaf', 'pkg.subpkg', 'sys', 'os')",
    ]


def test_native_package_main_entrypoint_preserves_main_module_identity(
    tmp_path: Path,
) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "import sys\n"
            "import pkg.helper as helper_alias\n"
            "from .helper import Helper as HelperAlias\n"
            "from .helper import ping as ping_alias\n"
            "print(__name__)\n"
            "print(__package__)\n"
            "print(sys.modules['__main__'] is sys.modules[__name__])\n"
            "print(helper_alias is sys.modules['pkg.helper'])\n"
            "print(HelperAlias is helper_alias.Helper)\n"
            "print(ping_alias is helper_alias.ping)\n"
            "print(HelperAlias().describe())\n"
            "print(ping_alias())\n"
        ),
        "package_main_entrypoint_identity",
        session_id=f"{NATIVE_BOOTSTRAP_SESSION_ID}-package-main",
        cache_dir=ROOT / ".molt_cache-package-main",
        backend="cranelift",
        source_relpath="pkg/__main__.py",
        extra_files={
            "pkg/__init__.py": "",
            "pkg/helper.py": (
                "from .sibling import SIBLING\n"
                "\n"
                "class Helper:\n"
                "    def describe(self):\n"
                "        return 'helper-ok'\n"
                "\n"
                "def ping():\n"
                "    return SIBLING\n"
            ),
            "pkg/sibling.py": "SIBLING = 'sibling-ok'\n",
        },
        extra_env={"MOLT_MODULE_ROOTS": str(tmp_path)},
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "__main__",
        "pkg",
        "True",
        "True",
        "True",
        "True",
        "helper-ok",
        "sibling-ok",
    ]


def test_native_top_level_alias_imports_preserve_os_sys_identity(
    tmp_path: Path,
) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "import os\n"
            "import sys\n"
            "import os as os_alias\n"
            "import sys as sys_alias\n"
            "print(sys_alias is sys.modules['sys'])\n"
            "print(os_alias is sys.modules['os'])\n"
            "print(sys_alias is sys)\n"
            "print(os_alias is os)\n"
            "print(sys_alias.__name__)\n"
            "print(os_alias.__name__)\n"
        ),
        "top_level_alias_identity",
        session_id=f"{NATIVE_BOOTSTRAP_SESSION_ID}-top-level-alias",
        cache_dir=ROOT / ".molt_cache-top-level-alias",
        backend="cranelift",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "True",
        "True",
        "True",
        "True",
        "sys",
        "os",
    ]


def test_native_llvm_json_loads_with_kwonly_defaults_executes(tmp_path: Path) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "import json\n"
            "class C:\n"
            "    def __init__(self, dim: int = 768, n_layers: int = 22):\n"
            "        self.dim = dim\n"
            "        self.n_layers = n_layers\n"
            "    @classmethod\n"
            "    def make(cls, s: str):\n"
            "        data = json.loads(s)\n"
            "        return cls(dim=data['dim'])\n"
            "obj = C.make('{\"dim\":5}')\n"
            "print(obj.dim)\n"
            "print(obj.n_layers)\n"
        ),
        "llvm_json_loads_kwonly_defaults",
        session_id=f"{NATIVE_BOOTSTRAP_SESSION_ID}-llvm",
        cache_dir=ROOT / ".molt_cache",
        backend="llvm",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["5", "22"]


def test_native_llvm_rebuild_reuses_shared_stdlib_without_duplicate_symbols(
    tmp_path: Path,
) -> None:
    source = (
        "import json\n"
        "class C:\n"
        "    def __init__(self, dim: int = 768, n_layers: int = 22):\n"
        "        self.dim = dim\n"
        "        self.n_layers = n_layers\n"
        "    @classmethod\n"
        "    def make(cls, s: str):\n"
        "        data = json.loads(s)\n"
        "        return cls(dim=data['dim'])\n"
        "obj = C.make('{\"dim\":5}')\n"
        "print(obj.dim)\n"
        "print(obj.n_layers)\n"
    )
    cache_dir = ROOT / ".molt_cache-llvm-stdlib-reuse"

    first = _build_and_run_with_env(
        tmp_path / "first",
        source,
        "llvm_json_first",
        session_id=f"{NATIVE_BOOTSTRAP_SESSION_ID}-llvm",
        cache_dir=cache_dir,
        backend="llvm",
    )
    second = _build_and_run_with_env(
        tmp_path / "second",
        source,
        "llvm_json_second",
        session_id=f"{NATIVE_BOOTSTRAP_SESSION_ID}-llvm",
        cache_dir=cache_dir,
        backend="llvm",
    )

    assert first.returncode == 0, first.stdout + first.stderr
    assert second.returncode == 0, second.stdout + second.stderr
    assert first.stdout.strip().splitlines() == ["5", "22"]
    assert second.stdout.strip().splitlines() == ["5", "22"]


def test_native_import_os_is_clean(tmp_path: Path) -> None:
    run = _build_and_run(tmp_path, "import os\nprint('ok')\n", "import_os")
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "ok"


def test_native_array_repeat_semantics_executes(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "from array import array\n"
            "a = array('f', [1.5, -2.0])\n"
            "print((a * 3).tolist())\n"
            "a *= 2\n"
            "print(a.tolist())\n"
            "print((a * 0).tolist())\n"
        ),
        "array_repeat_semantics",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "[1.5, -2.0, 1.5, -2.0, 1.5, -2.0]",
        "[1.5, -2.0, 1.5, -2.0]",
        "[]",
    ]


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


def test_native_tensor_linear_f32_contiguous_fast_path(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "from array import array\n"
            "from molt.gpu import to_device\n"
            "from molt.gpu.tensor import Tensor\n"
            "x = Tensor(to_device(array('f', [1.0, 2.0, 3.0, 4.0])), shape=(2, 2))\n"
            "w = Tensor(to_device(array('f', [5.0, 6.0, 7.0, 8.0, 9.0, 10.0])), shape=(3, 2))\n"
            "out = x.linear(w)\n"
            "print(out._buf.format_char)\n"
            "print(out.to_list())\n"
        ),
        "tensor_linear_f32_contiguous",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "f",
        "[[17.0, 23.0, 29.0], [39.0, 53.0, 67.0]]",
    ]


def test_native_tensor_broadcast_f32_contiguous_fast_path(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "from array import array\n"
            "from molt.gpu import to_device\n"
            "from molt.gpu.tensor import Tensor\n"
            "a = Tensor(to_device(array('f', [1.0, 2.0, 3.0, 4.0])), shape=(2, 2))\n"
            "b = Tensor(to_device(array('f', [10.0, 20.0])), shape=(1, 2))\n"
            "out = a + b\n"
            "print(out._buf.format_char)\n"
            "print(out.to_list())\n"
        ),
        "tensor_broadcast_f32_contiguous",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "f",
        "[[11.0, 22.0], [13.0, 24.0]]",
    ]


def test_native_tensor_permute_f32_contiguous_fast_path(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "from array import array\n"
            "from molt.gpu import to_device\n"
            "from molt.gpu.tensor import Tensor\n"
            "t = Tensor(to_device(array('f', [1.0, 2.0, 3.0, 4.0])), shape=(1, 2, 1, 2))\n"
            "out = t.permute(0, 2, 1, 3)\n"
            "print(out._buf.format_char)\n"
            "print(out.to_list())\n"
        ),
        "tensor_permute_f32_contiguous",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "f",
        "[[[[1.0, 2.0], [3.0, 4.0]]]]",
    ]


def test_native_tensor_matmul_f32_contiguous_fast_path(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "from array import array\n"
            "from molt.gpu import to_device\n"
            "from molt.gpu.tensor import Tensor\n"
            "a = Tensor(to_device(array('f', [1.0, 2.0, 3.0, 4.0])), shape=(2, 2))\n"
            "b = Tensor(to_device(array('f', [5.0, 6.0, 7.0, 8.0])), shape=(2, 2))\n"
            "out = a @ b\n"
            "print(out._buf.format_char)\n"
            "print(out.to_list())\n"
        ),
        "tensor_matmul_f32_contiguous",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "f",
        "[[19.0, 22.0], [43.0, 50.0]]",
    ]


def test_native_tensor_linear_split_last_dim_f32_contiguous_fast_path(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "from array import array\n"
            "from molt.gpu import to_device\n"
            "from molt.gpu.tensor import Tensor\n"
            "x = Tensor(to_device(array('f', [1.0, 2.0, 3.0, 4.0])), shape=(2, 2))\n"
            "w = Tensor(to_device(array('f', [1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 2.0, 0.0, 0.0, 2.0])), shape=(5, 2))\n"
            "left, right = x.linear_split_last_dim(w, (2, 3))\n"
            "print(left._buf.format_char)\n"
            "print(right._buf.format_char)\n"
            "print(left.to_list())\n"
            "print(right.to_list())\n"
        ),
        "tensor_linear_split_last_dim_f32_contiguous",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "f",
        "f",
        "[[1.0, 2.0], [3.0, 4.0]]",
        "[[3.0, 2.0, 4.0], [7.0, 6.0, 8.0]]",
    ]


def test_native_tensor_linear_squared_relu_gate_interleaved_f32_contiguous_fast_path(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "from array import array\n"
            "from molt.gpu import to_device\n"
            "from molt.gpu.tensor import Tensor\n"
            "x = Tensor(to_device(array('f', [1.0, 2.0, 3.0, 4.0])), shape=(2, 2))\n"
            "w = Tensor(to_device(array('f', [1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 2.0, 0.0])), shape=(4, 2))\n"
            "out = x.linear_squared_relu_gate_interleaved(w)\n"
            "print(out._buf.format_char)\n"
            "print(out.to_list())\n"
        ),
        "tensor_linear_squared_relu_gate_interleaved_f32_contiguous",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "f",
        "[[2.0, 18.0], [36.0, 294.0]]",
    ]


def test_native_tensor_functional_linear_family_fast_path(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "from array import array\n"
            "from molt.gpu import to_device\n"
            "from molt.gpu.tensor import Tensor, tensor_linear, tensor_linear_split_last_dim, tensor_linear_squared_relu_gate_interleaved\n"
            "x = Tensor(to_device(array('f', [1.0, 2.0, 3.0, 4.0])), shape=(2, 2))\n"
            "split_weight = Tensor(to_device(array('f', [1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 2.0, 0.0, 0.0, 2.0])), shape=(5, 2))\n"
            "gate_weight = Tensor(to_device(array('f', [1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 2.0, 0.0])), shape=(4, 2))\n"
            "proj = tensor_linear(x, split_weight)\n"
            "left, right = tensor_linear_split_last_dim(x, split_weight, (2, 3))\n"
            "gated = tensor_linear_squared_relu_gate_interleaved(x, gate_weight)\n"
            "print(proj._buf.format_char)\n"
            "print(left._buf.format_char)\n"
            "print(right._buf.format_char)\n"
            "print(gated._buf.format_char)\n"
            "print(left.to_list())\n"
            "print(right.to_list())\n"
            "print(gated.to_list())\n"
        ),
        "tensor_functional_linear_family_f32_contiguous",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "f",
        "f",
        "f",
        "f",
        "[[1.0, 2.0], [3.0, 4.0]]",
        "[[3.0, 2.0, 4.0], [7.0, 6.0, 8.0]]",
        "[[2.0, 18.0], [36.0, 294.0]]",
    ]


def test_native_tensor_functional_permute_and_softmax_last_axis_fast_path(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "from array import array\n"
            "from molt.gpu import to_device\n"
            "from molt.gpu.tensor import Tensor, tensor_permute_dims, tensor_softmax_last_axis\n"
            "t = Tensor(to_device(array('f', [1.0, 2.0, 3.0, 4.0])), shape=(1, 2, 1, 2))\n"
            "permuted = tensor_permute_dims(t, (0, 2, 1, 3))\n"
            "softmaxed = tensor_softmax_last_axis(Tensor(to_device(array('f', [1.0, 2.0, 3.0, 4.0])), shape=(2, 2)))\n"
            "print(permuted._buf.format_char)\n"
            "print(permuted.to_list())\n"
            "print(softmaxed._buf.format_char)\n"
            "print(softmaxed.shape)\n"
        ),
        "tensor_functional_permute_softmax_f32_contiguous",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "f",
        "[[[[1.0, 2.0], [3.0, 4.0]]]]",
        "f",
        "(2, 2)",
    ]


def test_native_tensor_functional_reshape_and_data_list_fast_path(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "from array import array\n"
            "from molt.gpu import to_device\n"
            "from molt.gpu.tensor import Tensor, tensor_data_list, tensor_reshape_view\n"
            "t = Tensor(to_device(array('f', [1.0, 2.0, 3.0, 4.0])), shape=(4,))\n"
            "reshaped = tensor_reshape_view(t, (2, 2))\n"
            "print(reshaped._buf.format_char)\n"
            "print(reshaped.to_list())\n"
            "print(tensor_data_list(reshaped))\n"
        ),
        "tensor_functional_reshape_data_list_f32",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "f",
        "[[1.0, 2.0], [3.0, 4.0]]",
        "[1.0, 2.0, 3.0, 4.0]",
    ]


def test_native_tensor_scaled_dot_product_attention_fast_path(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "from array import array\n"
            "from molt.gpu import to_device\n"
            "from molt.gpu.tensor import Tensor, tensor_scaled_dot_product_attention\n"
            "q = Tensor(to_device(array('f', [1.0, 0.0, 0.0, 1.0])), shape=(1, 1, 2, 2))\n"
            "k = Tensor(to_device(array('f', [1.0, 0.0, 0.0, 1.0])), shape=(1, 1, 2, 2))\n"
            "v = Tensor(to_device(array('f', [10.0, 1.0, 2.0, 20.0])), shape=(1, 1, 2, 2))\n"
            "mask = Tensor(to_device(array('f', [0.0, -1.0e9, -1.0e9, 0.0])), shape=(1, 1, 2, 2))\n"
            "out = tensor_scaled_dot_product_attention(q, k, v, mask, 1.0)\n"
            "print(out._buf.format_char)\n"
            "print(out.to_list())\n"
        ),
        "tensor_scaled_dot_product_attention",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "f",
        "[[[[10.0, 1.0], [2.0, 20.0]]]]",
    ]


def test_native_tensor_functional_take_rows_fast_path(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "from array import array\n"
            "from molt.gpu import to_device\n"
            "from molt.gpu.tensor import Tensor, tensor_take_rows\n"
            "weight = Tensor(to_device(array('f', [1.0, 2.0, 3.0, 4.0, 5.0, 6.0])), shape=(3, 2))\n"
            "out = tensor_take_rows(weight, [2, 0])\n"
            "print(out._buf.format_char)\n"
            "print(out.to_list())\n"
        ),
        "tensor_functional_take_rows_f32",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "f",
        "[[5.0, 6.0], [1.0, 2.0]]",
    ]


def test_native_tensor_softmax_f32_contiguous_fast_path(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "from array import array\n"
            "from molt.gpu import to_device\n"
            "from molt.gpu.tensor import Tensor\n"
            "t = Tensor(to_device(array('f', [1.0, 2.0, 3.0, 4.0])), shape=(2, 2))\n"
            "out = t.softmax(axis=-1)\n"
            "print(out._buf.format_char)\n"
            "print(out.shape)\n"
            "print(out.to_list())\n"
        ),
        "tensor_softmax_f32_contiguous",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    lines = run.stdout.strip().splitlines()
    assert lines[0] == "f"
    assert lines[1] == "(2, 2)"


def test_native_tensor_ndim_property_fast_path(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "from array import array\n"
            "from molt.gpu import to_device\n"
            "from molt.gpu.tensor import Tensor\n"
            "t = Tensor(to_device(array('f', [1.0, 2.0, 3.0, 4.0])), shape=(2, 2))\n"
            "print(t.shape)\n"
            "print(t.ndim)\n"
        ),
        "tensor_ndim_property",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "(2, 2)",
        "2",
    ]


def test_native_intrinsics_module_exports_module_form_api(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "import _intrinsics as intr\n"
            "print(callable(intr.require_intrinsic))\n"
            "print(callable(intr.load_intrinsic))\n"
            "print(intr.runtime_active())\n"
            "print(intr.load_intrinsic('molt_gpu_buffer_to_list') is not None)\n"
            "print(intr.load_intrinsic('molt_gpu_tensor__zeros') is not None)\n"
            "print(intr.load_intrinsic('molt_gpu_tensor__tensor_scaled_dot_product_attention') is not None)\n"
            "print(intr.load_intrinsic('molt_gpu_turboquant_attention_packed') is not None)\n"
            "print(intr.load_intrinsic('molt_missing_intrinsic') is None)\n"
        ),
        "intrinsics_module_api",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "True",
        "True",
        "True",
        "True",
        "True",
        "True",
        "True",
        "True",
    ]


def test_native_tensor_rms_norm_f32_contiguous_fast_path(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "from array import array\n"
            "from molt.gpu import to_device\n"
            "from molt.gpu.tensor import Tensor\n"
            "t = Tensor(to_device(array('f', [3.0, 4.0, 0.0, 5.0])), shape=(2, 2))\n"
            "out = t.rms_norm(0.0)\n"
            "print(out._buf.format_char)\n"
            "print([[round(v, 6) for v in row] for row in out.to_list()])\n"
        ),
        "tensor_rms_norm_f32_contiguous",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "f",
        "[[0.848528, 1.131371], [0.0, 1.414214]]",
    ]


def test_native_tensor_squared_relu_gate_interleaved_f32_contiguous_fast_path(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "from array import array\n"
            "from molt.gpu import to_device\n"
            "from molt.gpu.tensor import Tensor\n"
            "t = Tensor(to_device(array('f', [1.0, 10.0, -2.0, 20.0, 3.0, 30.0, 4.0, 40.0])), shape=(1, 8))\n"
            "out = t.squared_relu_gate_interleaved()\n"
            "print(out._buf.format_char)\n"
            "print(out.to_list())\n"
        ),
        "tensor_squared_relu_gate_interleaved_f32_contiguous",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "f",
        "[[10.0, 0.0, 270.0, 640.0]]",
    ]


def test_native_bound_method_positional_fast_path_preserves_semantics(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "class Acc:\n"
            "    def add3(self, a, b, c):\n"
            "        return a + b + c\n"
            "obj = Acc()\n"
            "print(obj.add3(1, 2, 3))\n"
        ),
        "bound_method_positional_fast_path",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "6"


def test_native_bound_method_defaults_still_take_full_binding_path(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "class Acc:\n"
            "    def add(self, a, b=2):\n"
            "        return a + b\n"
            "obj = Acc()\n"
            "print(obj.add(1))\n"
        ),
        "bound_method_defaults_binding",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "3"


def test_native_callable_object_positional_fast_path_preserves_semantics(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "class Acc:\n"
            "    def __call__(self, a, b, c):\n"
            "        return a + b + c\n"
            "obj = Acc()\n"
            "print(obj(1, 2, 3))\n"
        ),
        "callable_object_positional_fast_path",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "6"


def test_native_callable_object_defaults_still_take_full_binding_path(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "class Acc:\n"
            "    def __call__(self, a, b=2):\n"
            "        return a + b\n"
            "obj = Acc()\n"
            "print(obj(1))\n"
        ),
        "callable_object_defaults_binding",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "3"


def test_native_indirect_noncallable_still_raises_typeerror(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "def f():\n"
            "    x = 1\n"
            "    x()\n"
            "f()\n"
        ),
        "indirect_noncallable_typeerror",
    )
    assert run.returncode != 0
    assert "TypeError" in run.stderr
    assert "not callable" in run.stderr


def test_native_import_typing_optional_is_clean(tmp_path: Path) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        "import typing\nfrom typing import Optional\nprint('ok')\n",
        "import_typing_optional",
        session_id="pytest-native-bootstrap-typing",
        cache_dir=ROOT / ".molt_cache-typing",
        backend="cranelift",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "ok"


def test_native_collections_abc_runtime_types_does_not_poison_next_abc_class(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "from _intrinsics import require_intrinsic as _require_intrinsic\n"
            "from abc import ABCMeta, abstractmethod\n"
            "class H(metaclass=ABCMeta):\n"
            "    __slots__ = ()\n"
            "    @abstractmethod\n"
            "    def __hash__(self):\n"
            "        return 0\n"
            "    @classmethod\n"
            "    def __subclasshook__(cls, C):\n"
            "        return NotImplemented\n"
            "_payload = _require_intrinsic('molt_collections_abc_runtime_types')()\n"
            "class K(metaclass=ABCMeta):\n"
            "    __slots__ = ()\n"
            "    @abstractmethod\n"
            "    def __hash__(self):\n"
            "        return 0\n"
            "    @classmethod\n"
            "    def __subclasshook__(cls, C):\n"
            "        return NotImplemented\n"
            "print(type(K).__name__)\n"
        ),
        "collections_abc_runtime_types_no_poison",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "ABCMeta"


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
        backend="cranelift",
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
        backend="cranelift",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["160", "Tensor"]


def test_native_load_safetensors_mapping_get_returns_tensor_and_default(
    tmp_path: Path,
) -> None:
    safetensors_path = tmp_path / "multi_get.safetensors"
    _write_safetensors_fixture(safetensors_path, count=8)
    run = _build_and_run_with_env(
        tmp_path,
        (
            "from molt.gpu.interop import load_safetensors\n"
            f"weights = load_safetensors({str(safetensors_path)!r})\n"
            "print(type(weights.get('t0')).__name__)\n"
            "print(weights.get('missing', 'fallback'))\n"
        ),
        "load_safetensors_mapping_get",
        session_id="pytest-native-bootstrap-safetensors-get",
        cache_dir=ROOT / ".molt_cache-safetensors-get",
        backend="cranelift",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["Tensor", "fallback"]


def test_native_dict_annotation_does_not_force_dict_get_fast_path(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "class MappingLike:\n"
            "    def __init__(self):\n"
            "        self.data = {'x': 7}\n"
            "    def get(self, key, default=None):\n"
            "        return self.data.get(key, default)\n"
            "def f(state: dict):\n"
            "    print(state.get('x', 'fallback'))\n"
            "f(MappingLike())\n"
        ),
        "dict_annotation_mapping_get",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "7"


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


def test_native_abc_register_builtin_iterator_type(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "import abc\n"
            "class IterABC(metaclass=abc.ABCMeta):\n"
            "    pass\n"
            "IterABC.register(type(iter(b'')))\n"
            "print('ok')\n"
        ),
        "abc_register_builtin_iterator_type",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "ok"


def test_native_abc_register_builtin_iterator_type_on_derived_abc(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "import abc\n"
            "class Iterable(metaclass=abc.ABCMeta):\n"
            "    pass\n"
            "class Iterator(Iterable):\n"
            "    pass\n"
            "Iterator.register(type(iter(b'')))\n"
            "print('ok')\n"
        ),
        "abc_register_builtin_iterator_type_derived",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "ok"


def test_native_abc_register_builtin_iterator_family_on_derived_abc(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "import abc\n"
            "class Iterable(metaclass=abc.ABCMeta):\n"
            "    pass\n"
            "class Iterator(Iterable):\n"
            "    pass\n"
            "types = [\n"
            "    type(iter(b'')),\n"
            "    type(iter(bytearray())),\n"
            "    type(iter({}.keys())),\n"
            "    type(iter({}.values())),\n"
            "    type(iter({}.items())),\n"
            "    type(iter([])),\n"
            "    type(reversed([])),\n"
            "    type(iter(range(0))),\n"
            "    type(iter(set())),\n"
            "    type(iter('')),\n"
            "    type(iter(())),\n"
            "    type(zip()),\n"
            "]\n"
            "for tp in types:\n"
            "    Iterator.register(tp)\n"
            "print('ok')\n"
        ),
        "abc_register_builtin_iterator_family",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "ok"


def test_native_abc_register_realistic_iterator_abc_shape(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "import abc\n"
            "class Iterable(metaclass=abc.ABCMeta):\n"
            "    __slots__ = ()\n"
            "    @abc.abstractmethod\n"
            "    def __iter__(self):\n"
            "        while False:\n"
            "            yield None\n"
            "    @classmethod\n"
            "    def __subclasshook__(cls, C):\n"
            "        return NotImplemented\n"
            "class Iterator(Iterable):\n"
            "    __slots__ = ()\n"
            "    @abc.abstractmethod\n"
            "    def __next__(self):\n"
            "        raise StopIteration\n"
            "    def __iter__(self):\n"
            "        return self\n"
            "    @classmethod\n"
            "    def __subclasshook__(cls, C):\n"
            "        return NotImplemented\n"
            "for tp in [type(iter(b'')), type(iter(bytearray())), type(iter({}.keys()))]:\n"
            "    Iterator.register(tp)\n"
            "print('ok')\n"
        ),
        "abc_register_realistic_iterator_shape",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "ok"


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
