"""Executable custody fixtures; tool behavior belongs to explicit subprocess mocks."""

from __future__ import annotations

import os
from pathlib import Path
import stat
import sys
from collections.abc import Callable, Mapping


_DONOR_FACTORY: Callable[[], Path] | None = None
_DONOR: Path | None = None
_BUILD_ENVIRONMENT: Mapping[str, str] | None = None


def bind_native_donor_factory(
    factory: Callable[[], Path] | None, *, environment: Mapping[str, str] | None
) -> None:
    """Bind the pytest session's lazy directory owner and build environment, or
    release both at teardown.

    Fixture executables are session apparatus, so they build in the environment
    the session started with, never in the environment of the test that first
    asks: a test that points HOME elsewhere hides rustup's toolchains from the
    rustc proxy, which then fails or installs a toolchain.
    """
    global _DONOR_FACTORY, _DONOR, _BUILD_ENVIRONMENT
    _DONOR_FACTORY, _DONOR = factory, None
    _BUILD_ENVIRONMENT = None if environment is None else dict(environment)


def native_executable_donor() -> Path:
    """One small real native image, built only if this test session needs it.

    Runtime fixture identities repeatedly hash each independent image. Copying
    the macOS interpreter made each image 18 MB; system binaries carry a
    restricted flag a copy cannot retain. Windows's interpreter launcher is
    already small and requires no compilation.
    """
    global _DONOR
    if os.name == "nt":
        return Path(sys.executable).resolve(strict=True)
    if _DONOR is None:
        if _DONOR_FACTORY is None:
            raise RuntimeError("tests/conftest.py must bind the native donor directory")
        _DONOR = build_native_executable(_DONOR_FACTORY() / "donor", "fn main() {}\n")
    return _DONOR


def write_mock_executable(path: Path, content: bytes) -> Path:
    """Write mocked tool bytes with the host's executable permission contract."""
    path.write_bytes(content)
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    return path


def native_executable_name(stem: str) -> str:
    """Return the host spelling of an executable file name."""
    return f"{stem}.exe" if os.name == "nt" else stem


def custody_spelling(path: Path) -> str:
    """State proof custody's canonical spelling of an entry the test created.

    Windows custody spells the drive or UNC anchor in lowercase and keeps every
    other component as its directory entry spells it. A test creates each of
    those components itself, so only the anchor changes. POSIX custody is the
    absolute lexical path. Assert custody records against this statement, not
    against the product's own lookup.
    """
    absolute = os.fspath(path.absolute())
    if os.name != "nt":
        return absolute
    anchor, rest = os.path.splitdrive(absolute)
    return anchor.lower() + rest


def build_native_executable(path: Path, rust_source: str) -> Path:
    """Compile a tiny native executable from Rust source with the pinned rustc.

    Tool identity admits only native executables (ELF, Mach-O, PE). A test that
    needs a fake tool with real process behavior, such as answering differently
    by `argv[0]`, compiles one instead of writing a script.
    """
    from tests.process_guard_common import run_guarded_test_process

    source = path.with_name(f"{path.name}.rs")
    source.write_text(rust_source, encoding="utf-8")
    if _BUILD_ENVIRONMENT is None:
        raise RuntimeError("tests/conftest.py must bind the native build environment")
    run_guarded_test_process(
        ["rustc", "--edition=2024", "-C", "opt-level=0", "-o", str(path), str(source)],
        cwd=path.parent,
        env=_BUILD_ENVIRONMENT,
        check=True,
    )
    return path
