from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

import pytest

from molt.dx import cargo_target_dir_for_artifact_root, development_artifact_env
from tests.wasm_linked_runner import _run_wasm_test_process

ROOT = Path(__file__).resolve().parents[1]


def _split_runtime_imported_module_target_dirs(
    env: dict[str, str],
    *,
    cargo_target_dir: Path | None = None,
    session_id: str = "split-runtime-imported-module",
) -> tuple[Path, Path]:
    if cargo_target_dir is not None:
        return cargo_target_dir, cargo_target_dir
    raw_target = env.get("CARGO_TARGET_DIR", "").strip()
    if raw_target:
        target_dir = Path(raw_target).expanduser()
    else:
        raw_root = env.get("MOLT_EXT_ROOT", "").strip()
        artifact_root = Path(raw_root).expanduser() if raw_root else ROOT
        if not artifact_root.is_absolute():
            artifact_root = ROOT / artifact_root
        target_dir = cargo_target_dir_for_artifact_root(
            artifact_root.resolve(),
            env.get("MOLT_SESSION_ID") or session_id,
        )
    raw_diff_target = env.get("MOLT_DIFF_CARGO_TARGET_DIR", "").strip()
    diff_target_dir = (
        Path(raw_diff_target).expanduser() if raw_diff_target else target_dir
    )
    return target_dir, diff_target_dir


def _split_runtime_imported_module_build_env(
    *,
    session_id: str = "split-runtime-imported-module",
) -> dict[str, str]:
    env = development_artifact_env(
        ROOT,
        os.environ,
        session_prefix=session_id,
        session_id=os.environ.get("MOLT_SESSION_ID") or session_id,
        create_dirs=True,
    )
    env["MOLT_BACKEND_DAEMON"] = "0"
    env.setdefault("CARGO_BUILD_JOBS", "1")
    env.setdefault("MOLT_WASM_DISABLE_SCCACHE", "1")
    env.setdefault("MOLT_BUILD_LOCK_TIMEOUT", "45")
    env.setdefault("MOLT_CARGO_TIMEOUT", "900")
    return env


def test_split_runtime_imported_module_target_dir_respects_explicit_env_override() -> (
    None
):
    env = {
        "CARGO_TARGET_DIR": "/tmp/molt-imported-target",
        "MOLT_DIFF_CARGO_TARGET_DIR": "/tmp/molt-imported-diff-target",
    }

    target_dir, diff_target_dir = _split_runtime_imported_module_target_dirs(env)

    assert target_dir == Path("/tmp/molt-imported-target")
    assert diff_target_dir == Path("/tmp/molt-imported-diff-target")


def test_split_runtime_imported_module_target_dir_prefers_explicit_arg() -> None:
    explicit = Path("/tmp/molt-explicit-arg")

    target_dir, diff_target_dir = _split_runtime_imported_module_target_dirs(
        {}, cargo_target_dir=explicit
    )

    assert target_dir == explicit
    assert diff_target_dir == explicit


def test_split_runtime_imported_module_target_dir_defaults_to_dx_session_target() -> (
    None
):
    target_dir, diff_target_dir = _split_runtime_imported_module_target_dirs({})

    assert target_dir == cargo_target_dir_for_artifact_root(
        ROOT,
        "split-runtime-imported-module",
    )
    assert diff_target_dir == target_dir


def test_split_runtime_imported_module_build_uses_wasm_test_guard(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    source = tmp_path / "main.py"
    source.write_text("print('ok')\n", encoding="utf-8")
    out_dir = tmp_path / "out"
    out_dir.mkdir()
    captured: dict[str, object] = {}

    def fake_run(cmd, **kwargs):  # type: ignore[no-untyped-def]
        captured["cmd"] = list(cmd)
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

    monkeypatch.setattr(sys.modules[__name__], "_run_wasm_test_process", fake_run)

    result = _build_split(source, out_dir)

    assert result.returncode == 0
    assert "--split-runtime" in captured["cmd"]
    assert captured["kwargs"]["cwd"] == ROOT
    assert captured["kwargs"]["timeout"] == 300


def _build_split(
    source_file: Path,
    output_dir: Path,
    *,
    extra_env: dict[str, str] | None = None,
    cargo_target_dir: Path | None = None,
) -> subprocess.CompletedProcess[str]:
    env = _split_runtime_imported_module_build_env()
    target_dir, diff_target_dir = _split_runtime_imported_module_target_dirs(
        env, cargo_target_dir=cargo_target_dir
    )
    target_dir.mkdir(parents=True, exist_ok=True)
    diff_target_dir.mkdir(parents=True, exist_ok=True)
    env["CARGO_TARGET_DIR"] = str(target_dir)
    env["MOLT_DIFF_CARGO_TARGET_DIR"] = str(diff_target_dir)
    if extra_env:
        env.update(extra_env)
    cmd = [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        str(source_file),
        "--build-profile",
        "dev",
        "--target",
        "wasm",
        "--split-runtime",
        "--no-cache",
        "--out-dir",
        str(output_dir),
    ]
    return _run_wasm_test_process(
        cmd,
        env=env,
        cwd=ROOT,
        timeout=300,
    )


def _run_split_direct(output_dir: Path) -> subprocess.CompletedProcess[str]:
    env = os.environ.copy()
    env["MOLT_WASM_DIRECT_LINK"] = "1"
    env["MOLT_WASM_PREFER_LINKED"] = "0"
    env["MOLT_RUNTIME_WASM"] = str(output_dir / "molt_runtime.wasm")
    return _run_wasm_test_process(
        ["node", "wasm/run_wasm.js", str(output_dir / "app.wasm")],
        env=env,
        cwd=ROOT,
        timeout=300,
    )


@pytest.mark.slow
def test_split_runtime_imported_module_function_attr_survives_publication(
    tmp_path: Path,
) -> None:
    module_src = tmp_path / "probe_mod.py"
    module_src.write_text("def foo():\n    return 7\n")
    main_src = tmp_path / "probe_main.py"
    main_src.write_text(
        "import probe_mod\nprint(callable(probe_mod.foo))\nprint(probe_mod.foo())\n"
    )
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    build = _build_split(
        main_src,
        out_dir,
        extra_env={"MOLT_MODULE_ROOTS": str(tmp_path)},
    )
    assert build.returncode == 0, (
        f"split build failed (rc={build.returncode}).\n"
        f"stdout:\n{build.stdout[-2000:]}\n"
        f"stderr:\n{build.stderr[-2000:]}"
    )

    run = _run_split_direct(out_dir)
    assert run.returncode == 0, (
        f"direct-link run failed (rc={run.returncode}).\n"
        f"stdout:\n{run.stdout[-2000:]}\n"
        f"stderr:\n{run.stderr[-2000:]}"
    )
    assert run.stdout == "True\n7\n"


@pytest.mark.slow
def test_split_runtime_import_os_exposes_open_flags(tmp_path: Path) -> None:
    main_src = tmp_path / "probe_main.py"
    main_src.write_text("import os\nprint(os.O_RDONLY)\n")
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    build = _build_split(main_src, out_dir)
    assert build.returncode == 0, (
        f"split build failed (rc={build.returncode}).\n"
        f"stdout:\n{build.stdout[-2000:]}\n"
        f"stderr:\n{build.stderr[-2000:]}"
    )

    run = _run_split_direct(out_dir)
    assert run.returncode == 0, (
        f"direct-link run failed (rc={run.returncode}).\n"
        f"stdout:\n{run.stdout[-2000:]}\n"
        f"stderr:\n{run.stderr[-2000:]}"
    )
    assert run.stdout == "0\n"


@pytest.mark.slow
def test_split_runtime_import_builtins_direct_mode(tmp_path: Path) -> None:
    main_src = tmp_path / "probe_main.py"
    main_src.write_text("import builtins\n")
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    build = _build_split(main_src, out_dir)
    assert build.returncode == 0, (
        f"split build failed (rc={build.returncode}).\n"
        f"stdout:\n{build.stdout[-2000:]}\n"
        f"stderr:\n{build.stderr[-2000:]}"
    )

    run = _run_split_direct(out_dir)
    assert run.returncode == 0, (
        f"direct-link run failed (rc={run.returncode}).\n"
        f"stdout:\n{run.stdout[-2000:]}\n"
        f"stderr:\n{run.stderr[-2000:]}"
    )
    assert run.stdout == ""


@pytest.mark.slow
def test_split_runtime_import_importlib_direct_mode(tmp_path: Path) -> None:
    main_src = tmp_path / "probe_main.py"
    main_src.write_text("import importlib\nprint('hi')\n")
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    build = _build_split(main_src, out_dir)
    assert build.returncode == 0, (
        f"split build failed (rc={build.returncode}).\n"
        f"stdout:\n{build.stdout[-2000:]}\n"
        f"stderr:\n{build.stderr[-2000:]}"
    )

    run = _run_split_direct(out_dir)
    assert run.returncode == 0, (
        f"direct-link run failed (rc={run.returncode}).\n"
        f"stdout:\n{run.stdout[-2000:]}\n"
        f"stderr:\n{run.stderr[-2000:]}"
    )
    assert run.stdout == "hi\n"


@pytest.mark.slow
def test_split_runtime_sys_version_info_direct_mode(tmp_path: Path) -> None:
    main_src = tmp_path / "probe_main.py"
    main_src.write_text(
        "import sys\n"
        "print(type(sys.version_info).__name__)\n"
        "print(sys.version_info[0])\n"
    )
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    build = _build_split(main_src, out_dir)
    assert build.returncode == 0, (
        f"split build failed (rc={build.returncode}).\n"
        f"stdout:\n{build.stdout[-2000:]}\n"
        f"stderr:\n{build.stderr[-2000:]}"
    )

    run = _run_split_direct(out_dir)
    assert run.returncode == 0, (
        f"direct-link run failed (rc={run.returncode}).\n"
        f"stdout:\n{run.stdout[-2000:]}\n"
        f"stderr:\n{run.stderr[-2000:]}"
    )
    assert run.stdout == "version_info\n3\n"


@pytest.mark.slow
def test_split_runtime_typing_alias_bootstrap(tmp_path: Path) -> None:
    main_src = tmp_path / "probe_main.py"
    main_src.write_text(
        "from __future__ import annotations\n"
        "\n"
        "TYPE_CHECKING = False\n"
        "\n"
        "if TYPE_CHECKING:\n"
        "    from typing import Any, Iterator\n"
        "else:\n"
        "    class _TypingAlias:\n"
        "        __slots__ = ()\n"
        "\n"
        "        def __getitem__(self, _item):\n"
        "            return self\n"
        "\n"
        "    Any = object\n"
        "    Iterator = _TypingAlias()\n"
        "    ItemsView = _TypingAlias()\n"
        "    KeysView = _TypingAlias()\n"
        "    ValuesView = _TypingAlias()\n"
        "\n"
        "print('ok')\n"
    )
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    build = _build_split(main_src, out_dir)
    assert build.returncode == 0, (
        f"split build failed (rc={build.returncode}).\n"
        f"stdout:\n{build.stdout[-2000:]}\n"
        f"stderr:\n{build.stderr[-2000:]}"
    )

    run = _run_split_direct(out_dir)
    assert run.returncode == 0, (
        f"direct-link run failed (rc={run.returncode}).\n"
        f"stdout:\n{run.stdout[-2000:]}\n"
        f"stderr:\n{run.stderr[-2000:]}"
    )
    assert run.stdout == "ok\n"


@pytest.mark.slow
def test_split_runtime_import_typing_direct_mode(tmp_path: Path) -> None:
    main_src = tmp_path / "probe_main.py"
    main_src.write_text("import typing\nprint('ok')\n")
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    build = _build_split(main_src, out_dir)
    assert build.returncode == 0, (
        f"split build failed (rc={build.returncode}).\n"
        f"stdout:\n{build.stdout[-2000:]}\n"
        f"stderr:\n{build.stderr[-2000:]}"
    )

    env = os.environ.copy()
    env["MOLT_WASM_DIRECT_LINK"] = "1"
    env["MOLT_WASM_PREFER_LINKED"] = "0"
    env["MOLT_RUNTIME_WASM"] = str(out_dir / "molt_runtime.wasm")
    run = _run_wasm_test_process(
        ["node", "wasm/run_wasm.js", str(out_dir / "app.wasm")],
        env=env,
        cwd=ROOT,
        timeout=30,
    )
    assert run.returncode == 0, (
        f"direct-link run failed (rc={run.returncode}).\n"
        f"stdout:\n{run.stdout[-2000:]}\n"
        f"stderr:\n{run.stderr[-2000:]}"
    )
    assert run.stdout == "ok\n"


@pytest.mark.slow
def test_split_runtime_branch_local_object_merge_direct_mode(tmp_path: Path) -> None:
    main_src = tmp_path / "probe_main.py"
    main_src.write_text(
        "def f(value):\n"
        "    if value:\n"
        "        alias = value\n"
        "    else:\n"
        "        alias = (1, 2, 3, 4, 5)\n"
        "    return alias\n"
        "\n"
        "print(repr(f((3, 12, 0, 'final', 0))))\n"
    )
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    build = _build_split(main_src, out_dir)
    assert build.returncode == 0, (
        f"split build failed (rc={build.returncode}).\n"
        f"stdout:\n{build.stdout[-2000:]}\n"
        f"stderr:\n{build.stderr[-2000:]}"
    )

    run = _run_split_direct(out_dir)
    assert run.returncode == 0, (
        f"direct-link run failed (rc={run.returncode}).\n"
        f"stdout:\n{run.stdout[-2000:]}\n"
        f"stderr:\n{run.stderr[-2000:]}"
    )
    assert run.stdout == "(3, 12, 0, 'final', 0)\n"


@pytest.mark.slow
def test_split_runtime_annotated_staticmethod_tuple_param_direct_mode(
    tmp_path: Path,
) -> None:
    main_src = tmp_path / "probe_main.py"
    main_src.write_text(
        "class C:\n"
        "    @staticmethod\n"
        "    def m(values: tuple[int, ...]):\n"
        "        return len(values)\n"
        "\n"
        "print(C.m((1, 2, 3)))\n"
    )
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    build = _build_split(main_src, out_dir)
    assert build.returncode == 0, (
        f"split build failed (rc={build.returncode}).\n"
        f"stdout:\n{build.stdout[-2000:]}\n"
        f"stderr:\n{build.stderr[-2000:]}"
    )

    run = _run_split_direct(out_dir)
    assert run.returncode == 0, (
        f"direct-link run failed (rc={run.returncode}).\n"
        f"stdout:\n{run.stdout[-2000:]}\n"
        f"stderr:\n{run.stderr[-2000:]}"
    )
    assert run.stdout == "3\n"


@pytest.mark.slow
def test_split_runtime_generator_creation_direct_mode(tmp_path: Path) -> None:
    main_src = tmp_path / "probe_main.py"
    main_src.write_text("def _f():\n    yield\n\n_g = _f()\nprint(type(_g))\n")
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    build = _build_split(main_src, out_dir)
    assert build.returncode == 0, (
        f"split build failed (rc={build.returncode}).\n"
        f"stdout:\n{build.stdout[-2000:]}\n"
        f"stderr:\n{build.stderr[-2000:]}"
    )

    run = _run_split_direct(out_dir)
    assert run.returncode == 0, (
        f"direct-link run failed (rc={run.returncode}).\n"
        f"stdout:\n{run.stdout[-2000:]}\n"
        f"stderr:\n{run.stderr[-2000:]}"
    )
    assert run.stdout == "<class 'generator'>\n"


@pytest.mark.slow
def test_split_runtime_namedtuple_replace_direct_mode(tmp_path: Path) -> None:
    main_src = tmp_path / "probe_main.py"
    main_src.write_text(
        "from collections import namedtuple\n"
        "\n"
        "T = namedtuple('T', ['a', 'b'])\n"
        "print(T(1, 2)._replace(a=3))\n"
    )
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    build = _build_split(main_src, out_dir)
    assert build.returncode == 0, (
        f"split build failed (rc={build.returncode}).\n"
        f"stdout:\n{build.stdout[-2000:]}\n"
        f"stderr:\n{build.stderr[-2000:]}"
    )

    run = _run_split_direct(out_dir)
    assert run.returncode == 0, (
        f"direct-link run failed (rc={run.returncode}).\n"
        f"stdout:\n{run.stdout[-2000:]}\n"
        f"stderr:\n{run.stderr[-2000:]}"
    )
    assert run.stdout == "T(a=3, b=2)\n"


@pytest.mark.slow
def test_split_runtime_imported_module_load_safetensors_bytes_is_published(
    tmp_path: Path,
) -> None:
    main_src = tmp_path / "probe_main.py"
    main_src.write_text(
        "import molt.gpu.interop as interop\n"
        "print(hasattr(interop, 'load_safetensors_bytes'))\n"
        "print(type(interop.load_safetensors_bytes).__name__)\n"
    )
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    build = _build_split(main_src, out_dir)
    assert build.returncode == 0, (
        f"split build failed (rc={build.returncode}).\n"
        f"stdout:\n{build.stdout[-2000:]}\n"
        f"stderr:\n{build.stderr[-2000:]}"
    )

    run = _run_split_direct(out_dir)
    assert run.returncode == 0, (
        f"direct-link run failed (rc={run.returncode}).\n"
        f"stdout:\n{run.stdout[-2000:]}\n"
        f"stderr:\n{run.stderr[-2000:]}"
    )
    assert run.stdout == "True\nfunction\n"


def test_split_runtime_imported_module_getframe_globals_direct_mode(
    tmp_path: Path,
) -> None:
    module_src = tmp_path / "probe_mod.py"
    module_src.write_text(
        "import sys\n"
        "\n"
        "def probe():\n"
        "    return sys._getframe(1).f_globals.get('__name__', '__main__')\n"
    )
    main_src = tmp_path / "probe_main.py"
    main_src.write_text("from probe_mod import probe\nprint(probe())\n")
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    build = _build_split(
        main_src,
        out_dir,
        extra_env={"MOLT_MODULE_ROOTS": str(tmp_path)},
    )
    assert build.returncode == 0, (
        f"split build failed (rc={build.returncode}).\n"
        f"stdout:\n{build.stdout[-2000:]}\n"
        f"stderr:\n{build.stderr[-2000:]}"
    )

    run = _run_split_direct(out_dir)
    assert run.returncode == 0, (
        f"direct-link run failed (rc={run.returncode}).\n"
        f"stdout:\n{run.stdout[-2000:]}\n"
        f"stderr:\n{run.stderr[-2000:]}"
    )
    assert run.stdout == "__main__\n"


@pytest.mark.slow
def test_split_runtime_inline_python_function_returned_list_prints(
    tmp_path: Path,
) -> None:
    main_src = tmp_path / "probe_main.py"
    main_src.write_text("def f():\n    return [1, 2]\n\nprint(f())\n")
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    build = _build_split(main_src, out_dir)
    assert build.returncode == 0, (
        f"split build failed (rc={build.returncode}).\n"
        f"stdout:\n{build.stdout[-2000:]}\n"
        f"stderr:\n{build.stderr[-2000:]}"
    )

    run = _run_split_direct(out_dir)
    assert run.returncode == 0, (
        f"direct-link run failed (rc={run.returncode}).\n"
        f"stdout:\n{run.stdout[-2000:]}\n"
        f"stderr:\n{run.stderr[-2000:]}"
    )
    assert run.stdout == "[1, 2]\n"


@pytest.mark.slow
def test_split_runtime_module_loop_dict_store_direct_mode(tmp_path: Path) -> None:
    main_src = tmp_path / "probe_main.py"
    main_src.write_text(
        "_FIELDS = ('debug', 'inspect', 'interactive', 'optimize')\n"
        "_INDEX = {}\n"
        "for _i__SYS_FLAGS_SEQUENCE_INDEX in range(len(_FIELDS)):\n"
        "    _INDEX[_FIELDS[_i__SYS_FLAGS_SEQUENCE_INDEX]] = _i__SYS_FLAGS_SEQUENCE_INDEX\n"
        "print(_INDEX)\n"
    )
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    build = _build_split(main_src, out_dir)
    assert build.returncode == 0, (
        f"split build failed (rc={build.returncode}).\n"
        f"stdout:\n{build.stdout[-2000:]}\n"
        f"stderr:\n{build.stderr[-2000:]}"
    )

    run = _run_split_direct(out_dir)
    assert run.returncode == 0, (
        f"direct-link run failed (rc={run.returncode}).\n"
        f"stdout:\n{run.stdout[-2000:]}\n"
        f"stderr:\n{run.stderr[-2000:]}"
    )
    assert run.stdout == "{'debug': 0, 'inspect': 1, 'interactive': 2, 'optimize': 3}\n"


@pytest.mark.slow
def test_split_runtime_direct_mode_surfaces_unhandled_exception(
    tmp_path: Path,
) -> None:
    main_src = tmp_path / "probe_main.py"
    main_src.write_text("raise RuntimeError('boom')\n")
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    build = _build_split(main_src, out_dir)
    assert build.returncode == 0, (
        f"split build failed (rc={build.returncode}).\n"
        f"stdout:\n{build.stdout[-2000:]}\n"
        f"stderr:\n{build.stderr[-2000:]}"
    )

    run = _run_split_direct(out_dir)
    assert run.returncode != 0, (
        "direct-link run reported success despite an unhandled exception.\n"
        f"stdout:\n{run.stdout[-2000:]}\n"
        f"stderr:\n{run.stderr[-2000:]}"
    )
    assert "RuntimeError" in run.stderr or "boom" in run.stderr


@pytest.mark.slow
def test_split_runtime_import_typing_then_raise_direct_mode_surfaces_exception(
    tmp_path: Path,
) -> None:
    main_src = tmp_path / "probe_main.py"
    main_src.write_text("import typing\nraise RuntimeError('AFTER')\n")
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    build = _build_split(main_src, out_dir)
    assert build.returncode == 0, (
        f"split build failed (rc={build.returncode}).\n"
        f"stdout:\n{build.stdout[-2000:]}\n"
        f"stderr:\n{build.stderr[-2000:]}"
    )

    run = _run_split_direct(out_dir)
    assert run.returncode != 0, (
        "direct-link run reported success despite a raised exception after importing typing.\n"
        f"stdout:\n{run.stdout[-2000:]}\n"
        f"stderr:\n{run.stderr[-2000:]}"
    )
