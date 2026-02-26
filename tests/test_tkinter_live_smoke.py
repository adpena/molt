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
        import _tkinter
        import os
        import sys
        import tkinter as tk
        from tkinter import simpledialog

        if not ({platform_gate}):
            print("SKIP:platform-mismatch")
            raise SystemExit(0)

        try:
            root = tk.Tk()
        except Exception as exc:  # noqa: BLE001
            print(f"SKIP:tk-root:{{type(exc).__name__}}:{{exc}}")
            raise SystemExit(0)

        root.withdraw()
        widget = tk.Frame(root, width=120, height=80)
        widget.pack()

        state = {{
            "timer_fired": False,
            "bind_widget_ok": False,
            "bind_payload": None,
        }}

        def on_bind(event):
            state["bind_widget_ok"] = getattr(event, "widget", None) is widget
            state["bind_payload"] = (
                getattr(event, "x", None),
                getattr(event, "y", None),
                getattr(event, "delta", None),
                getattr(event, "keysym", None),
                getattr(event, "char", None),
                getattr(event, "serial", None),
                getattr(event, "type", None),
                getattr(event, "x_root", None),
                getattr(event, "y_root", None),
            )

        bind_id = widget.bind("<KeyPress>", on_bind)
        bind_query_before = widget.bind("<KeyPress>")

        dooneevent_ticks = []

        def dooneevent_tick():
            dooneevent_ticks.append("fired")

        root.after(1, dooneevent_tick)
        for _ in range(400):
            if dooneevent_ticks:
                break
            root.dooneevent(tk.DONT_WAIT)

        trace_events = []
        trace_var = tk.StringVar(root, value="seed")
        trace_id = trace_var.trace_add(
            "write",
            lambda name, index, mode: trace_events.append((name, index, mode)),
        )
        trace_var.set("seed-updated")
        trace_var.trace_remove("write", trace_id)
        trace_info_after = tuple(trace_var.trace_info())

        wait_var = tk.StringVar(root, value="pending")
        root.after(5, lambda: wait_var.set("ready"))
        root.wait_variable(wait_var)
        wait_var_value = wait_var.get()

        after_cancel_hits = []
        cancel_token = root.after(50, lambda: after_cancel_hits.append("fired"))
        root.after_cancel(cancel_token)
        for _ in range(200):
            root.dooneevent(tk.DONT_WAIT)
        after_cancel_ok = after_cancel_hits == []

        filehandler_events = []
        filehandler_contract_ok = False
        read_fd = None
        write_fd = None
        try:
            if sys.platform == "win32":
                try:
                    root.createfilehandler(0, _tkinter.READABLE, lambda *_args: None)
                except NotImplementedError:
                    filehandler_contract_ok = True
                except Exception:
                    filehandler_contract_ok = False
                else:
                    filehandler_contract_ok = False
            else:
                read_fd, write_fd = os.pipe()

                def _on_file_ready(fileobj, mask):
                    try:
                        fd_value = int(fileobj)
                    except Exception:  # noqa: BLE001
                        try:
                            fd_value = int(fileobj.fileno())
                        except Exception:  # noqa: BLE001
                            fd_value = -1
                    filehandler_events.append((fd_value, int(mask)))
                    try:
                        os.read(read_fd, 1)
                    except Exception:  # noqa: BLE001
                        pass
                    try:
                        root.deletefilehandler(read_fd)
                    except Exception:  # noqa: BLE001
                        pass

                root.createfilehandler(read_fd, _tkinter.READABLE, _on_file_ready)
                os.write(write_fd, b"x")
                for _ in range(1000):
                    if filehandler_events:
                        break
                    root.dooneevent(tk.DONT_WAIT)
                expected_event = (read_fd, int(_tkinter.READABLE))
                filehandler_contract_ok = filehandler_events == [expected_event]
        finally:
            if read_fd is not None:
                try:
                    root.deletefilehandler(read_fd)
                except Exception:  # noqa: BLE001
                    pass
                try:
                    os.close(read_fd)
                except Exception:  # noqa: BLE001
                    pass
            if write_fd is not None:
                try:
                    os.close(write_fd)
                except Exception:  # noqa: BLE001
                    pass

        def trigger():
            state["timer_fired"] = True
            root.tk.call(
                bind_id,
                "55",
                "1",
                "1",
                "10",
                "11",
                "12",
                "13",
                "14",
                "15",
                "16",
                "A",
                "1",
                "K",
                "65",
                widget._w,
                "KeyPress",
                "115",
                "116",
                "120",
            )
            root.after(10, root.quit)

        root.after(15, trigger)
        root.mainloop()

        bind_after = widget.bind("<KeyPress>")
        widget.unbind("<KeyPress>", bind_id)
        bind_after_unbind = widget.bind("<KeyPress>")
        children = tuple(root.winfo_children())

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
            and isinstance(bind_query_before, str)
            and bind_id in bind_query_before
            and bind_after == bind_query_before
            and bind_after_unbind == ""
            and state["timer_fired"] is True
            and dooneevent_ticks == ["fired"]
            and trace_events == [(trace_var._name, "", "write")]
            and trace_info_after == ()
            and wait_var_value == "ready"
            and after_cancel_ok is True
            and filehandler_contract_ok is True
            and state["bind_widget_ok"] is True
            and state["bind_payload"] == (15, 16, 120, "K", "A", 55, "KeyPress", 115, 116)
            and len(children) == 1
            and str(children[0]) == str(widget)
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
            f"bind_query_before={{bind_query_before!r}}",
            f"bind_payload={{state['bind_payload']!r}}",
            f"bind_after={{bind_after!r}}",
            f"bind_after_unbind={{bind_after_unbind!r}}",
            f"dooneevent_ticks={{dooneevent_ticks!r}}",
            f"trace_events={{trace_events!r}}",
            f"trace_info_after={{trace_info_after!r}}",
            f"wait_var_value={{wait_var_value!r}}",
            f"after_cancel_ok={{after_cancel_ok!r}}",
            f"filehandler_events={{filehandler_events!r}}",
            f"filehandler_contract_ok={{filehandler_contract_ok!r}}",
            f"children={{children!r}}",
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


def _live_filehandler_smoke_script(expected_platform: str) -> str:
    platform_gate = _platform_gate_source(expected_platform)
    return textwrap.dedent(
        f"""
        import _tkinter
        import os
        import sys
        import tkinter as tk

        if not ({platform_gate}):
            print("SKIP:platform-mismatch")
            raise SystemExit(0)

        try:
            root = tk.Tk()
        except Exception as exc:  # noqa: BLE001
            print(f"SKIP:tk-root:{{type(exc).__name__}}:{{exc}}")
            raise SystemExit(0)

        root.withdraw()
        events = []
        ok = False
        read_fd = None
        write_fd = None
        try:
            if sys.platform == "win32":
                try:
                    root.createfilehandler(0, _tkinter.READABLE, lambda *_args: None)
                except NotImplementedError:
                    ok = True
                except Exception:  # noqa: BLE001
                    ok = False
                else:
                    ok = False
            else:
                read_fd, write_fd = os.pipe()

                def _on_ready(fileobj, mask):
                    try:
                        fd = int(fileobj)
                    except Exception:  # noqa: BLE001
                        try:
                            fd = int(fileobj.fileno())
                        except Exception:  # noqa: BLE001
                            fd = -1
                    events.append((fd, int(mask)))
                    try:
                        os.read(read_fd, 1)
                    except Exception:  # noqa: BLE001
                        pass
                    root.deletefilehandler(read_fd)

                root.createfilehandler(read_fd, _tkinter.READABLE, _on_ready)
                os.write(write_fd, b"x")
                for _ in range(1000):
                    if events:
                        break
                    root.dooneevent(tk.DONT_WAIT)
                ok = events == [(read_fd, int(_tkinter.READABLE))]
        finally:
            if read_fd is not None:
                try:
                    root.deletefilehandler(read_fd)
                except Exception:  # noqa: BLE001
                    pass
                try:
                    os.close(read_fd)
                except Exception:  # noqa: BLE001
                    pass
            if write_fd is not None:
                try:
                    os.close(write_fd)
                except Exception:  # noqa: BLE001
                    pass
            root.destroy()

        print("OK" if ok else "FAIL")
        print(f"DETAIL:platform={{sys.platform}} events={{events!r}}")
        if not ok:
            raise SystemExit(3)
        """
    )


def _run_live_filehandler_smoke(expected_platform: str) -> None:
    _require_live_smoke_prereqs(expected_platform)
    base_tmp = EXT_ROOT / "tmp"
    base_tmp.mkdir(parents=True, exist_ok=True)
    run_dir = Path(
        tempfile.mkdtemp(
            prefix=f"tk_filehandler_{expected_platform}_", dir=str(base_tmp)
        )
    )
    script_path = run_dir / "live_filehandler_smoke.py"
    out_dir = run_dir / "out"
    output = out_dir / f"tk_filehandler_{expected_platform}_molt"
    script_path.write_text(_live_filehandler_smoke_script(expected_platform))

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
        f"live tkinter filehandler smoke build failed for {expected_platform}\n"
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
        f"live tkinter filehandler smoke run failed for {expected_platform}\n"
        f"stdout:\n{run.stdout}\n\nstderr:\n{run.stderr}"
    )
    assert any(line == "OK" for line in run.stdout.splitlines()), (
        f"live tkinter filehandler smoke did not report success for {expected_platform}\n"
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


@pytest.mark.skipif(sys.platform != "darwin", reason="macOS-only live Tk smoke")
def test_tkinter_live_filehandler_smoke_macos() -> None:
    _run_live_filehandler_smoke("darwin")


@pytest.mark.skipif(
    not sys.platform.startswith("linux"), reason="Linux-only live Tk smoke"
)
def test_tkinter_live_filehandler_smoke_linux() -> None:
    _run_live_filehandler_smoke("linux")


@pytest.mark.skipif(sys.platform != "win32", reason="Windows-only live Tk smoke")
def test_tkinter_live_filehandler_smoke_windows() -> None:
    _run_live_filehandler_smoke("win32")
