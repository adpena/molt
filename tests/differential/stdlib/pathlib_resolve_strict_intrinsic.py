# MOLT_ENV: MOLT_CAPABILITIES=fs.read
"""Purpose: intrinsic-backed pathlib resolve strict/non-strict parity."""

from pathlib import Path
import tempfile


def main() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        base = Path(tmp)
        missing = base / "missing" / "leaf.txt"
        resolved = missing.resolve()
        print("strict_false_abs", resolved.is_absolute())
        print("strict_false_tail", resolved.as_posix().endswith("missing/leaf.txt"))
        try:
            missing.resolve(strict=True)
            print("strict_true", "no-error")
        except FileNotFoundError:
            print("strict_true", "file-not-found")
        except Exception as exc:  # pragma: no cover - differential output only
            print("strict_true", type(exc).__name__)


if __name__ == "__main__":
    main()
