import re
from pathlib import Path


def _load(path: Path) -> str:
    if not path.exists():
        return ""
    return path.read_text()


def _extract_todos(text: str, pattern: str) -> set[str]:
    return set(re.findall(pattern, text))


def main() -> int:
    matrix = Path("docs/spec/0014_TYPE_COVERAGE_MATRIX.md")
    stdlib = Path("docs/spec/0015_STDLIB_COMPATIBILITY_MATRIX.md")
    roadmap = Path("ROADMAP.md")

    matrix_todos = _extract_todos(_load(matrix), r"TODO\\(type-coverage[^)]*\\)")
    stdlib_todos = _extract_todos(_load(stdlib), r"TODO\\(stdlib-compat[^)]*\\)")
    roadmap_todos = _extract_todos(
        _load(roadmap),
        r"TODO\\((type-coverage|stdlib-compat)[^)]*\\)",
    )

    missing = sorted((matrix_todos | stdlib_todos) - roadmap_todos)
    if missing:
        print("Missing spec TODOs in ROADMAP.md:")
        for todo in missing:
            print(f"  - {todo}")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
