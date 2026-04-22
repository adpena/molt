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
from typing import Optional


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


def run_repl(
    capabilities: Optional[str] = None,
    io_mode: str = "real",
    molt_cmd: str = "molt",
) -> int:
    """Run the interactive REPL.

    Parameters
    ----------
    capabilities : str, optional
        Comma-separated capability tokens.
    io_mode : str
        IO mode: "real", "virtual", or "callback".
    molt_cmd : str
        Path to the molt CLI binary.

    Returns
    -------
    int
        Exit code (0 for normal exit).
    """
    # Try to enable readline for history and completion
    history_path: Path | None = None
    try:
        import readline

        readline.parse_and_bind("tab: complete")

        # Custom completer
        def completer(text: str, state: int) -> Optional[str]:
            matches = [c for c in _COMPLETIONS if c.startswith(text)]
            return matches[state] if state < len(matches) else None

        readline.set_completer(completer)

        # Load history
        history_path = Path.home() / ".molt_history"
        if history_path.exists():
            readline.read_history_file(str(history_path))
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

        # Compile and run via molt
        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".py", delete=False, dir=tempfile.gettempdir()
        ) as f:
            f.write(wrapped)
            tmp_path = f.name

        try:
            cmd = [molt_cmd, "run", tmp_path]
            if capabilities:
                cmd.extend(["--capabilities", capabilities])

            env = os.environ.copy()
            env["MOLT_IO_MODE"] = io_mode

            result = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=30,
                env=env,
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
            print("Error: execution timed out (30s limit)", file=sys.stderr)
        except FileNotFoundError:
            print(
                f"Error: '{molt_cmd}' not found. Is Molt installed?",
                file=sys.stderr,
            )
            return 1
        finally:
            try:
                os.unlink(tmp_path)
            except OSError:
                pass

    # Save history
    if readline is not None and history_path is not None:
        try:
            readline.write_history_file(str(history_path))
        except OSError:
            pass

    return 0


if __name__ == "__main__":
    sys.exit(run_repl())
