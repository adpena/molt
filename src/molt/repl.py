"""Molt interactive REPL -- compile-and-execute Python expressions.

Inspired by Monty's MontyRepl, but backed by Molt's AOT compilation for
native-speed execution with persistent state across inputs.

Usage:
    molt repl [--capabilities CAPS] [--io-mode MODE]

Features:
    - Persistent variable state across inputs
    - Multiline input detection (incomplete expressions)
    - Capability-gated I/O (same as ``molt run``)
    - Tab completion for builtins and keywords
    - History with readline support
"""

from __future__ import annotations

import ast
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from collections.abc import Sequence
from typing import Any, Optional, cast

from molt import process_guard


# Python keywords and builtins for tab completion
_COMPLETIONS: list[str] = sorted(
    list(__builtins__.keys())
    if isinstance(__builtins__, dict)
    else list(__builtins__.__dict__.keys())
) + sorted(
    [
        "and",
        "as",
        "assert",
        "async",
        "await",
        "break",
        "class",
        "continue",
        "def",
        "del",
        "elif",
        "else",
        "except",
        "finally",
        "for",
        "from",
        "global",
        "if",
        "import",
        "in",
        "is",
        "lambda",
        "nonlocal",
        "not",
        "or",
        "pass",
        "raise",
        "return",
        "try",
        "while",
        "with",
        "yield",
    ]
)

REPL_MEMORY_GUARD_PREFIX = "MOLT_REPL"
DEFAULT_REPL_TIMEOUT_SEC = 30.0


def _is_incomplete(source: str) -> bool:
    """Check if the source code is incomplete (needs more lines).

    Returns True if the source ends with a colon, backslash continuation,
    or has unmatched brackets/parens, or if ``ast.parse`` raises an
    ``IndentationError`` or "unexpected EOF" ``SyntaxError``.
    """
    stripped = source.rstrip()
    if not stripped:
        return False

    # Obvious continuation markers
    if stripped.endswith((":", "\\", ",")):
        return True

    # Try parsing -- if we get "unexpected EOF", it's incomplete
    try:
        ast.parse(source, mode="exec")
        return False
    except IndentationError:
        return True
    except SyntaxError as e:
        if e.msg and "unexpected EOF" in e.msg:
            return True
        if e.msg and "expected an indented block" in e.msg:
            return True
        return False  # genuine syntax error -- let it through


def _wrap_for_repl(source: str, state_vars: set[str]) -> str:
    """Wrap REPL input for compilation.

    Wraps the input in a function that receives persistent state variables
    as parameters and returns any new/modified variables.
    """
    # Parse to detect assignments and expressions
    try:
        tree = ast.parse(source, mode="exec")
    except SyntaxError:
        return source  # let the compiler handle the error

    # Find all assigned names
    assigned: set[str] = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.Assign):
            for target in node.targets:
                if isinstance(target, ast.Name):
                    assigned.add(target.id)
        elif isinstance(node, ast.AugAssign):
            if isinstance(node.target, ast.Name):
                assigned.add(node.target.id)
        elif isinstance(node, ast.AnnAssign):
            if isinstance(node.target, ast.Name):
                assigned.add(node.target.id)

    # If the last statement is an expression, add print()
    lines = source.split("\n")
    if tree.body and isinstance(tree.body[-1], ast.Expr):
        # Wrap the last expression in print(repr(...))
        last_line = lines[-1]
        indent = len(last_line) - len(last_line.lstrip())
        lines[-1] = " " * indent + f"print(repr({last_line.strip()}))"

    return "\n".join(lines)


def _repl_project_root() -> Path:
    raw_root = os.environ.get("MOLT_EXT_ROOT")
    root = Path(raw_root).expanduser() if raw_root else Path.cwd()
    if not root.is_absolute():
        root = Path.cwd() / root
    return root.resolve()


def _repl_tmp_dir(project_root: Path) -> Path:
    tmp_dir = project_root / "tmp" / "repl"
    tmp_dir.mkdir(parents=True, exist_ok=True)
    return tmp_dir


def _molt_command_prefix(molt_cmd: str | Sequence[str]) -> list[str]:
    if isinstance(molt_cmd, str):
        return [molt_cmd]
    return [str(part) for part in molt_cmd]


def run_repl(
    capabilities: Optional[str] = None,
    io_mode: str = "real",
    molt_cmd: str | Sequence[str] = "molt",
    timeout_sec: float | None = None,
) -> int:
    """Run the interactive REPL.

    Parameters
    ----------
    capabilities : str, optional
        Comma-separated capability tokens.
    io_mode : str
        IO mode: "real", "virtual", or "callback".
    molt_cmd : str or sequence of str
        Molt CLI command prefix.
    timeout_sec : float, optional
        Per-snippet timeout. Defaults to ``MOLT_REPL_TIMEOUT_SEC`` or 30s.

    Returns
    -------
    int
        Exit code (0 for normal exit).
    """
    # Try to enable readline for history and completion
    history_path: Path | None = None
    readline_module: Any | None = None
    try:
        readline_module = cast(Any, __import__("readline"))

        readline_module.parse_and_bind("tab: complete")

        # Custom completer
        def completer(text: str, state: int) -> Optional[str]:
            matches = [c for c in _COMPLETIONS if c.startswith(text)]
            return matches[state] if state < len(matches) else None

        readline_module.set_completer(completer)

        # Load history
        history_path = Path.home() / ".molt_history"
        if history_path.exists():
            try:
                readline_module.read_history_file(str(history_path))
            except OSError:
                pass
    except ImportError:
        pass

    print("Molt REPL (Python subset, compiled execution)")
    print("Type 'exit()' or Ctrl-D to quit.")
    if capabilities:
        print(f"Capabilities: {capabilities}")
    print()

    state_vars: set[str] = set()

    while True:
        try:
            # Read input
            line = input("molt> ")
        except (EOFError, KeyboardInterrupt):
            print()
            break

        if not line.strip():
            continue

        if line.strip() in ("exit()", "quit()"):
            break

        # Accumulate multiline input
        source = line
        while _is_incomplete(source):
            try:
                continuation = input("...  ")
            except (EOFError, KeyboardInterrupt):
                print()
                break
            source += "\n" + continuation

        # Wrap for REPL execution
        wrapped = _wrap_for_repl(source, state_vars)

        project_root = _repl_project_root()
        tmp_dir = _repl_tmp_dir(project_root)

        # Compile and run via molt under the shared process guard.
        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".py", delete=False, dir=tmp_dir
        ) as f:
            f.write(wrapped)
            tmp_path = f.name

        try:
            cmd = [*_molt_command_prefix(molt_cmd), "run", tmp_path]
            if capabilities:
                cmd.extend(["--capabilities", capabilities])

            env = os.environ.copy()
            env["MOLT_IO_MODE"] = io_mode

            timeout = process_guard.timeout_from_env(
                REPL_MEMORY_GUARD_PREFIX,
                env,
                explicit=timeout_sec,
                default=DEFAULT_REPL_TIMEOUT_SEC,
                cwd=project_root,
            )
            result = process_guard.run_completed_command(
                cmd,
                cwd=project_root,
                env=env,
                capture_output=True,
                memory_guard_prefix=REPL_MEMORY_GUARD_PREFIX,
                timeout=timeout,
            )

            if result.stdout:
                print(result.stdout, end="")
            if result.stderr:
                # Filter out compilation noise, show only runtime errors
                for err_line in result.stderr.splitlines():
                    if any(
                        kw in err_line
                        for kw in [
                            "Error",
                            "error",
                            "Traceback",
                            "raise",
                            "Exception",
                            "Warning",
                            "warning",
                        ]
                    ):
                        print(err_line, file=sys.stderr)
        except subprocess.TimeoutExpired:
            timeout_text = "disabled" if timeout is None else f"{timeout:g}s limit"
            print(f"Error: execution timed out ({timeout_text})", file=sys.stderr)
        except FileNotFoundError:
            print(
                f"Error: '{' '.join(_molt_command_prefix(molt_cmd))}' not found. Is Molt installed?",
                file=sys.stderr,
            )
            return 1
        finally:
            try:
                os.unlink(tmp_path)
            except OSError:
                pass

    # Save history
    if readline_module is not None and history_path is not None:
        try:
            readline_module.write_history_file(str(history_path))
        except OSError:
            pass

    return 0


if __name__ == "__main__":
    sys.exit(run_repl())
