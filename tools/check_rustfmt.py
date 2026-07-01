#!/usr/bin/env python3
"""Repo-owned Rust formatting gate.

This deliberately avoids raw ``cargo fmt`` as a broad workspace authority:
human Rust source is checked with rustfmt, while checked-in generated Rust is
left to its generator sync gate. Write mode formats through stdout and rewrites
only files whose normalized contents really changed, which prevents Windows
line-ending-only churn from cooling later Cargo proof lanes.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
GENERATED_HEAD_LINES = 12
WINDOWS_COMMAND_LINE_SOFT_LIMIT = 24_000
RUSTFMT_TIMEOUT_S = 60.0
THIRD_PARTY_PREFIXES = ("vendor/",)
RUSTFMT_STDOUT_HEADER_RE = re.compile(
    rb"(?m)^(?P<path>(?:[A-Za-z]:|\\\\\?\\|/).+?\.rs):\r?\n\r?\n"
)


@dataclass(frozen=True)
class RustfmtSelection:
    human: tuple[Path, ...]
    generated: tuple[Path, ...]
    third_party: tuple[Path, ...]


def _run_git(args: list[str]) -> bytes:
    proc = subprocess.run(
        ["git", "-C", str(ROOT), *args],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if proc.returncode != 0:
        raise RuntimeError(proc.stderr.decode("utf-8", errors="replace").strip())
    return proc.stdout


def _decode_z_paths(payload: bytes) -> tuple[Path, ...]:
    paths: list[Path] = []
    for raw in payload.split(b"\0"):
        if not raw:
            continue
        text = raw.decode("utf-8", errors="surrogateescape")
        paths.append(Path(text))
    return tuple(paths)


def _merge_base(ref: str) -> str | None:
    proc = subprocess.run(
        ["git", "-C", str(ROOT), "merge-base", ref, "HEAD"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if proc.returncode != 0:
        return None
    base = proc.stdout.decode("ascii", errors="ignore").strip()
    return base or None


def _upstream_ref() -> str | None:
    proc = subprocess.run(
        [
            "git",
            "-C",
            str(ROOT),
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if proc.returncode != 0:
        return None
    ref = proc.stdout.decode("utf-8", errors="replace").strip()
    return ref or None


def _changed_base() -> str | None:
    candidates: list[str] = []
    upstream = _upstream_ref()
    if upstream is not None:
        candidates.append(upstream)
    candidates.extend(["origin/main", "main"])
    seen: set[str] = set()
    for candidate in candidates:
        if candidate in seen:
            continue
        seen.add(candidate)
        base = _merge_base(candidate)
        if base is not None:
            return base
    return None


def _tracked_rust_paths() -> tuple[Path, ...]:
    return _decode_z_paths(_run_git(["ls-files", "-z", "--", "*.rs"]))


def _explicit_rust_paths(raw_paths: list[str]) -> tuple[Path, ...]:
    paths: set[Path] = set()
    for raw_path in raw_paths:
        path = Path(raw_path)
        full_path = path if path.is_absolute() else ROOT / path
        if not full_path.exists():
            raise RuntimeError(f"rustfmt path does not exist: {raw_path}")
        if full_path.is_file():
            if full_path.suffix == ".rs":
                paths.add(full_path.resolve().relative_to(ROOT.resolve()))
            continue
        for child in full_path.rglob("*.rs"):
            paths.add(child.resolve().relative_to(ROOT.resolve()))
    return tuple(sorted(paths, key=lambda path: path.as_posix()))


def _changed_rust_paths() -> tuple[Path, ...]:
    paths: set[Path] = set()
    for args in (
        ["diff", "--name-only", "-z", "--", "*.rs"],
        ["diff", "--cached", "--name-only", "-z", "--", "*.rs"],
        ["ls-files", "--others", "--exclude-standard", "-z", "--", "*.rs"],
    ):
        paths.update(_decode_z_paths(_run_git(args)))
    base = _changed_base()
    if base is not None:
        paths.update(
            _decode_z_paths(
                _run_git(
                    [
                        "diff",
                        "--name-only",
                        "-z",
                        "--diff-filter=ACMRTUXB",
                        f"{base}...HEAD",
                        "--",
                        "*.rs",
                    ]
                )
            )
        )
    return tuple(sorted(paths, key=lambda path: path.as_posix()))


def _head_text(path: Path) -> str:
    try:
        with path.open("r", encoding="utf-8") as handle:
            return "".join(line for _, line in zip(range(GENERATED_HEAD_LINES), handle))
    except UnicodeDecodeError:
        return ""


def is_generated_rust(path: Path) -> bool:
    head = _head_text(path)
    lowered = head.lower()
    return "@generated" in lowered or "do not edit" in lowered


def is_third_party_rust(path: Path) -> bool:
    rel = path if not path.is_absolute() else path.resolve().relative_to(ROOT.resolve())
    return rel.as_posix().startswith(THIRD_PARTY_PREFIXES)


def generated_owner(path: Path) -> str:
    match = re.search(r"@generated by ([^.\s]+\.py)", _head_text(path))
    if match:
        return f"python {match.group(1)} --check"
    return "the owning generator --check gate"


def select_rust_paths(paths: tuple[Path, ...]) -> RustfmtSelection:
    human: list[Path] = []
    generated: list[Path] = []
    third_party: list[Path] = []
    for rel_path in paths:
        full_path = ROOT / rel_path
        if not full_path.exists():
            continue
        if is_third_party_rust(rel_path):
            third_party.append(rel_path)
        elif is_generated_rust(full_path):
            generated.append(rel_path)
        else:
            human.append(rel_path)
    return RustfmtSelection(tuple(human), tuple(generated), tuple(third_party))


def _chunked_rustfmt_commands(
    rustfmt: str,
    paths: tuple[Path, ...],
    *,
    check: bool,
    edition: str,
) -> list[list[str]]:
    prefix = [rustfmt, "--edition", edition]
    if check:
        prefix.append("--check")
    commands: list[list[str]] = []
    current = list(prefix)
    current_len = sum(len(part) + 1 for part in current)
    for path in paths:
        text = str(ROOT / path)
        next_len = current_len + len(text) + 1
        if len(current) > len(prefix) and next_len > WINDOWS_COMMAND_LINE_SOFT_LIMIT:
            commands.append(current)
            current = list(prefix)
            current_len = sum(len(part) + 1 for part in current)
        current.append(text)
        current_len += len(text) + 1
    if len(current) > len(prefix):
        commands.append(current)
    return commands


def _nearest_cargo_toml(path: Path) -> Path | None:
    current = (ROOT / path).resolve().parent
    root = ROOT.resolve()
    while True:
        cargo_toml = current / "Cargo.toml"
        if cargo_toml.exists():
            return cargo_toml
        if current == root or current.parent == current:
            return None
        current = current.parent


def _edition_for_path(
    path: Path, default_edition: str, cache: dict[Path | None, str]
) -> str:
    cargo_toml = _nearest_cargo_toml(path)
    if cargo_toml in cache:
        return cache[cargo_toml]
    edition = default_edition
    if cargo_toml is not None:
        try:
            data = tomllib.loads(cargo_toml.read_text(encoding="utf-8"))
        except (OSError, tomllib.TOMLDecodeError):
            data = {}
        package = data.get("package")
        if isinstance(package, dict):
            raw = package.get("edition")
            if isinstance(raw, str) and raw:
                edition = raw
    cache[cargo_toml] = edition
    return edition


def _paths_by_edition(
    paths: tuple[Path, ...], default_edition: str
) -> dict[str, tuple[Path, ...]]:
    cache: dict[Path | None, str] = {}
    grouped: dict[str, list[Path]] = {}
    for path in paths:
        edition = _edition_for_path(path, default_edition, cache)
        grouped.setdefault(edition, []).append(path)
    return {edition: tuple(edition_paths) for edition, edition_paths in grouped.items()}


def _normalized_newlines(data: bytes) -> str:
    return data.decode("utf-8", errors="surrogateescape").replace("\r\n", "\n")


def _canonical_rustfmt_path_text(path: Path) -> str:
    text = str(path)
    if text.startswith("\\\\?\\"):
        text = text[4:]
    return text.replace("/", "\\").lower()


def _select_rustfmt_stdout_section(payload: bytes, path: Path) -> bytes:
    matches = list(RUSTFMT_STDOUT_HEADER_RE.finditer(payload))
    if not matches:
        return payload

    requested = _canonical_rustfmt_path_text((ROOT / path).resolve())
    for index, match in enumerate(matches):
        header_path = match.group("path").decode("utf-8", errors="surrogateescape")
        normalized_header = _canonical_rustfmt_path_text(Path(header_path))
        start = match.end()
        end = matches[index + 1].start() if index + 1 < len(matches) else len(payload)
        if normalized_header == requested:
            return payload[start:end].rstrip(b"\r\n") + b"\n"

    raise RuntimeError(f"rustfmt did not emit a stdout section for {path.as_posix()}")


def _rustfmt_stdout(path: Path, edition: str) -> bytes:
    proc = subprocess.run(
        [
            "rustfmt",
            "--edition",
            edition,
            "--emit",
            "stdout",
            str(ROOT / path),
        ],
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
        timeout=RUSTFMT_TIMEOUT_S,
    )
    if proc.returncode != 0:
        stderr = proc.stderr.decode("utf-8", errors="replace").strip()
        raise RuntimeError(stderr or f"rustfmt failed for {path.as_posix()}")
    selected = _select_rustfmt_stdout_section(proc.stdout, path)
    return selected.rstrip(b"\r\n") + b"\n"


def _run_rustfmt_write(paths: tuple[Path, ...], *, edition: str) -> int:
    formatted = 0
    for path in paths:
        full_path = ROOT / path
        rustfmt_output = _rustfmt_stdout(path, _edition_for_path(path, edition, {}))
        current = full_path.read_bytes()
        if _normalized_newlines(current) == _normalized_newlines(rustfmt_output):
            continue
        full_path.write_bytes(rustfmt_output)
        formatted += 1
    print(
        f"rustfmt: wrote {formatted} file(s); {len(paths) - formatted} already stable"
    )
    return 0


def _run_rustfmt(
    paths: tuple[Path, ...],
    *,
    check: bool,
    edition: str,
) -> int:
    if not check:
        return _run_rustfmt_write(paths, edition=edition)
    rustfmt = "rustfmt"
    for resolved_edition, edition_paths in _paths_by_edition(paths, edition).items():
        commands = _chunked_rustfmt_commands(
            rustfmt,
            edition_paths,
            check=True,
            edition=resolved_edition,
        )
        for cmd in commands:
            proc = subprocess.run(cmd, cwd=ROOT, check=False)
            if proc.returncode != 0:
                return proc.returncode
    return 0


def _print_generated_notice(paths: tuple[Path, ...], *, verbose: bool) -> None:
    if not paths:
        return
    if verbose:
        for path in paths:
            print(
                f"rustfmt: generated Rust skipped: {path.as_posix()} "
                f"(owner: {generated_owner(ROOT / path)})"
            )
        return
    owners = sorted({generated_owner(ROOT / path) for path in paths})
    print(
        "rustfmt: skipped "
        f"{len(paths)} generated Rust file(s); generator authority: "
        + "; ".join(owners)
    )


def _print_third_party_notice(paths: tuple[Path, ...], *, verbose: bool) -> None:
    if not paths:
        return
    if verbose:
        for path in paths:
            print(f"rustfmt: third-party Rust skipped: {path.as_posix()}")
        return
    print(f"rustfmt: skipped {len(paths)} third-party Rust file(s)")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument(
        "--all",
        action="store_true",
        help="check all tracked Rust source files",
    )
    mode.add_argument(
        "--changed",
        action="store_true",
        help="check changed, staged, and untracked Rust source files",
    )
    parser.add_argument(
        "paths",
        nargs="*",
        help="optional Rust files or directories; overrides the changed-file default",
    )
    parser.add_argument(
        "--write",
        action="store_true",
        help="format selected human Rust files in place",
    )
    parser.add_argument("--edition", default="2024")
    parser.add_argument("--verbose", action="store_true")
    args = parser.parse_args(argv)

    if args.paths and (args.all or args.changed):
        parser.error("pass explicit paths, --all, or --changed; combine none of them")
    if args.paths:
        paths = _explicit_rust_paths(args.paths)
    else:
        paths = _tracked_rust_paths() if args.all else _changed_rust_paths()
    selection = select_rust_paths(paths)
    _print_third_party_notice(selection.third_party, verbose=args.verbose)
    _print_generated_notice(selection.generated, verbose=args.verbose)
    if not selection.human:
        print("rustfmt: no human Rust files selected")
        return 0

    action = "formatting" if args.write else "checking"
    print(f"rustfmt: {action} {len(selection.human)} human Rust file(s)")
    return _run_rustfmt(
        selection.human,
        check=not args.write,
        edition=args.edition,
    )


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except RuntimeError as exc:
        print(f"rustfmt gate failed: {exc}", file=sys.stderr)
        raise SystemExit(2) from exc
