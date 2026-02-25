from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
import textwrap
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[1]
EXT_ROOT = Path("/Volumes/APDataStore/Molt")


def _python_executable() -> str:
    exe = sys.executable
    if exe and os.path.exists(exe) and os.access(exe, os.X_OK):
        return exe
    fallback = shutil.which("python3") or shutil.which("python")
    if fallback:
        return fallback
    return exe


def _live_smoke_enabled() -> bool:
    raw = os.environ.get("MOLT_TK_LIVE_SMOKE", "")
    return raw.strip().lower() in {"1", "true", "yes", "on"}


def _require_live_smoke_prereqs(expected_platform: str) -> None:
    if not _live_smoke_enabled():
        pytest.skip("set MOLT_TK_LIVE_SMOKE=1 to run live tkinter smoke tests")
    if expected_platform == "linux":
        if not (os.environ.get("DISPLAY") or os.environ.get("WAYLAND_DISPLAY")):
            pytest.skip("linux live tkinter smoke requires DISPLAY or WAYLAND_DISPLAY")
    if not EXT_ROOT.is_dir():
        pytest.skip(f"external artifact root not mounted: {EXT_ROOT}")


def _build_env() -> dict[str, str]:
    env = os.environ.copy()
    env["PYTHONPATH"] = str(ROOT / "src")
    env["MOLT_EXT_ROOT"] = str(EXT_ROOT)
    env["CARGO_TARGET_DIR"] = str(EXT_ROOT / "cargo-target-tk-live-smoke")
    env["MOLT_DIFF_CARGO_TARGET_DIR"] = env["CARGO_TARGET_DIR"]
    env["MOLT_CACHE"] = str(EXT_ROOT / "molt_cache")
    env["MOLT_DIFF_ROOT"] = str(EXT_ROOT / "diff")
    env["MOLT_DIFF_TMPDIR"] = str(EXT_ROOT / "tmp")
    env["UV_CACHE_DIR"] = str(EXT_ROOT / "uv-cache")
    env["TMPDIR"] = str(EXT_ROOT / "tmp")
    env["MOLT_BACKEND_DAEMON"] = "0"
    env["MOLT_USE_SCCACHE"] = "0"
    env["MOLT_RUNTIME_TK_NATIVE"] = "1"
    return env


def _platform_gate_source(expected_platform: str) -> str:
    if expected_platform == "linux":
        return 'sys.platform.startswith("linux")'
    if expected_platform == "darwin":
        return 'sys.platform == "darwin"'
    if expected_platform == "win32":
        return 'sys.platform == "win32"'
    raise RuntimeError(f"unsupported platform gate: {expected_platform}")


def _live_smoke_script(expected_platform: str) -> str:
    platform_gate = _platform_gate_source(expected_platform)
    return textwrap.dedent(
        f"""
        import sys
        import tkinter as tk
        from tkinter import simpledialog, ttk

        if not ({platform_gate}):
            print("SKIP:platform-mismatch")
            raise SystemExit(0)

        try:
            root = tk.Tk()
        except Exception as exc:  # noqa: BLE001
            print(f"SKIP:tk-root:{{type(exc).__name__}}:{{exc}}")
            raise SystemExit(0)

        root.withdraw()
        tree = ttk.Treeview(root, columns=("value",))
        tree.pack()
        tree.insert("", "end", iid="row1", text="row-one", values=("v0",), tags=("tag-one",))
        tree.focus("row1")
        tree.selection_set("row1")

        state = {{
            "timer_fired": False,
            "bind_widget_ok": False,
            "selection": (),
            "tag_calls": 0,
            "tag_widget_ok": False,
        }}

        def on_select(event):
            state["bind_widget_ok"] = getattr(event, "widget", None) is tree
            state["selection"] = tuple(tree.selection())

        def on_tag(event):
            state["tag_calls"] += 1
            state["tag_widget_ok"] = getattr(event, "widget", None) is tree

        bind_id = tree.bind("<<TreeviewSelect>>", on_select)
        tag_id = tree.tag_bind("tag-one", "<<TreeviewOpen>>", on_tag)
        tag_query_before = tree.tag_bind("tag-one", "<<TreeviewOpen>>")

        def trigger():
            state["timer_fired"] = True
            tree.set("row1", "value", "v1")
            root.tk.call(bind_id)
            root.tk.call(tag_id)
            root.after(10, root.quit)

        root.after(15, trigger)
        root.mainloop()

        tag_unbind_result = tree.tag_unbind("tag-one", "<<TreeviewOpen>>", tag_id)
        tag_query_after = tree.tag_bind("tag-one", "<<TreeviewOpen>>")

        children = tuple(tree.get_children())
        value = tree.set("row1", "value")
        text = tree.item("row1", "text")

        def _drive_simpledialog_input(next_value):
            attempts = {{"count": 0}}

            def _tick():
                attempts["count"] += 1
                children_now = root.tk.splitlist(root.tk.call("winfo", "children", "."))
                target = None
                for child in children_now:
                    child_name = str(child)
                    if child_name.startswith(".__molt_simpledialog_"):
                        target = child_name
                        break
                if target is None:
                    if attempts["count"] < 2000:
                        root.after(5, _tick)
                    return
                root.tk.call(f"{{target}}.body.entry", "delete", 0, "end")
                root.tk.call(f"{{target}}.body.entry", "insert", 0, str(next_value))
                root.tk.call(f"{{target}}.buttons.ok", "invoke")

            root.after(5, _tick)

        _drive_simpledialog_input("live-string")
        simpledialog_string = simpledialog.askstring(
            "SimpleDialog",
            "Enter string",
            parent=root,
            initialvalue="seed",
        )
        _drive_simpledialog_input("42")
        simpledialog_int = simpledialog.askinteger(
            "SimpleDialog",
            "Enter int",
            parent=root,
            initialvalue="7",
            minvalue=1,
            maxvalue=100,
        )
        _drive_simpledialog_input("2.25")
        simpledialog_float = simpledialog.askfloat(
            "SimpleDialog",
            "Enter float",
            parent=root,
            initialvalue="1.0",
            minvalue=1.0,
            maxvalue=3.0,
        )

        ok = (
            isinstance(bind_id, str)
            and isinstance(tag_id, str)
            and isinstance(tag_query_before, str)
            and tag_id in tag_query_before
            and tag_query_after == ""
            and tag_unbind_result is None
            and state["timer_fired"] is True
            and state["bind_widget_ok"] is True
            and state["selection"] == ("row1",)
            and state["tag_calls"] == 1
            and state["tag_widget_ok"] is True
            and children == ("row1",)
            and value == "v1"
            and text == "row-one"
            and simpledialog_string == "live-string"
            and simpledialog_int == 42
            and simpledialog_float == 2.25
        )
        root.destroy()

        print("OK" if ok else "FAIL")
        print(
            "DETAIL:",
            f"platform={{sys.platform}}",
            f"bind_id={{bind_id!r}}",
            f"tag_id={{tag_id!r}}",
            f"selection={{state['selection']!r}}",
            f"tag_calls={{state['tag_calls']!r}}",
            f"children={{children!r}}",
            f"value={{value!r}}",
            f"text={{text!r}}",
            f"simpledialog_string={{simpledialog_string!r}}",
            f"simpledialog_int={{simpledialog_int!r}}",
            f"simpledialog_float={{simpledialog_float!r}}",
        )
        if not ok:
            raise SystemExit(3)
        """
    )


def _run_live_smoke(expected_platform: str) -> None:
    _require_live_smoke_prereqs(expected_platform)
    base_tmp = EXT_ROOT / "tmp"
    base_tmp.mkdir(parents=True, exist_ok=True)
    run_dir = Path(
        tempfile.mkdtemp(prefix=f"tk_live_{expected_platform}_", dir=str(base_tmp))
    )
    script_path = run_dir / "live_smoke.py"
    out_dir = run_dir / "out"
    output = out_dir / f"tk_live_{expected_platform}_molt"
    script_path.write_text(_live_smoke_script(expected_platform))

    env = _build_env()
    build_cmd = [
        _python_executable(),
        "-m",
        "molt.cli",
        "build",
        "--profile",
        "dev",
        str(script_path),
        "--out-dir",
        str(out_dir),
        "--output",
        str(output),
    ]
    build = subprocess.run(
        build_cmd,
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=900,
    )
    assert build.returncode == 0, (
        f"live tkinter smoke build failed for {expected_platform}\n"
        f"stdout:\n{build.stdout}\n\nstderr:\n{build.stderr}"
    )

    run = subprocess.run(
        [str(output)],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=120,
    )
    first_line = next((line for line in run.stdout.splitlines() if line), "")
    if first_line.startswith("SKIP:"):
        pytest.skip(first_line)
    assert run.returncode == 0, (
        f"live tkinter smoke run failed for {expected_platform}\n"
        f"stdout:\n{run.stdout}\n\nstderr:\n{run.stderr}"
    )
    assert any(line == "OK" for line in run.stdout.splitlines()), (
        f"live tkinter smoke did not report success for {expected_platform}\n"
        f"stdout:\n{run.stdout}\n\nstderr:\n{run.stderr}"
    )


@pytest.mark.skipif(sys.platform != "darwin", reason="macOS-only live Tk smoke")
def test_tkinter_live_smoke_macos() -> None:
    _run_live_smoke("darwin")


@pytest.mark.skipif(
    not sys.platform.startswith("linux"), reason="Linux-only live Tk smoke"
)
def test_tkinter_live_smoke_linux() -> None:
    _run_live_smoke("linux")


@pytest.mark.skipif(sys.platform != "win32", reason="Windows-only live Tk smoke")
def test_tkinter_live_smoke_windows() -> None:
    _run_live_smoke("win32")
