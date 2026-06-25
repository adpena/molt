from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import struct
import sys
from pathlib import Path

import pytest

from tests.native_process_guard import run_native_test_process


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


def _build_and_run(
    tmp_path: Path, source: str, name: str
) -> subprocess.CompletedProcess[str]:
    return _build_and_run_with_env(
        tmp_path,
        source,
        name,
        session_id=NATIVE_BOOTSTRAP_SESSION_ID,
        cache_dir=ROOT / ".molt_cache",
        backend="cranelift",
    )


def _build_native_binary_with_env(
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
    extra_build_args: list[str] | None = None,
) -> tuple[Path, dict[str, str]]:
    src_path = (
        tmp_path / source_relpath
        if source_relpath is not None
        else tmp_path / f"{name}.py"
    )
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

    build_cmd = [
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
    ]
    if extra_build_args:
        build_cmd.extend(extra_build_args)

    build = run_native_test_process(
        build_cmd,
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        default_timeout=NATIVE_BUILD_TIMEOUT_SECS,
    )
    assert build.returncode == 0, build.stdout + build.stderr
    return out_path, env


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
    extra_build_args: list[str] | None = None,
    run_timeout_secs: int = 60,
) -> subprocess.CompletedProcess[str]:
    out_path, env = _build_native_binary_with_env(
        tmp_path,
        source,
        name,
        session_id=session_id,
        cache_dir=cache_dir,
        backend=backend,
        source_relpath=source_relpath,
        extra_files=extra_files,
        extra_env=extra_env,
        extra_build_args=extra_build_args,
    )

    run = run_native_test_process(
        [str(out_path)],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=run_timeout_secs,
    )
    return run


def _write_external_static_native_package_fixture(
    tmp_path: Path,
    *,
    shim_source: str,
    init_source: str,
) -> Path:
    external_root = tmp_path / "external_site"
    package_dir = external_root / "nativepkg"
    package_dir.mkdir(parents=True)
    (package_dir / "__init__.py").write_text(init_source, encoding="utf-8")
    artifact_bytes = b"native-extension"
    artifact_path = package_dir / "_native.so"
    artifact_path.write_bytes(artifact_bytes)
    manifest = {
        "schema_version": 1,
        "module": "nativepkg._native",
        "molt_c_api_version": "1",
        "abi_tag": "molt_abi1",
        "python_tag": "py3",
        "target_triple": "x86_64-unknown-linux-gnu",
        "platform_tag": "x86_64_unknown_linux_gnu",
        "capabilities": ["module.extension.exec"],
        "extension": "_native.so",
        "extension_sha256": hashlib.sha256(artifact_bytes).hexdigest(),
    }
    (package_dir / "extension_manifest.json").write_text(
        json.dumps(manifest, indent=2) + "\n",
        encoding="utf-8",
    )
    (package_dir / "_native.so.molt.py").write_text(
        shim_source,
        encoding="utf-8",
    )
    return external_root


def test_native_external_static_package_import_uses_staged_runtime_root_after_source_root_removed(
    tmp_path: Path,
) -> None:
    external_root = _write_external_static_native_package_fixture(
        tmp_path,
        shim_source="VALUE = 911\n",
        init_source="import nativepkg._native\nVALUE = nativepkg._native.VALUE\n",
    )

    out_path, build_env = _build_native_binary_with_env(
        tmp_path,
        "import nativepkg\nprint(nativepkg.VALUE)\n",
        "external_static_runtime_root",
        session_id=f"{NATIVE_BOOTSTRAP_SESSION_ID}-external-static-runtime-root",
        cache_dir=ROOT / ".molt_cache-external-static-runtime-root",
        backend="cranelift",
        extra_env={"MOLT_EXTERNAL_STATIC_PACKAGES": "nativepkg"},
        extra_build_args=[
            "--no-trusted",
            "--capabilities",
            "fs.read,module.extension.exec",
            "--lib-path",
            str(external_root),
            "--rebuild",
        ],
    )

    shutil.rmtree(external_root)
    run_env = build_env.copy()
    for key in (
        "MOLT_MODULE_ROOTS",
        "MOLT_EXTERNAL_STATIC_PACKAGES",
        "PYTHONPATH",
        "PYTHONHOME",
        "VIRTUAL_ENV",
        "MOLT_CAPABILITIES",
        "MOLT_TRUSTED",
    ):
        run_env.pop(key, None)

    run = run_native_test_process(
        [str(out_path)],
        cwd=ROOT,
        env=run_env,
        capture_output=True,
        text=True,
        timeout=60,
    )

    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "911"


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
    header_json = json.dumps(header, separators=(",", ":"), sort_keys=True).encode(
        "utf-8"
    )
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


def test_native_dict_and_set_probe_collisions_gate_equality_by_hash(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "class Key:\n"
            "    def __init__(self, value, h):\n"
            "        self.value = value\n"
            "        self.h = h\n"
            "    def __hash__(self):\n"
            "        return self.h\n"
            "    def __eq__(self, other):\n"
            "        raise RuntimeError('hash-mismatched equality was called')\n"
            "\n"
            "a = Key('a', 1)\n"
            "b = Key('b', 9)\n"
            "d = {}\n"
            "d[a] = 'A'\n"
            "d[b] = 'B'\n"
            "s = set()\n"
            "s.add(a)\n"
            "s.add(b)\n"
            "print(len(d), d[a], d[b], len(s))\n"
        ),
        "dict_set_hash_gate",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "2 A B 2"


def test_native_weakref_dict_keys_do_not_compare_hash_mismatched_referents(
    tmp_path: Path,
) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "import weakref\n"
            "\n"
            "class Target:\n"
            "    def __init__(self, h):\n"
            "        self.h = h\n"
            "    def __hash__(self):\n"
            "        return self.h\n"
            "    def __eq__(self, other):\n"
            "        raise RuntimeError('referent equality was called')\n"
            "\n"
            "a = Target(1)\n"
            "b = Target(9)\n"
            "ra = weakref.ref(a)\n"
            "rb = weakref.ref(b)\n"
            "d = {}\n"
            "d[ra] = None\n"
            "d[rb] = None\n"
            "print(len(d), d[ra] is None, d[rb] is None)\n"
        ),
        "weakref_dict_hash_gate",
        session_id="pytest-native-bootstrap-weakref-hash-gate",
        cache_dir=ROOT / ".molt_cache",
        backend="cranelift",
        extra_env={"MOLT_STDLIB_PROFILE": "full"},
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "2 True True"


def test_native_inline_list_setitem_marks_heap_refs_for_container_drop(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "events = []\n"
            "\n"
            "class Item:\n"
            "    def __del__(self):\n"
            "        events.append('dropped')\n"
            "\n"
            "def make():\n"
            "    xs = [None]\n"
            "    xs[0] = Item()\n"
            "\n"
            "make()\n"
            "print(len(events))\n"
        ),
        "list_setitem_heap_ref_flag",
    )

    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "1"


def test_native_reassigned_list_local_does_not_drop_stale_initial_owner(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "class Item:\n"
            "    pass\n"
            "\n"
            "def choose(flag):\n"
            "    xs = []\n"
            "    if flag:\n"
            "        xs = [Item()]\n"
            "    if flag:\n"
            "        return 'taken'\n"
            "    return 'skipped'\n"
            "\n"
            "print(choose(True))\n"
        ),
        "reassigned_list_local_stale_owner",
    )

    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "taken"


def test_native_import_sys_is_clean(tmp_path: Path) -> None:
    run = _build_and_run(tmp_path, "import sys\nprint('ok')\n", "import_sys")
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "ok"


def test_native_contextlib_contextmanager_exit_reads_pending_generator_exception(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "import contextlib\n"
            "\n"
            "@contextlib.contextmanager\n"
            "def managed_error(log):\n"
            "    try:\n"
            "        yield 'ok'\n"
            "    except ValueError:\n"
            "        log.append('handled')\n"
            "\n"
            "events = []\n"
            "cm = managed_error(events)\n"
            "print('enter', cm.__enter__())\n"
            "try:\n"
            "    raise ValueError('boom')\n"
            "except ValueError as exc:\n"
            "    print('exit', cm.__exit__(type(exc), exc, None))\n"
            "print(events)\n"
            "with managed_error(events) as out:\n"
            "    print('with', out)\n"
            "    raise ValueError('boom2')\n"
            "print(events)\n"
        ),
        "contextlib_contextmanager_pending_generator_exception",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "enter ok",
        "exit True",
        "['handled']",
        "with ok",
        "['handled', 'handled']",
    ]


def test_native_full_profile_importlib_machinery_sees_bootstrapped_sys_platform(
    tmp_path: Path,
) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "import sys\n"
            "import importlib.machinery\n"
            "print(type(sys.platform).__name__)\n"
            "print(len(sys.platform) > 0)\n"
            "print(importlib.machinery.EXTENSION_SUFFIXES[0])\n"
        ),
        "full_profile_importlib_machinery_sys_platform",
        session_id="pytest-native-bootstrap-full-importlib-machinery",
        cache_dir=ROOT / ".molt_cache",
        backend="cranelift",
        extra_build_args=["--stdlib-profile", "full"],
        run_timeout_secs=30,
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["str", "True", ".so"]


@pytest.mark.parametrize("split_limit", ["", "1000", "500"])
def test_native_full_profile_import_threading_survives_split_frame_transport(
    tmp_path: Path, split_limit: str
) -> None:
    cache_suffix = split_limit or "default"
    run = _build_and_run_with_env(
        tmp_path,
        "import threading\nprint(threading.__name__)\n",
        f"full_profile_import_threading_{cache_suffix}",
        session_id=f"pytest-native-bootstrap-full-threading-{cache_suffix}",
        cache_dir=ROOT / f".molt_cache-threading-split-{cache_suffix}",
        backend="cranelift",
        extra_env={"MOLT_MAX_FUNCTION_OPS": split_limit},
        extra_build_args=["--stdlib-profile", "full"],
        run_timeout_secs=60,
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "threading"


def test_native_globals_builtin_is_callable(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        "print(type(globals).__name__)\nprint(callable(globals))\nprint(type(globals()).__name__)\n",
        "globals_builtin_callable",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "builtin_function_or_method",
        "True",
        "dict",
    ]


def test_native_dynamic_metaclass_class_bootstrap_is_clean(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "class ABCMeta(type):\n"
            "    pass\n"
            "class ABC(metaclass=ABCMeta):\n"
            "    __slots__ = ()\n"
            "print('OK')\n"
        ),
        "dynamic_metaclass_bootstrap",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "OK"


def test_native_metaclass_inherits_type_prepare(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "class ABCMeta(type):\n"
            "    pass\n"
            "print(type(ABCMeta.__prepare__).__name__)\n"
            "print(callable(ABCMeta.__prepare__))\n"
        ),
        "metaclass_inherits_type_prepare",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "builtin_function_or_method",
        "True",
    ]


def test_native_import_math_preserves_stdio_bootstrap(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        "print('PRE')\n__import__('math')\nprint('POST')\n",
        "import_math_stdio",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["PRE", "POST"]


def test_native_import_math_compiles_future_feature_repr(tmp_path: Path) -> None:
    # Regression: importing a stdlib module that uses ``from __future__ import
    # annotations`` (math does — math.py:9) makes the native backend compile
    # ``__future__._Feature.__repr__``. Its ``self.optional`` / ``.mandatory`` /
    # ``.compiler_flag`` reads lower to ``guarded_field_get`` ops that a TIR
    # guard-splitting pass can leave as the CANONICAL bare ``get_attr`` SimpleIR
    # op — ``tir::lower_to_simple``'s documented no-``_original_kind`` default,
    # the same fallback every other op family already handles
    # (``index`` / ``call`` / ...). The native (Cranelift) attribute handler
    # claimed every specialized ``get_attr_*`` alias but not the canonical
    # ``get_attr``, so native batch codegen panicked:
    #   native backend: no codegen for result-producing op kind `get_attr` ...
    #   in function `__future_____Feature___repr__`
    # Building the importing program must stay clean. The deterministic
    # dispatch-level regressions live in molt-backend
    # (``fc::op_family::canonical_attribute_defaults_route_to_attrs`` and
    # ``simple_backend::native_compiles_canonical_bare_get_attr``); this is the
    # end-to-end build+run guard for the originally-reported reproduction.
    run = _build_and_run(
        tmp_path,
        "import math\nprint(math.floor(3.7))\n",
        "import_math_future_feature_repr",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "3"


def test_native_types_bootstrap_payload_is_callable(tmp_path: Path) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "from _intrinsics import require_intrinsic\n"
            "boot = require_intrinsic('molt_types_bootstrap')\n"
            "payload = boot()\n"
            "print(type(payload).__name__)\n"
            "print(type(payload['resolve_bases']).__name__)\n"
            "print(callable(payload['resolve_bases']))\n"
            "print(type(payload['prepare_class']).__name__)\n"
            "print(callable(payload['prepare_class']))\n"
        ),
        "types_bootstrap_payload_callable",
        session_id=f"{NATIVE_BOOTSTRAP_SESSION_ID}-types-bootstrap-payload",
        cache_dir=ROOT / ".molt_cache-types-bootstrap-payload",
        backend="cranelift",
        run_timeout_secs=20,
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "dict",
        "function",
        "True",
        "function",
        "True",
    ]


def test_native_types_bootstrap_descriptor_types_match_runtime_carriers(
    tmp_path: Path,
) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "from _intrinsics import require_intrinsic\n"
            "data = require_intrinsic('molt_types_bootstrap')()\n"
            "checks = [\n"
            "    data['WrapperDescriptorType'] is type(object.__init__),\n"
            "    data['MethodWrapperType'] is type(object().__str__),\n"
            "    data['MethodDescriptorType'] is type(str.join),\n"
            "    data['ClassMethodDescriptorType'] is type(dict.fromkeys),\n"
            "    data['GetSetDescriptorType'] is type(type.__getattribute__(type(lambda: None), '__code__')),\n"
            "    data['MemberDescriptorType'] is type(type.__getattribute__(type(lambda: None), '__globals__')),\n"
            "]\n"
            "for item in checks:\n"
            "    print(item)\n"
        ),
        "types_bootstrap_descriptor_types",
        session_id=f"{NATIVE_BOOTSTRAP_SESSION_ID}-types-bootstrap-descriptors",
        cache_dir=ROOT / ".molt_cache-types-bootstrap-descriptors",
        backend="cranelift",
        run_timeout_secs=20,
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["True"] * 6


def test_native_importlib_import_module_tkinter_after_find_spec_is_clean(
    tmp_path: Path,
) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "import importlib\n"
            "import importlib.util\n"
            "import _tkinter\n"
            "print('PRE')\n"
            "print(importlib.util.find_spec('tkinter') is not None)\n"
            "mod = importlib.import_module('tkinter')\n"
            "print(mod.__name__)\n"
        ),
        "importlib_import_module_tkinter_after_find_spec",
        session_id=f"{NATIVE_BOOTSTRAP_SESSION_ID}-importlib-tkinter",
        cache_dir=ROOT / ".molt_cache-importlib-tkinter",
        backend="cranelift",
        extra_build_args=["--stdlib-profile", "full"],
        run_timeout_secs=20,
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["PRE", "True", "tkinter"]


def test_native_builtin_import_tkinter_after_find_spec_is_clean(tmp_path: Path) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "import builtins\n"
            "import importlib.util\n"
            "import _tkinter\n"
            "print('PRE')\n"
            "print(importlib.util.find_spec('tkinter') is not None)\n"
            "mod = builtins.__import__('tkinter', globals(), locals(), (), 0)\n"
            "print(mod.__name__)\n"
        ),
        "builtin_import_tkinter_after_find_spec",
        session_id=f"{NATIVE_BOOTSTRAP_SESSION_ID}-builtin-tkinter",
        cache_dir=ROOT / ".molt_cache-builtin-tkinter",
        backend="cranelift",
        extra_build_args=["--stdlib-profile", "full"],
        run_timeout_secs=20,
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["PRE", "True", "tkinter"]


def test_native_imported_module_dunder_getattr_handles_missing_attr(
    tmp_path: Path,
) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "import probe_mod\n"
            "try:\n"
            "    getattr(probe_mod, 'sentinel_missing')\n"
            "except BaseException as exc:\n"
            "    print(type(exc).__name__)\n"
            "    print(str(exc))\n"
        ),
        "module_dunder_getattr_missing_attr",
        session_id=f"{NATIVE_BOOTSTRAP_SESSION_ID}-module-dunder-getattr",
        cache_dir=ROOT / ".molt_cache-module-dunder-getattr",
        backend="cranelift",
        extra_files={
            "probe_mod.py": (
                "def __getattr__(name):\n    raise AttributeError(f'HOOK::{name}')\n"
            )
        },
        extra_env={"MOLT_MODULE_ROOTS": str(tmp_path)},
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "AttributeError",
        "HOOK::sentinel_missing",
    ]


def test_native_module_attr_dunder_getattr_preserves_raised_attribute_error(
    tmp_path: Path,
) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "import probe_mod\n"
            "try:\n"
            "    probe_mod.sentinel_missing\n"
            "except BaseException as exc:\n"
            "    print(type(exc).__name__)\n"
            "    print(str(exc))\n"
        ),
        "module_attr_dunder_getattr_missing_attr",
        session_id=f"{NATIVE_BOOTSTRAP_SESSION_ID}-module-attr-dunder-getattr",
        cache_dir=ROOT / ".molt_cache-module-attr-dunder-getattr",
        backend="cranelift",
        extra_files={
            "probe_mod.py": (
                "def __getattr__(name):\n    raise AttributeError(f'HOOK::{name}')\n"
            )
        },
        extra_env={"MOLT_MODULE_ROOTS": str(tmp_path)},
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "AttributeError",
        "HOOK::sentinel_missing",
    ]


def test_native_getattr_default_suppresses_module_dunder_attribute_error(
    tmp_path: Path,
) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "import probe_mod\n"
            "print(getattr(probe_mod, 'sentinel_missing', 'fallback'))\n"
            "print('after')\n"
        ),
        "module_dunder_getattr_default",
        session_id=f"{NATIVE_BOOTSTRAP_SESSION_ID}-module-dunder-getattr-default",
        cache_dir=ROOT / ".molt_cache-module-dunder-getattr-default",
        backend="cranelift",
        extra_files={
            "probe_mod.py": (
                "def __getattr__(name):\n    raise AttributeError(f'HOOK::{name}')\n"
            )
        },
        extra_env={"MOLT_MODULE_ROOTS": str(tmp_path)},
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["fallback", "after"]


def test_native_local_function_raise_is_caught_by_try_except(tmp_path: Path) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "print('START')\n"
            "def boom():\n"
            "    raise AttributeError('HOOK::local')\n"
            "value = 'UNSET'\n"
            "try:\n"
            "    value = boom()\n"
            "    print('AFTER_CALL')\n"
            "except BaseException as exc:\n"
            "    print('EXC', type(exc).__name__, str(exc))\n"
            "print('END', type(value).__name__, repr(value))\n"
        ),
        "local_try_raise_caught",
        session_id=f"{NATIVE_BOOTSTRAP_SESSION_ID}-local-try-raise",
        cache_dir=ROOT / ".molt_cache-local-try-raise",
        backend="cranelift",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "START",
        "EXC AttributeError HOOK::local",
        "END str 'UNSET'",
    ]


def test_native_module_try_except_assignment_survives_post_try_load(
    tmp_path: Path,
) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "try:\n"
            "    raise ModuleNotFoundError('x')\n"
            "except ModuleNotFoundError:\n"
            "    flag = True\n"
            "else:\n"
            "    flag = False\n"
            "print(flag)\n"
        ),
        "module_try_except_assignment_post_load",
        session_id=f"{NATIVE_BOOTSTRAP_SESSION_ID}-module-try-except-assignment",
        cache_dir=ROOT / ".molt_cache-module-try-except-assignment",
        backend="cranelift",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["True"]


def test_native_plain_function_metadata_survives_try_scope(tmp_path: Path) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "try:\n"
            "    def f(a, **kwargs):\n"
            "        return a\n"
            "    print(type(f.__kwdefaults__).__name__)\n"
            "    print(repr(f.__kwdefaults__))\n"
            "    print(f(1))\n"
            "except BaseException as e:\n"
            "    print('CAUGHT', type(e).__name__, str(e))\n"
            "    raise\n"
        ),
        "plain_function_metadata_inside_try",
        session_id=f"{NATIVE_BOOTSTRAP_SESSION_ID}-plain-function-metadata-inside-try",
        cache_dir=ROOT / ".molt_cache-plain-function-metadata-inside-try",
        backend="cranelift",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "NoneType",
        "None",
        "1",
    ]


def test_native_plain_method_in_try_scope_is_callable(tmp_path: Path) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "try:\n"
            "    class C:\n"
            "        def f(self):\n"
            "            return 1\n"
            "    print(type(C.f).__name__)\n"
            "    print(callable(C.f))\n"
            "    print(C.f(C()))\n"
            "except BaseException as e:\n"
            "    print('CAUGHT', type(e).__name__, str(e))\n"
            "    raise\n"
        ),
        "plain_method_inside_try_callable",
        session_id=f"{NATIVE_BOOTSTRAP_SESSION_ID}-plain-method-inside-try",
        cache_dir=ROOT / ".molt_cache-plain-method-inside-try",
        backend="cranelift",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "function",
        "True",
        "1",
    ]


def test_native_direct_raise_is_caught_by_try_except(tmp_path: Path) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "print('START')\n"
            "try:\n"
            "    raise AttributeError('HOOK::direct')\n"
            "except BaseException as exc:\n"
            "    print('EXC', type(exc).__name__, str(exc))\n"
        ),
        "direct_try_raise_caught",
        session_id=f"{NATIVE_BOOTSTRAP_SESSION_ID}-direct-try-raise",
        cache_dir=ROOT / ".molt_cache-direct-try-raise",
        backend="cranelift",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "START",
        "EXC AttributeError HOOK::direct",
    ]


def test_native_try_multibase_class_statement_preserves_namespace(
    tmp_path: Path,
) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "try:\n"
            "    class EnumType(type):\n"
            "        def __new__(mcls, name, bases, namespace, **kwargs):\n"
            "            print('NEW_START', name, type(namespace).__name__)\n"
            "            members = []\n"
            "            for key, value in list(namespace.items()):\n"
            "                if key.startswith('_'):\n"
            "                    continue\n"
            "                if callable(value):\n"
            "                    continue\n"
            "                members.append((key, value))\n"
            "                namespace.pop(key, None)\n"
            "            cls = super().__new__(mcls, name, bases, dict(namespace))\n"
            "            for member_name, raw_value in members:\n"
            "                member = cls.__new__(cls, raw_value)\n"
            "                member._name_ = member_name\n"
            "                member._value_ = raw_value\n"
            "                setattr(cls, member_name, member)\n"
            "            return cls\n"
            "    class Enum(metaclass=EnumType):\n"
            "        def __new__(cls, value):\n"
            "            obj = object.__new__(cls)\n"
            "            obj._value_ = value\n"
            "            return obj\n"
            "    class IntEnum(int, Enum):\n"
            "        A = 1\n"
            "        B = 2\n"
            "    print('INTENUM_OK', IntEnum.A, IntEnum.B)\n"
            "except BaseException as e:\n"
            "    print('CAUGHT', type(e).__name__, str(e))\n"
            "    raise\n"
        ),
        "try_multibase_class_statement_namespace",
        session_id=f"{NATIVE_BOOTSTRAP_SESSION_ID}-multibase-class-namespace",
        cache_dir=ROOT / ".molt_cache-multibase-class-namespace",
        backend="cranelift",
        run_timeout_secs=20,
    )
    assert run.returncode == 0, run.stdout + run.stderr
    lines = run.stdout.strip().splitlines()
    assert lines[:2] == [
        "NEW_START Enum dict",
        "NEW_START IntEnum dict",
    ]
    assert len(lines) == 3, lines
    assert lines[2].startswith("INTENUM_OK "), lines


def test_native_try_metaclass_preserves_namespace_dict(tmp_path: Path) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "def f():\n"
            "    try:\n"
            "        class Meta(type):\n"
            "            def __new__(mcls, name, bases, namespace):\n"
            "                print('NS_TYPE', type(namespace).__name__)\n"
            "                print('NS_ITEMS', list(namespace.items()))\n"
            "                return super().__new__(mcls, name, bases, dict(namespace))\n"
            "        class Box(metaclass=Meta):\n"
            "            value = 1\n"
            "        print('BOX_OK', Box.value)\n"
            "    except BaseException as e:\n"
            "        print('CAUGHT', type(e).__name__, str(e))\n"
            "        raise\n"
            "f()\n"
        ),
        "try_metaclass_preserves_namespace",
        session_id=f"{NATIVE_BOOTSTRAP_SESSION_ID}-try-metaclass-namespace",
        cache_dir=ROOT / ".molt_cache-try-metaclass-namespace",
        backend="cranelift",
        run_timeout_secs=20,
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "NS_TYPE dict",
        "NS_ITEMS [('__module__', '__main__'), ('__qualname__', 'f.<locals>.Box'), ('__firstlineno__', 8), ('value', 1), ('__static_attributes__', ())]",
        "BOX_OK 1",
    ]


def test_native_module_metaclass_zero_arg_super_new(tmp_path: Path) -> None:
    # Module-level metaclass: zero-arg ``super().__new__`` must resolve the
    # ``__class__`` cell (the metaclass) and build the class.  Sibling of the
    # function-local case to ensure the structural ``__class__``-cell fix keeps
    # the module-level path correct (no asymmetry / regression).
    run = _build_and_run(
        tmp_path,
        "class Meta(type):\n"
        "    def __new__(mcls, name, bases, namespace):\n"
        "        return super().__new__(mcls, name, bases, dict(namespace))\n"
        "class Box(metaclass=Meta):\n"
        "    value = 1\n"
        "print('BOX_OK', Box.value, type(Box).__name__)\n",
        "module_metaclass_zero_arg_super",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["BOX_OK 1 Meta"]


def test_native_nested_class_zero_arg_super_method(tmp_path: Path) -> None:
    # Plain (non-metaclass) function-local nested classes with zero-arg
    # ``super()`` in an instance method.  Guards against over-fitting the
    # ``__class__``-cell fix to metaclasses: any function-local class that uses
    # zero-arg ``super()`` must read the class from its ``__class__`` closure
    # cell rather than re-deriving it by module-attribute name.
    run = _build_and_run(
        tmp_path,
        "def make():\n"
        "    class Base:\n"
        "        def label(self):\n"
        "            return 'B'\n"
        "    class Child(Base):\n"
        "        def label(self):\n"
        "            return 'C+' + super().label()\n"
        "        def who(self):\n"
        "            return __class__.__name__\n"
        "    c = Child()\n"
        "    return c.label(), c.who()\n"
        "label, who = make()\n"
        "print('NESTED', label, who)\n",
        "nested_class_zero_arg_super",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["NESTED C+B Child"]


def test_native_local_metaclass_super_with_captured_local(tmp_path: Path) -> None:
    # Function-local metaclass whose ``__new__`` BOTH closes over an enclosing
    # local (``prefix``) AND uses zero-arg ``super()``.  Proves the implicit
    # ``__class__`` cell coexists with a real captured free variable in the same
    # closure (consistent closure indices), not just standalone.
    run = _build_and_run(
        tmp_path,
        "def f(prefix):\n"
        "    class Meta(type):\n"
        "        def __new__(mcls, name, bases, ns):\n"
        "            ns = dict(ns)\n"
        "            ns['tag'] = prefix + name\n"
        "            return super().__new__(mcls, name, bases, ns)\n"
        "    class Box(metaclass=Meta):\n"
        "        pass\n"
        "    return Box.tag, type(Box).__name__\n"
        "tag, meta_name = f('PRE_')\n"
        "print('CAP', tag, meta_name)\n",
        "local_metaclass_super_captured_local",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["CAP PRE_Box Meta"]


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


def test_native_from_import_package_export_wins_over_same_named_child_module(
    tmp_path: Path,
) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        ("from pkg import value\nprint(value)\n"),
        "package_export_wins_over_child",
        session_id="pytest-native-bootstrap-from-export-vs-child",
        cache_dir=ROOT / ".molt_cache-package-from-export-vs-child",
        backend="cranelift",
        source_relpath="main.py",
        extra_files={
            "pkg/__init__.py": "value = 'public-export'\n__all__ = ['value']\n",
            "pkg/value.py": "value = 'child-module'\n",
        },
        extra_env={"MOLT_MODULE_ROOTS": str(tmp_path)},
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "public-export"


def test_native_from_import_auto_imports_child_module_and_binds_parent_attr(
    tmp_path: Path,
) -> None:
    run = _build_and_run_package_bootstrap(
        tmp_path,
        (
            "import sys\n"
            "from pkg import helper\n"
            "print(helper.__name__)\n"
            "print(helper is sys.modules['pkg.helper'])\n"
            "print(getattr(sys.modules['pkg'], 'helper') is helper)\n"
        ),
        "package_from_import_child_binding",
        cache_suffix="from-child-binding",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "pkg.helper",
        "True",
        "True",
    ]


def test_native_from_import_missing_child_reports_import_from_error(
    tmp_path: Path,
) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "try:\n"
            "    from pkg import missing_child\n"
            "except BaseException as exc:\n"
            "    print(type(exc).__name__)\n"
            "    print(str(exc))\n"
        ),
        "package_from_import_missing_child",
        session_id="pytest-native-bootstrap-from-missing-child",
        cache_dir=ROOT / ".molt_cache-package-from-missing-child",
        backend="cranelift",
        source_relpath="main.py",
        extra_files={"pkg/__init__.py": "VALUE = 1\n"},
        extra_env={"MOLT_MODULE_ROOTS": str(tmp_path)},
    )
    assert run.returncode == 0, run.stdout + run.stderr
    lines = run.stdout.strip().splitlines()
    assert lines[0] == "ImportError"
    assert "cannot import name 'missing_child' from 'pkg'" in lines[1]


def test_native_from_import_child_dependency_error_propagates(
    tmp_path: Path,
) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "try:\n"
            "    from pkg import child\n"
            "except BaseException as exc:\n"
            "    print(type(exc).__name__)\n"
            "    print(str(exc))\n"
        ),
        "package_from_import_child_dependency_error",
        session_id="pytest-native-bootstrap-from-child-dependency-error",
        cache_dir=ROOT / ".molt_cache-package-from-child-dependency-error",
        backend="cranelift",
        source_relpath="main.py",
        extra_files={
            "pkg/__init__.py": "VALUE = 1\n",
            "pkg/child.py": "import definitely_missing_dependency\n",
        },
        extra_env={"MOLT_MODULE_ROOTS": str(tmp_path)},
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "ModuleNotFoundError",
        "No module named 'definitely_missing_dependency'",
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


def test_native_import_transaction_unifies_public_import_surfaces(
    tmp_path: Path,
) -> None:
    run = _build_and_run_package_bootstrap(
        tmp_path,
        (
            "import builtins\n"
            "import importlib\n"
            "import sys\n"
            "import pkg.helper as helper\n"
            "leaf_from_importlib = importlib.import_module('pkg.helper')\n"
            "leaf_from_relative_importlib = importlib.import_module('.helper', 'pkg')\n"
            "resolved_relative = importlib.util.resolve_name('.helper', 'pkg')\n"
            "top_from_dunder = builtins.__import__('pkg.helper', globals(), locals(), (), 0)\n"
            "leaf_from_dunder = builtins.__import__('pkg.helper', globals(), locals(), ('ping',), 0)\n"
            "relative_leaf = builtins.__import__('helper', globals(), locals(), ('ping',), 1)\n"
            "relative_empty = builtins.__import__('', globals(), locals(), (), 1)\n"
            "print(resolved_relative)\n"
            "print(leaf_from_importlib is helper)\n"
            "print(leaf_from_relative_importlib is helper)\n"
            "print(top_from_dunder is sys.modules['pkg'])\n"
            "print(leaf_from_dunder is helper)\n"
            "print(relative_leaf is helper)\n"
            "print(relative_empty is sys.modules['pkg'])\n"
            "dotted_direct_rejected = False\n"
            "try:\n"
            "    builtins.__import__('.helper', globals(), locals(), ('ping',), 1)\n"
            "except ModuleNotFoundError:\n"
            "    dotted_direct_rejected = True\n"
            "else:\n"
            "    dotted_direct_rejected = False\n"
            "print(dotted_direct_rejected)\n"
            "print(getattr(sys.modules['pkg'], 'helper') is helper)\n"
        ),
        "package_entry_import_transaction_identity",
        cache_suffix="import-transaction",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["pkg.helper"] + ["True"] * 8


def test_native_importlib_import_module_literal_respects_rebinding(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "import importlib\n"
            "def fake(name):\n"
            "    return 'fake:' + name\n"
            "importlib.import_module = fake\n"
            "print(importlib.import_module('json'))\n"
        ),
        "importlib_import_module_literal_rebinding",
    )

    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "fake:json"


def test_native_importlib_import_module_literal_uses_transaction(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "import importlib\n"
            "import sys\n"
            "mod = importlib.import_module('json')\n"
            "print(mod.__name__)\n"
            "print(mod is sys.modules['json'])\n"
        ),
        "importlib_import_module_literal_transaction",
    )

    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["json", "True"]


def test_native_importlib_dynamic_source_failure_does_not_commit_partial_module(
    tmp_path: Path,
) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "import importlib\n"
            "import sys\n"
            f"sys.path.insert(0, {str(tmp_path / 'runtime_site')!r})\n"
            "module_name = sys.__name__.replace('sys', 'dynamic_unsupported')\n"
            "try:\n"
            "    importlib.import_module(module_name)\n"
            "except BaseException as exc:\n"
            "    print(type(exc).__name__)\n"
            "    print('unsupported module statement' in str(exc))\n"
            "else:\n"
            "    print('NO_ERROR')\n"
            "print(sys.modules.get(module_name) is None)\n"
        ),
        "importlib_dynamic_source_fail_closed",
        session_id=f"{NATIVE_BOOTSTRAP_SESSION_ID}-importlib-dynamic-source-fail",
        cache_dir=ROOT / ".molt_cache-importlib-dynamic-source-fail",
        backend="cranelift",
        extra_files={
            "runtime_site/dynamic_unsupported.py": (
                "from typing import Any\n\n"
                "def unsupported_runtime_function():\n"
                "    return Any\n"
            )
        },
        extra_build_args=[
            "--capabilities",
            "fs.read",
            "--stdlib-profile",
            "full",
            "--rebuild",
            "--no-cache",
        ],
        run_timeout_secs=20,
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "NotImplementedError",
        "True",
        "True",
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


def test_native_module_hasattr_missing_uses_seeded_module_name_metadata(
    tmp_path: Path,
) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "import sys\n"
            "import builtins\n"
            "import types\n"
            "synthetic = types.ModuleType('synthetic_mod')\n"
            "print(sys.__name__)\n"
            "print(hasattr(sys, 'tolist'))\n"
            "print(synthetic.__name__)\n"
            "print(hasattr(synthetic, 'tolist'))\n"
        ),
        "module_hasattr_seeded_name_metadata",
        session_id=f"{NATIVE_BOOTSTRAP_SESSION_ID}-module-hasattr",
        cache_dir=ROOT / ".molt_cache-module-hasattr",
        backend="cranelift",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "sys",
        "False",
        "synthetic_mod",
        "False",
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


def test_native_import_os_is_clean(tmp_path: Path) -> None:
    run = _build_and_run(tmp_path, "import os\nprint('ok')\n", "import_os")
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "ok"


def test_native_hasattr_missing_attr_does_not_poison_following_import(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "import os\n"
            "print(hasattr(os, 'sched_getaffinity'))\n"
            "import enum\n"
            "print(enum.IntEnum.__name__)\n"
        ),
        "hasattr_missing_attr_then_enum",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["False", "IntEnum"]


def test_native_slots_subclass_inherits_base_instance_dict(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "class Base:\n"
            "    pass\n"
            "class Child(Base):\n"
            "    __slots__ = ('slot_value',)\n"
            "c = Child()\n"
            "c.slot_value = 3\n"
            "c.dynamic_value = 4\n"
            "print(c.slot_value)\n"
            "print(c.dynamic_value)\n"
            "print(c.__dict__['dynamic_value'])\n"
            "class SlotOnly:\n"
            "    __slots__ = ('slot_value',)\n"
            "s = SlotOnly()\n"
            "s.slot_value = 5\n"
            "print(s.slot_value)\n"
            "try:\n"
            "    s.dynamic_value = 6\n"
            "except AttributeError:\n"
            "    print('blocked')\n"
        ),
        "slots_subclass_inherits_base_instance_dict",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["3", "4", "4", "5", "blocked"]


def test_native_handled_missing_ctypes_attr_does_not_poison_dataclass_enum_imports(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "import ctypes\n"
            "try:\n"
            "    getattr(ctypes, '__molt_missing_ctypes_attr__')\n"
            "except AttributeError:\n"
            "    pass\n"
            "from dataclasses import dataclass\n"
            "@dataclass(frozen=True)\n"
            "class Box:\n"
            "    name: str\n"
            "    value: int = 1\n"
            "print(Box('a').name)\n"
            "import enum\n"
            "print(enum.IntEnum.__name__)\n"
        ),
        "handled_missing_ctypes_attr_dataclass_enum",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["a", "IntEnum"]


def test_native_dataclass_field_objects_survive_class_storage_and_fields_tuple(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "from dataclasses import dataclass, field, fields\n"
            "import ctypes\n"
            "try:\n"
            "    getattr(ctypes, '__molt_missing_ctypes_attr__')\n"
            "except AttributeError:\n"
            "    pass\n"
            "@dataclass(frozen=True, eq=False)\n"
            "class DType:\n"
            "    priority: int\n"
            "    bitsize: int\n"
            "    name: str = field(default='void', metadata={'kind': 'scalar'})\n"
            "    count: int = 1\n"
            "@dataclass(frozen=True, eq=False)\n"
            "class PtrDType(DType):\n"
            "    base: DType = field(default=DType(-1, 0))\n"
            "    size: int = -1\n"
            "dt = DType(0, 1, 'bool')\n"
            "pt = PtrDType(2, 64, 'ptr', 1, dt, 4)\n"
            "dtype_fields = fields(DType)\n"
            "ptr_fields = fields(PtrDType)\n"
            "print(dtype_fields[2].name)\n"
            "print(ptr_fields[-2].name)\n"
            "print(pt.base.name)\n"
            "import enum\n"
            "print(enum.IntEnum.__name__)\n"
        ),
        "dataclass_field_objects_survive_class_storage",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["name", "base", "bool", "IntEnum"]


def test_native_loop_phi_local_boundary_set_survives_slotted_dataclass(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "def inherited_slots_probe():\n"
            "    inherited = set()\n"
            "    for base in ():\n"
            "        try:\n"
            "            for entry in base:\n"
            "                inherited.add(entry)\n"
            "        except TypeError:\n"
            "            continue\n"
            "    return inherited\n"
            "slots = inherited_slots_probe()\n"
            "print(type(slots).__name__)\n"
            "print('value' in slots)\n"
            "from dataclasses import dataclass\n"
            "@dataclass(eq=False, slots=True)\n"
            "class Node:\n"
            "    value: int\n"
            "print(Node(7).value)\n"
        ),
        "loop_phi_local_boundary_set_survives_slotted_dataclass",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["set", "False", "7"]


def test_native_slotted_dataclass_field_overrides_inherited_readonly_property(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "from dataclasses import dataclass\n"
            "class DTypeMixin:\n"
            "    @property\n"
            "    def dtype(self):\n"
            "        return 'base-property'\n"
            "@dataclass(eq=False, slots=True)\n"
            "class UOp(DTypeMixin):\n"
            "    dtype: str = 'void'\n"
            "    arg: int = 0\n"
            "u = UOp('float32', 7)\n"
            "print(u.dtype)\n"
            "print(u.arg)\n"
            "u.dtype = 'int32'\n"
            "print(u.dtype)\n"
            "try:\n"
            "    del u.dtype\n"
            "    print(u.dtype)\n"
            "except AttributeError:\n"
            "    print('missing')\n"
        ),
        "slotted_dataclass_field_overrides_inherited_readonly_property",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["float32", "7", "int32", "missing"]


def test_native_slotted_dataclass_inherits_base_dict_without_field_mirroring(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "from dataclasses import dataclass\n"
            "class recursive_property(property):\n"
            "    def __init__(self, fn):\n"
            "        self.nm = '_cache_' + fn.__name__\n"
            "        self.fxn = fn\n"
            "        super().__init__(fn)\n"
            "    def __get__(self, obj, cls=None):\n"
            "        if obj is None:\n"
            "            return self\n"
            "        if self.nm not in obj.__dict__:\n"
            "            obj.__dict__[self.nm] = self.fxn(obj)\n"
            "        return obj.__dict__[self.nm]\n"
            "class Mixin:\n"
            "    pass\n"
            "@dataclass(eq=False, slots=True)\n"
            "class Node(Mixin):\n"
            "    value: int\n"
            "    @recursive_property\n"
            "    def doubled(self):\n"
            "        return self.value * 2\n"
            "n = Node(6)\n"
            "print(type(n.__dict__).__name__)\n"
            "print('value' in n.__dict__)\n"
            "print(n.doubled)\n"
            "cache_name = Node.doubled.nm\n"
            "print(cache_name in n.__dict__)\n"
            "print(n.__dict__[cache_name])\n"
            "n.extra = 9\n"
            "print(n.extra)\n"
            "print(n.value)\n"
        ),
        "slotted_dataclass_inherits_base_dict_without_field_mirroring",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "dict",
        "False",
        "12",
        "True",
        "12",
        "9",
        "6",
    ]


def test_native_functools_cached_property_descriptor_survives_class_lifetime(
    tmp_path: Path,
) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "import functools\n"
            "class _Device:\n"
            "    @property\n"
            "    def DEFAULT(self):\n"
            "        return self._select_device\n"
            "    @functools.cached_property\n"
            "    def _select_device(self):\n"
            "        return 'PYTHON'\n"
            "Device = _Device()\n"
            "print(Device.DEFAULT)\n"
            "print(Device.__dict__['_select_device'])\n"
            "print(Device.DEFAULT)\n"
            "print(_Device._select_device.attrname)\n"
        ),
        "functools_cached_property_descriptor_survives_class_lifetime",
        session_id=f"{NATIVE_BOOTSTRAP_SESSION_ID}-cached-property-class-lifetime",
        cache_dir=ROOT / ".molt_cache-cached-property-class-lifetime",
        backend="cranelift",
        extra_build_args=["--rebuild", "--no-cache"],
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "PYTHON",
        "PYTHON",
        "PYTHON",
        "_select_device",
    ]


def test_native_slots_only_dataclass_rejects_instance_dict_and_extra_attrs(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "from dataclasses import dataclass\n"
            "@dataclass(eq=False, slots=True)\n"
            "class SlotOnly:\n"
            "    value: int\n"
            "s = SlotOnly(3)\n"
            "try:\n"
            "    print(s.__dict__)\n"
            "except AttributeError:\n"
            "    print('no-dict')\n"
            "try:\n"
            "    s.extra = 4\n"
            "    print('set')\n"
            "except AttributeError:\n"
            "    print('no-extra')\n"
            "print(s.value)\n"
        ),
        "slots_only_dataclass_rejects_instance_dict_and_extra_attrs",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["no-dict", "no-extra", "3"]


def test_native_property_getter_runtime_error_propagates_without_attr_fallback(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "class C:\n"
            "    @property\n"
            "    def value(self):\n"
            "        raise RuntimeError('boom')\n"
            "try:\n"
            "    print(C().value)\n"
            "except Exception as exc:\n"
            "    print(type(exc).__name__, str(exc))\n"
        ),
        "property_getter_runtime_error_propagates",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["RuntimeError boom"]


def test_native_ctypes_tinygrad_scalar_surface_uses_intrinsic_coercion(
    tmp_path: Path,
) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "import ctypes\n"
            "print(hasattr(ctypes.c_float, '_fields_'))\n"
            "print(ctypes.c_char(65).value)\n"
            "print(ctypes.c_uint8(-1).value)\n"
            "print(ctypes.c_int8(255).value)\n"
            "print(ctypes.c_uint64(-1).value)\n"
            "print(ctypes.c_float(0.1).value != 0.1)\n"
            "print(ctypes.sizeof(ctypes.c_uint64))\n"
            "arr = (ctypes.c_uint8 * 3)(1, -1, 256)\n"
            "print(list(arr))\n"
        ),
        "ctypes_tinygrad_scalar_surface",
        session_id=f"{NATIVE_BOOTSTRAP_SESSION_ID}-ctypes-tinygrad-scalar",
        cache_dir=ROOT / ".molt_cache-ctypes-tinygrad-scalar",
        backend="cranelift",
        extra_build_args=[
            "--capabilities",
            "ffi.unsafe",
            "--stdlib-profile",
            "full",
            "--rebuild",
            "--no-cache",
        ],
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "False",
        "b'A'",
        "255",
        "-1",
        "18446744073709551615",
        "True",
        "8",
        "[1, 255, 0]",
    ]


def test_native_ctypes_scalar_numeric_protocol_matches_cpython_shape(
    tmp_path: Path,
) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "import ctypes\n"
            "class ConstFloat:\n"
            "    def __init__(self, value):\n"
            "        self.value_payload = value\n"
            "    def __float__(self):\n"
            "        return self.value_payload\n"
            "    def __int__(self):\n"
            "        return int(self.value_payload)\n"
            "class IndexLike:\n"
            "    def __index__(self):\n"
            "        return 260\n"
            "class IntOnly:\n"
            "    def __int__(self):\n"
            "        return 7\n"
            "class Box:\n"
            "    value = 3\n"
            "class FloatSubclass(float):\n"
            "    __slots__ = ('bits',)\n"
            "    def __new__(cls, value):\n"
            "        obj = super().__new__(cls, value)\n"
            "        obj.bits = 99\n"
            "        return obj\n"
            "class IntSubclass(int):\n"
            "    __slots__ = ('bits',)\n"
            "    def __new__(cls, value):\n"
            "        obj = super().__new__(cls, value)\n"
            "        obj.bits = 77\n"
            "        return obj\n"
            "sub = FloatSubclass(2.25)\n"
            "isub = IntSubclass(12)\n"
            "print(ctypes.c_float(ConstFloat(1.5)).value)\n"
            "print(ctypes.c_int8(IndexLike()).value)\n"
            "print(type(sub).__name__)\n"
            "print(float(sub))\n"
            "print(float.__float__(sub))\n"
            "print(ctypes.c_float(sub).value)\n"
            "print(type(isub).__name__)\n"
            "print(int(isub))\n"
            "print(isub.bits)\n"
            "print(ctypes.c_int(isub).value)\n"
            "for value in (IntOnly(), Box(), 7.0):\n"
            "    try:\n"
            "        ctypes.c_int(value)\n"
            "    except Exception as exc:\n"
            "        print(type(exc).__name__)\n"
            "    else:\n"
            "        print('no-error')\n"
        ),
        "ctypes_scalar_numeric_protocol",
        session_id=f"{NATIVE_BOOTSTRAP_SESSION_ID}-ctypes-scalar-numeric-protocol",
        cache_dir=ROOT / ".molt_cache-ctypes-scalar-numeric-protocol",
        backend="cranelift",
        extra_build_args=[
            "--capabilities",
            "ffi.unsafe",
            "--stdlib-profile",
            "full",
            "--rebuild",
            "--no-cache",
        ],
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "1.5",
        "4",
        "FloatSubclass",
        "2.25",
        "2.25",
        "2.25",
        "IntSubclass",
        "12",
        "77",
        "12",
        "TypeError",
        "TypeError",
        "TypeError",
    ]


def test_native_dict_values_loop_cleanup_preserves_dict_owned_values(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "class Box:\n"
            "    pass\n"
            "def make():\n"
            "    d = {}\n"
            "    d['x'] = Box()\n"
            "    d['y'] = Box()\n"
            "    for _ in range(8):\n"
            "        for value in d.values():\n"
            "            hold = value\n"
            "        hold = None\n"
            "    for value in d.values():\n"
            "        print(value.__class__.__name__)\n"
            "make()\n"
        ),
        "dict_values_loop_cleanup_preserves_dict_owned_values",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["Box", "Box"]


def test_native_builtin_container_class_attribute_matches_type(tmp_path: Path) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "values = [(), [], {}, set(), 'x', b'y', 1, True, 2.5, None]\n"
            "for value in values:\n"
            "    print(\n"
            "        value.__class__.__name__,\n"
            "        value.__class__ is type(value),\n"
            "        getattr(value, '__class__') is type(value),\n"
            "        hasattr(value, '__class__'),\n"
            "    )\n"
        ),
        "builtin_container_class_attribute_matches_type",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "tuple True True True",
        "list True True True",
        "dict True True True",
        "set True True True",
        "str True True True",
        "bytes True True True",
        "int True True True",
        "bool True True True",
        "float True True True",
        "NoneType True True True",
    ]


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


def test_native_functools_cache_binds_binary_special_method_descriptor(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "import functools\n"
            "class Acc:\n"
            "    @functools.cache\n"
            "    def __add__(self, other):\n"
            "        print(type(self).__name__, type(other).__name__)\n"
            "        return 7\n"
            "a = Acc()\n"
            "b = Acc()\n"
            "print(a + b)\n"
            "print(a + b)\n"
        ),
        "functools_cache_binary_special_descriptor",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["Acc Acc", "7", "7"]


def test_native_tuple_ordering_reports_first_unorderable_element(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "try:\n"
            "    print((object(),) < (object(),))\n"
            "except TypeError as exc:\n"
            "    print(str(exc))\n"
            "try:\n"
            "    print([(object(),), (object(),)].sort())\n"
            "except TypeError as exc:\n"
            "    print(str(exc))\n"
        ),
        "tuple_ordering_first_unorderable_element",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "'<' not supported between instances of 'object' and 'object'",
        "'<' not supported between instances of 'object' and 'object'",
    ]


def test_native_sequence_comparison_uses_rich_equality_before_ordering(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "from dataclasses import dataclass\n"
            "\n"
            "@dataclass(frozen=True)\n"
            "class Box:\n"
            "    x: int\n"
            "    y: tuple\n"
            "\n"
            "class Opaque:\n"
            "    pass\n"
            "\n"
            "print((Box(2, (3,)),) == (Box(2, (3,)),))\n"
            "print((1, Box(2, (3,)), 'a') < (1, Box(2, (3,)), 'b'))\n"
            "try:\n"
            "    print((1, Box(2, (Opaque(),)), 'a') < (1, Box(2, (Opaque(),)), 'b'))\n"
            "except TypeError as exc:\n"
            "    print(str(exc))\n"
        ),
        "sequence_rich_equality_before_ordering",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "True",
        "True",
        "'<' not supported between instances of 'Box' and 'Box'",
    ]


def test_native_property_getter_exception_matches_superclass_handler(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "class MovementMixin:\n"
            "    @property\n"
            "    def shape(self):\n"
            "        raise NotImplementedError\n"
            "\n"
            "class UPat(MovementMixin):\n"
            "    pass\n"
            "\n"
            "try:\n"
            "    UPat().shape\n"
            "except (RuntimeError, ValueError) as exc:\n"
            "    print('caught', type(exc).__name__)\n"
            "\n"
            "try:\n"
            "    UPat().shape\n"
            "except Exception as exc:\n"
            "    print('caught2', type(exc).__name__)\n"
        ),
        "property_getter_exception_superclass_handler",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "caught NotImplementedError",
        "caught2 NotImplementedError",
    ]


def test_native_unary_invert_dispatches_dunder_for_user_objects(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "class Bits:\n"
            "    def __init__(self, tag):\n"
            "        self.tag = tag\n"
            "    def __xor__(self, other):\n"
            "        return Bits(('xor', self.tag, other))\n"
            "    def bitwise_not(self):\n"
            "        return self ^ -1\n"
            "    def __invert__(self):\n"
            "        return self.bitwise_not()\n"
            "\n"
            "print((~Bits('x')).tag)\n"
        ),
        "unary_invert_dunder_user_object",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "('xor', 'x', -1)"


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
        ("def f():\n    x = 1\n    x()\nf()\n"),
        "indirect_noncallable_typeerror",
    )
    assert run.returncode != 0
    assert "TypeError" in run.stderr
    assert "not callable" in run.stderr


def test_native_import_typing_optional_is_clean(tmp_path: Path) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "import typing\n"
            "from typing import Generator, Optional, Sequence, Type\n"
            "print(isinstance([], Sequence))\n"
            "print(typing.get_origin(Sequence[int]).__name__)\n"
            "print(typing.get_args(Generator[int, None, None])[0].__name__)\n"
            "print(typing.get_origin(Type[int]).__name__)\n"
            "print(Optional)\n"
            "print('ok')\n"
        ),
        "import_typing_optional",
        session_id="pytest-native-bootstrap-typing",
        cache_dir=ROOT / ".molt_cache-typing",
        backend="cranelift",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "True",
        "Sequence",
        "int",
        "type",
        "typing.Optional",
        "ok",
    ]


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


def test_native_import_builtins_descriptor_types_are_bootstrapped(
    tmp_path: Path,
) -> None:
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


def test_native_repo_package_imports_include_molt_parent_package(
    tmp_path: Path,
) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        ("from molt.gpu.tensor import Tensor\nprint('ok')\n"),
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


def test_native_abc_register_builtin_iterator_type_on_derived_abc(
    tmp_path: Path,
) -> None:
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


def test_native_abc_register_builtin_iterator_family_on_derived_abc(
    tmp_path: Path,
) -> None:
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


def test_native_constant_if_false_branch_is_eliminated(tmp_path: Path) -> None:
    # CPython's compiler drops a constant-false branch entirely: names assigned
    # only there stay unbound and references inside it never reach bytecode. Molt
    # must match this (the `if False:  # TYPE_CHECKING` block in builtins.py whose
    # annotation keys collide with intrinsic names — e.g. `molt_thread_spawn` —
    # must NOT leak into the per-app intrinsic manifest). A name assigned only in
    # the dead branch must raise NameError when read.
    run = _build_and_run(
        tmp_path,
        (
            "def main() -> None:\n"
            "    if False:\n"
            "        only_dead = 1\n"
            "    try:\n"
            "        print(only_dead)\n"
            "    except NameError:\n"
            "        print('unbound-ok')\n"
            "\n"
            "main()\n"
        ),
        "constant_if_false_eliminated",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "unbound-ok"


def test_native_constant_if_branches_match_cpython(tmp_path: Path) -> None:
    program = (
        "def main() -> None:\n"
        "    out = []\n"
        "    if True:\n"
        "        out.append('T')\n"
        "    else:\n"
        "        out.append('T_dead')\n"
        "    if False:\n"
        "        out.append('F_dead')\n"
        "    else:\n"
        "        out.append('F')\n"
        "    if 0:\n"
        "        out.append('0_dead')\n"
        "    if 1:\n"
        "        out.append('1')\n"
        "    if '':\n"
        "        out.append('emptystr_dead')\n"
        "    if 'x':\n"
        "        out.append('str')\n"
        "    if None:\n"
        "        out.append('none_dead')\n"
        "    print(','.join(out))\n"
        "\n"
        "main()\n"
    )
    run = _build_and_run(tmp_path, program, "constant_if_branches")
    assert run.returncode == 0, run.stdout + run.stderr
    # Mirror CPython's own evaluation of the identical program.
    namespace: dict[str, object] = {}
    exec(compile(program, "<cpython-ref>", "exec"), namespace)  # noqa: S102
    expected_out: list[str] = []
    for token in ("T", "F", "1", "str"):
        expected_out.append(token)
    assert run.stdout.strip() == ",".join(expected_out)


def test_native_deferred_builtin_surface_resolves_on_first_use(
    tmp_path: Path,
) -> None:
    # Bootstrap Authority: the cold/deferred builtin + sys surface must still
    # resolve ON FIRST USE after the constant-fold change strips the dead
    # `if False:` annotation block. Exercises pow(), compile() (its intrinsic
    # must resolve — it raises on the micro profile because parser-backed
    # validation needs the `stdlib_ast` feature, which still proves the
    # `molt_compile_builtin` intrinsic is reachable, not stripped), sys._getframe(),
    # and exception-class construction in one native binary.
    program = (
        "import sys\n"
        "\n"
        "def main() -> None:\n"
        "    print(pow(2, 10))\n"
        "    print(pow(2, 10, 100))\n"
        "    try:\n"
        "        compile('x=', 'f', 'exec')\n"
        "        print('compile-no-raise')\n"
        "    except (SyntaxError, NotImplementedError):\n"
        "        print('compile-resolved')\n"
        # sys._getframe must resolve and be callable on first use; native
        # release builds legitimately return None (frames are not materialized
        # unless a debug/trace build requests them), so accept either — the
        # regression guards resolution, not frame materialization.
        "    sys._getframe(0)\n"
        "    print('getframe-resolved')\n"
        "    try:\n"
        "        raise ValueError('boom')\n"
        "    except ValueError as exc:\n"
        "        print(type(exc).__name__, str(exc))\n"
        "    print(isinstance(KeyError(), LookupError))\n"
        "\n"
        "main()\n"
    )
    run = _build_and_run(tmp_path, program, "deferred_builtin_surface")
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "1024",
        "24",
        "compile-resolved",
        "getframe-resolved",
        "ValueError boom",
        "True",
    ]


def test_native_functools_cache_keyword_key_keeps_runtime_marker_root(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "from functools import cache\n"
            "\n"
            "calls = 0\n"
            "\n"
            "@cache\n"
            "def pattern(name=None):\n"
            "    global calls\n"
            "    calls += 1\n"
            "    return (name, calls)\n"
            "\n"
            "def main() -> None:\n"
            "    print(pattern(name='x'))\n"
            "    for _ in range(64):\n"
            "        pattern(name='x')\n"
            "    print(calls)\n"
            "    print(pattern(name='y'))\n"
            "    info = pattern.cache_info()\n"
            "    print(info.hits, info.misses)\n"
            "\n"
            "main()\n"
        ),
        "functools_cache_keyword_marker_root",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "('x', 1)",
        "1",
        "('y', 2)",
        "64 2",
    ]


def test_native_help_resolves_via_builtins_attr(tmp_path: Path) -> None:
    # `help`/`quit`/`exit` are site-like builtins backed by `_sitebuiltins`.
    # Verify the bound objects are still reachable and callable-shaped after the
    # constant-fold change (repr of the Quitter carries its CPython-parity text).
    run = _build_and_run(
        tmp_path,
        (
            "import builtins\n"
            "\n"
            "def main() -> None:\n"
            "    print(callable(help))\n"
            "    print(callable(builtins.help))\n"
            "    print('exit' in repr(exit))\n"
            "\n"
            "main()\n"
        ),
        "help_via_builtins_attr",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["True", "True", "True"]


def test_native_function_code_names_match_global_import_and_attr_order(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "HELPER_CONST = 4\n"
            "\n"
            "def helper(value):\n"
            "    return value + 1\n"
            "\n"
            "def target(value):\n"
            "    local = helper(value)\n"
            "    import os.path as p\n"
            "    return HELPER_CONST + local.real\n"
            "\n"
            "def main() -> None:\n"
            "    print(target.__code__.co_names)\n"
            "    print(isinstance(target.__code__.co_names, tuple))\n"
            "\n"
            "main()\n"
        ),
        "function_code_names_order",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "('helper', 'os.path', 'path', 'HELPER_CONST', 'real')",
        "True",
    ]


def test_native_types_functiontype_reconstructs_executable_function_from_code(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "import builtins\n"
            "import types\n"
            "\n"
            "SCALE = 7\n"
            "\n"
            "def base(value=3):\n"
            "    return SCALE + value\n"
            "\n"
            "clone_globals = {\n"
            "    'SCALE': 11,\n"
            "    '__builtins__': builtins,\n"
            "    '__name__': 'clone_mod',\n"
            "}\n"
            "clone = types.FunctionType(base.__code__, clone_globals, 'clone', (5,))\n"
            "print(clone.__name__)\n"
            "print(clone.__module__)\n"
            "print(clone.__defaults__)\n"
            "print(clone())\n"
            "print(clone(2))\n"
            "print(clone(value=2))\n"
            "print(clone(**{'value': 4}))\n"
            "print(clone.__globals__['SCALE'])\n"
        ),
        "types_functiontype_reconstructs_code",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "clone",
        "clone_mod",
        "(5,)",
        "16",
        "13",
        "13",
        "15",
        "11",
    ]


def test_native_itertools_repeat_is_runtime_type(tmp_path: Path) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "import itertools\n"
            "r = itertools.repeat('x', 2)\n"
            "print(itertools.repeat.__name__)\n"
            "print(type(r) is itertools.repeat)\n"
            "print(isinstance(r, itertools.repeat))\n"
            "print(next(r))\n"
            "print(next(r))\n"
            "try:\n"
            "    next(r)\n"
            "except StopIteration:\n"
            "    print('stop')\n"
        ),
        "itertools_repeat_runtime_type",
        session_id=f"{NATIVE_BOOTSTRAP_SESSION_ID}-itertools-repeat-type",
        cache_dir=ROOT / ".molt_cache-itertools-repeat-type",
        backend="cranelift",
        extra_build_args=["--stdlib-profile", "full", "--rebuild", "--no-cache"],
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "repeat",
        "True",
        "True",
        "x",
        "x",
        "stop",
    ]


def test_native_inspect_getmembers_lists_and_filters_functions(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "import inspect\n"
            "\n"
            "class C:\n"
            "    value = 2\n"
            "    def method(self):\n"
            "        return 1\n"
            "\n"
            "members = dict(inspect.getmembers(C))\n"
            "funcs = dict(inspect.getmembers(C, inspect.isfunction))\n"
            "print('method' in members, '__name__' in members)\n"
            "print('method' in funcs, 'value' in funcs)\n"
        ),
        "inspect_getmembers_lists_and_filters",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "True True",
        "True False",
    ]


def test_native_higher_order_builtin_minmax_binds_vararg_shape(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "def apply(fn, value):\n"
            "    return fn(value)\n"
            "\n"
            "def apply_args(fn, a, b, c):\n"
            "    return fn(a, b, c)\n"
            "\n"
            "print(apply(max, [1, 5, 3]))\n"
            "print(apply(min, [1, 5, 3]))\n"
            "print(apply_args(max, 1, 5, 3))\n"
            "print(apply_args(min, 1, 5, 3))\n"
        ),
        "higher_order_builtin_minmax_binds_vararg_shape",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "5",
        "1",
        "5",
        "1",
    ]


def test_native_scalar_subclass_hash_matches_value_equality(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "from enum import IntEnum\n"
            "\n"
            "class A(IntEnum):\n"
            "    X = 2\n"
            "\n"
            "class B(IntEnum):\n"
            "    X = 2\n"
            "\n"
            "class I(int):\n"
            "    pass\n"
            "\n"
            "class F(float):\n"
            "    pass\n"
            "\n"
            "print(hash(A.X), hash(B.X), hash(2))\n"
            "print(A.X == B.X, B.X in {A.X}, A.X in {2}, 2 in {A.X})\n"
            "print(hash(I(7)), hash(7), I(7) in {7}, 7 in {I(7)})\n"
            "print(hash(F(1.5)) == hash(1.5), F(1.5) in {1.5}, 1.5 in {F(1.5)})\n"
        ),
        "scalar_subclass_hash_matches_value_equality",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "2 2 2",
        "True True True True",
        "7 7 True True",
        "True True True",
    ]


def test_native_set_multi_methods_keep_operand_tuple_on_ic_hits(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "base = {1, 2}\n"
            "other = {3}\n"
            "for i in range(6):\n"
            "    u = base.union(other)\n"
            "    print(i, len(u), 3 in u, len(base), 3 in base)\n"
            "\n"
            "mutable = {1}\n"
            "for i in range(3):\n"
            "    mutable.update({i + 2})\n"
            "print(sorted(mutable))\n"
            "\n"
            "frozen = frozenset({1, 2})\n"
            "for i in range(4):\n"
            "    fu = frozen.union({3})\n"
            "    print('f', i, len(fu), 3 in fu, len(frozen), 3 in frozen)\n"
        ),
        "set_multi_methods_keep_operand_tuple_on_ic_hits",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "0 3 True 2 False",
        "1 3 True 2 False",
        "2 3 True 2 False",
        "3 3 True 2 False",
        "4 3 True 2 False",
        "5 3 True 2 False",
        "[1, 2, 3, 4]",
        "f 0 3 True 2 False",
        "f 1 3 True 2 False",
        "f 2 3 True 2 False",
        "f 3 3 True 2 False",
    ]


def test_native_collections_namedtuple_kwonly_defaults_bind_as_keywords(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "from collections import namedtuple\n"
            "\n"
            "Point = namedtuple('Point', ['x', 'y'])\n"
            "print(Point(3, 4))\n"
            "Renamed = namedtuple('Renamed', ['class', 'value'], rename=True)\n"
            "print(Renamed._fields)\n"
        ),
        "collections_namedtuple_kwonly_defaults_bind_as_keywords",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "Point(x=3, y=4)",
        "('_0', 'value')",
    ]


def test_native_direct_call_alias_returns_are_owned_across_fast_paths(
    tmp_path: Path,
) -> None:
    run = _build_and_run(
        tmp_path,
        (
            "def passthrough(x):\n"
            "    return x\n"
            "\n"
            "fn = passthrough\n"
            "value = fn([1, 2, 3])\n"
            "print(value[0])\n"
            "print(len(value))\n"
            "\n"
            "class Echo:\n"
            "    def echo(self, x):\n"
            "        return x\n"
            "\n"
            "obj = Echo()\n"
            "for i in range(6):\n"
            "    out = obj.echo(value)\n"
            "print(out[1])\n"
            "\n"
            "class Child(Echo):\n"
            "    def echo(self, x):\n"
            "        return super().echo(x)\n"
            "\n"
            "child = Child()\n"
            "for i in range(6):\n"
            "    inherited = child.echo(value)\n"
            "print(inherited[2])\n"
        ),
        "direct_call_alias_returns_owned",
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == ["1", "3", "2", "3"]
