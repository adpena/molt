import os
import shutil
import sys
import tempfile
import textwrap
from pathlib import Path

from tests.wasm_harness import write_wasm_runner

ROOT = Path(__file__).resolve().parents[1]
TOOLS_ROOT = ROOT / "tools"
if str(TOOLS_ROOT) not in sys.path:
    sys.path.insert(0, str(TOOLS_ROOT))

import harness_memory_guard  # noqa: E402


def _run_guarded(cmd: list[str], *, cwd: Path, env: dict[str, str] | None = None):
    return harness_memory_guard.guarded_completed_process(
        cmd,
        prefix="MOLT_WASM_TEST",
        cwd=cwd,
        env=env,
        capture_output=True,
        text=True,
        timeout=harness_memory_guard.timeout_from_env(
            "MOLT_WASM_TEST",
            env or os.environ,
            default=300.0,
        ),
    )


def main() -> int:
    if shutil.which("node") is None:
        print("skip: node is required for wasm open parity")
        return 0
    if shutil.which("cargo") is None:
        print("skip: cargo is required for wasm open parity")
        return 0

    root = ROOT
    tmp_root = root / "tmp"
    tmp_root.mkdir(parents=True, exist_ok=True)
    tmpdir = Path(tempfile.mkdtemp(prefix="molt_wasm_open_", dir=tmp_root))
    src = tmpdir / "open_parity.py"
    src.write_text(
        textwrap.dedent(
            """\
            import os
            import tempfile
            from pathlib import Path

            def show(label, value):
                print(label, value)

            def show_err(label, func):
                try:
                    func()
                except Exception as exc:
                    print(label, type(exc).__name__, exc)

            path = Path(tempfile.gettempdir()) / f"molt_wasm_open_{os.getpid()}.txt"
            path.write_bytes(b"a\\r\\nb\\rc\\n")

            with open(path, "r", newline=None) as handle:
                show("newline_none", repr(handle.read()))

            show_err("encoding_binary", lambda: open(path, "rb", encoding="utf-8"))

            path.unlink()
            """
        )
    )

    output_wasm = tmpdir / "output.wasm"
    runner = write_wasm_runner(tmpdir, "run_wasm_open_parity.js")

    env = os.environ.copy()
    env["PYTHONPATH"] = str(root / "src")
    build = _run_guarded(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            str(src),
            "--target",
            "wasm",
            "--out-dir",
            str(tmpdir),
        ],
        cwd=root,
        env=env,
    )
    if build.returncode != 0:
        print(build.stderr, file=sys.stderr)
        return build.returncode

    run = _run_guarded(
        ["node", str(runner), str(output_wasm)],
        cwd=root,
    )
    if run.returncode != 0:
        print(run.stderr, file=sys.stderr)
        return run.returncode

    expected = (
        "newline_none 'a\\nb\\nc\\n'\\n"
        "encoding_binary ValueError binary mode doesn't take an encoding argument"
    )
    if run.stdout.strip() != expected:
        print("unexpected output:")
        print(run.stdout, file=sys.stderr)
        return 1
    print(run.stdout.strip())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
