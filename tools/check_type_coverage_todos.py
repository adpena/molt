import re
from pathlib import Path


MATRICES = {
    "type-coverage": Path(
        "docs/spec/areas/compat/surfaces/language/type_coverage_matrix.md"
    ),
    "stdlib-compat": Path(
        "docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md"
    ),
}
TODO_START = re.compile(r"TODO\((type-coverage|stdlib-compat)\b")
FIELD = re.compile(r"([a-z]+):([A-Za-z0-9_./+-]+)")
FIELDS = ("owner", "milestone", "priority", "status")


def _extract_todos(text: str) -> set[str]:
    """Read complete gap records, preserving descriptions as part of identity."""
    records = set()
    for line_number, line in enumerate(text.splitlines(), 1):
        for match in TODO_START.finditer(line):
            end = line.find(")", match.end())
            if end < 0:
                raise ValueError(f"line {line_number}: unterminated TODO header")
            # The matrices document their marker syntax in inline code.
            if (
                match.start() > 0
                and line[match.start() - 1] == "`"
                and line[end + 1 : end + 2] == "`"
                and line[match.start() : end + 1] == f"TODO({match[1]}, ...)"
            ):
                continue
            header = line[match.start() + len("TODO(") : end].split(",")
            category = header[0].strip()
            fields = {}
            for part in header[1:]:
                field = FIELD.fullmatch(part.strip())
                if field is None or field[1] not in FIELDS or field[1] in fields:
                    raise ValueError(f"line {line_number}: invalid TODO field {part!r}")
                fields[field[1]] = field[2]
            if category not in MATRICES or set(fields) != set(FIELDS):
                raise ValueError(f"line {line_number}: incomplete TODO header")
            if fields["priority"] not in {"P0", "P1", "P2", "P3"}:
                raise ValueError(f"line {line_number}: invalid TODO priority")
            tail = line[end + 1 :].strip()
            if not tail.startswith(":"):
                raise ValueError(f"line {line_number}: missing TODO description")
            description = tail[1:].strip()
            if description.endswith("|"):
                description = description[:-1].rstrip()
            if match.start() > 0 and line[match.start() - 1] == "(":
                wrapper = re.fullmatch(r"(.*)\)(?:\.)?", description)
                if wrapper is None:
                    raise ValueError(f"line {line_number}: unclosed TODO wrapper")
                description = wrapper[1].rstrip()
            if not description:
                raise ValueError(f"line {line_number}: empty TODO description")
            normalized = ", ".join(
                [category, *(f"{key}:{fields[key]}" for key in FIELDS)]
            )
            records.add(f"TODO({normalized}): {description}")
    return records


def main(root: Path | None = None) -> int:
    root = Path(__file__).resolve().parents[1] if root is None else root
    try:
        canonical = set()
        for category, path in MATRICES.items():
            records = _extract_todos((root / path).read_text(encoding="utf-8"))
            if not any(record.startswith(f"TODO({category},") for record in records):
                raise ValueError(f"{path.as_posix()}: no complete {category} records")
            canonical.update(records)
        roadmap = (root / "ROADMAP.md").read_text(encoding="utf-8")
        links = {
            target.removeprefix("./").split("#", 1)[0]
            for target in re.findall(r"\[[^\]\r\n]+\]\(([^)\s]+)\)", roadmap)
        }
        for path in MATRICES.values():
            if path.as_posix() not in links:
                raise ValueError(
                    f"ROADMAP.md: missing canonical matrix link {path.as_posix()}"
                )
        stale = _extract_todos(roadmap) - canonical
        if stale:
            raise ValueError(
                "ROADMAP.md: records differ from canonical matrices:\n  - "
                + "\n  - ".join(sorted(stale))
            )
    except (OSError, UnicodeError, ValueError) as error:
        print(f"Type/stdlib gap authority check failed: {error}")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
