"""Adapt Monty test files for Molt differential testing.

Monty test files use expectation comments:
  # Return=42          -> program should evaluate to 42
  # Return.str=hello   -> str(result) == "hello"
  # Raise=TypeError(...) -> program should raise TypeError
  # NoException         -> program should complete without error

This adapter creates modified copies that:
1. Wrap the main expression in print(repr(...)) for Return= tests
2. Add try/except for Raise= tests that print the exception
3. Leave assertion-only tests as-is (exit code is the signal)

Output goes to tests/harness/corpus/molt_adapted/
"""
from __future__ import annotations

import re
import sys
from pathlib import Path


def parse_expectation(filepath: Path) -> tuple[str, str]:
    """Parse expectation from test file comments and docstrings."""
    text = filepath.read_text()
    lines = text.strip().splitlines()

    # call-external files depend on helpers not in the file
    if lines and lines[0].strip() == "# call-external":
        return ("skip", "call-external")

    # Scan from bottom for comment-based expectations
    for line in reversed(lines):
        stripped = line.strip()
        if stripped.startswith("# Return="):
            return ("return", stripped[len("# Return="):])
        if stripped.startswith("# Return.str="):
            return ("return_str", stripped[len("# Return.str="):])
        if stripped.startswith("# Return.type="):
            return ("return_type", stripped[len("# Return.type="):])
        if stripped.startswith("# Raise="):
            return ("raise", stripped[len("# Raise="):])
        if stripped.startswith("# NoException"):
            return ("noexception", "")
        if stripped.startswith("# ref-counts="):
            return ("assert_only", "")  # Treat refcount metadata as assert-only
        if stripped.startswith("#"):
            continue
        break

    # Check for TRACEBACK: docstring block (135 files use this pattern)
    traceback_match = re.search(
        r'TRACEBACK:\s*\n.*?(\w+Error|\w+Exception|SyntaxError|ImportError)',
        text,
        re.DOTALL,
    )
    if traceback_match:
        exc_type = traceback_match.group(1)
        # Extract the exception message if present
        msg_match = re.search(
            exc_type + r':\s*(.+?)(?:\n|""")',
            text,
        )
        exc_msg = msg_match.group(1).strip() if msg_match else ""
        return ("raise", f"{exc_type}({exc_msg})" if exc_msg else exc_type)

    return ("assert_only", "")


def adapt_file(src: Path, dst: Path) -> bool:
    """Adapt a single Monty test file for Molt.

    Returns True if adapted, False if skipped.
    """
    kind, expected = parse_expectation(src)

    if kind == "skip":
        return False

    content = src.read_text()

    if kind == "return":
        # The file evaluates an expression on the last non-comment line.
        # Wrap it in print(repr(...))
        lines = content.strip().splitlines()
        for i in range(len(lines) - 1, -1, -1):
            stripped = lines[i].strip()
            if stripped and not stripped.startswith("#"):
                indent = len(lines[i]) - len(lines[i].lstrip())
                lines[i] = " " * indent + f"print(repr({stripped}))"
                break
        adapted = "\n".join(lines) + "\n"
        dst.write_text(adapted)
        dst.with_suffix(".expected").write_text(expected + "\n")
        return True

    elif kind == "return_str":
        lines = content.strip().splitlines()
        for i in range(len(lines) - 1, -1, -1):
            stripped = lines[i].strip()
            if stripped and not stripped.startswith("#"):
                indent = len(lines[i]) - len(lines[i].lstrip())
                lines[i] = " " * indent + f"print(str({stripped}))"
                break
        adapted = "\n".join(lines) + "\n"
        dst.write_text(adapted)
        dst.with_suffix(".expected").write_text(expected + "\n")
        return True

    elif kind == "return_type":
        lines = content.strip().splitlines()
        for i in range(len(lines) - 1, -1, -1):
            stripped = lines[i].strip()
            if stripped and not stripped.startswith("#"):
                indent = len(lines[i]) - len(lines[i].lstrip())
                lines[i] = " " * indent + f"print(type({stripped}).__name__)"
                break
        adapted = "\n".join(lines) + "\n"
        dst.write_text(adapted)
        dst.with_suffix(".expected").write_text(expected + "\n")
        return True

    elif kind == "raise":
        # Wrap in try/except, print exception type and message
        exc_match = re.match(r"(\w+)\((.*)\)", expected)
        if exc_match:
            exc_type = exc_match.group(1)
            exc_msg = exc_match.group(2).strip("'\"")
        else:
            exc_type = expected
            exc_msg = ""

        adapted = (
            "try:\n"
            + _indent(content, 4)
            + "\n"
            + f"except {exc_type} as e:\n"
            + f'    print(f"{exc_type}: {{e}}")\n'
            + "except Exception as e:\n"
            + '    print(f"WRONG_EXCEPTION: {type(e).__name__}: {e}")\n'
            + "else:\n"
            + '    print("NO_EXCEPTION_RAISED")\n'
        )
        dst.write_text(adapted)
        if exc_msg:
            dst.with_suffix(".expected").write_text(f"{exc_type}: {exc_msg}\n")
        else:
            dst.with_suffix(".expected").write_text(f"{exc_type}:\n")
        return True

    elif kind in ("noexception", "assert_only"):
        # Just copy -- exit code 0 means pass
        dst.write_text(content)
        dst.with_suffix(".expected").write_text("")  # empty = just check exit 0
        return True

    return False


def _indent(text: str, spaces: int) -> str:
    prefix = " " * spaces
    return "\n".join(prefix + line for line in text.splitlines())


def main() -> int:
    src_dir = Path("tests/harness/corpus/monty_compat")
    dst_dir = Path("tests/harness/corpus/molt_adapted")
    dst_dir.mkdir(parents=True, exist_ok=True)

    adapted = 0
    skipped = 0
    errors = 0

    for src_file in sorted(src_dir.glob("*.py")):
        try:
            if adapt_file(src_file, dst_dir / src_file.name):
                adapted += 1
            else:
                skipped += 1
        except Exception as e:
            errors += 1
            print(f"  ERROR {src_file.name}: {e}", file=sys.stderr)

    print(f"Adapted {adapted} files, skipped {skipped}, errors {errors}")
    print(f"Output: {dst_dir}/")
    return 0


if __name__ == "__main__":
    sys.exit(main())
