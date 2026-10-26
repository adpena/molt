#!/usr/bin/env python3
"""Generate the native launch ABI for the AOT main stub and the JIT runner.

``runtime/native_launch_abi.toml`` declares the runtime entry points a native
program's launch calls and their order, and the JIT runner's command line and
image format. The AOT main stub (``src/molt/cli/native_main_stub.py``)
declares the entries in C from the Python projection; the development JIT
runner (``runtime/molt-jit``) resolves them from the Rust projection. The CLI
(``run_tier.py``, ``jit_image.py``) and the runner read the command line and
image format from the same two projections. None spells them by hand.

Usage::

    python tools/gen_native_launch_abi.py --write
    python tools/gen_native_launch_abi.py --check
"""

from __future__ import annotations

import re
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tools.generator_io import (  # noqa: E402
    generator_main,
    ruff_format_source,
    rustfmt_source,
)

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "runtime" / "native_launch_abi.toml"
OUT_PYTHON = ROOT / "src" / "molt" / "cli" / "native_launch_abi_generated.py"
OUT_RUST = ROOT / "runtime" / "molt-jit" / "src" / "launch_abi_generated.rs"

_NAME = re.compile(r"^molt_[a-z0-9_]+$")

# The closed set of launch ABI types: (C spelling, Rust spelling).
_TYPES: dict[str, tuple[str, str]] = {
    "u64": ("unsigned long long", "u64"),
    "i32": ("int", "i32"),
    "argv": ("const char**", "*const *const std::ffi::c_char"),
    "c_string_array": ("const char* const*", "*const *const std::ffi::c_char"),
    "bytes": ("const unsigned char*", "*const u8"),
}
_RETURNS: dict[str, tuple[str, str]] = {
    "void": ("void", ""),
    "u64": ("unsigned long long", " -> u64"),
}
_PHASES = ("start", "finish")


_RUNNER_KEYS = (
    "image_flag",
    "probe_flag",
    "start_receipt_flag",
    "allocator_restart_flag",
    "start_receipt",
    "image_marker",
)
_FLAG = re.compile(r"^--[a-z][a-z0-9-]*$")


@dataclass(frozen=True)
class RunnerProtocol:
    """How the JIT runner is started, and the format of its image."""

    image_flag: str
    probe_flag: str
    start_receipt_flag: str
    allocator_restart_flag: str
    start_receipt: str
    image_marker: str


def load_runner(path: Path = SOURCE) -> RunnerProtocol:
    with path.open("rb") as handle:
        table = tomllib.load(handle).get("runner")
    if not isinstance(table, dict) or set(table) != set(_RUNNER_KEYS):
        raise ValueError(f"{path}: [runner] needs exactly {', '.join(_RUNNER_KEYS)}")
    if not all(isinstance(table[key], str) for key in _RUNNER_KEYS):
        raise ValueError(f"{path}: [runner] values must be strings")
    flags = [table[key] for key in _RUNNER_KEYS if key.endswith("_flag")]
    if not all(_FLAG.match(flag) for flag in flags) or len(set(flags)) != len(flags):
        raise ValueError(f"{path}: [runner] flags must be distinct --lowercase words")
    if not table["image_marker"].startswith("#") or any(
        character.isspace() for character in table["image_marker"]
    ):
        raise ValueError(f"{path}: the image marker must be one #word")
    if not table["start_receipt"].endswith("\n") or "\n" in table["start_receipt"][:-1]:
        raise ValueError(f"{path}: the start receipt must be one line")
    return RunnerProtocol(**{key: table[key] for key in _RUNNER_KEYS})


@dataclass(frozen=True)
class LaunchEntry:
    name: str
    phase: str
    returns: str
    params: tuple[str, ...]


def load_entries(path: Path = SOURCE) -> tuple[LaunchEntry, ...]:
    with path.open("rb") as handle:
        document = tomllib.load(handle)
    if document.get("schema") != 1:
        raise ValueError(f"{path}: schema must be 1")
    unknown = set(document) - {"schema", "runner", *_PHASES}
    if unknown:
        raise ValueError(f"{path}: unknown keys {sorted(unknown)}")
    entries: list[LaunchEntry] = []
    for phase in _PHASES:
        for row in document.get(phase, ()):
            if set(row) != {"name", "returns", "params"}:
                raise ValueError(f"{path}: {phase} entry needs name, returns, params")
            name, returns, params = row["name"], row["returns"], tuple(row["params"])
            if not _NAME.match(name):
                raise ValueError(f"{path}: {name!r} is not a molt_ runtime symbol")
            if returns not in _RETURNS:
                raise ValueError(f"{path}: {name} returns unknown type {returns!r}")
            for param in params:
                if param not in _TYPES:
                    raise ValueError(f"{path}: {name} takes unknown type {param!r}")
            entries.append(LaunchEntry(name, phase, returns, params))
    names = [entry.name for entry in entries]
    if len(set(names)) != len(names):
        raise ValueError(f"{path}: a launch entry is declared twice")
    return tuple(entries)


def _c_declaration(entry: LaunchEntry) -> str:
    params = ", ".join(_TYPES[param][0] for param in entry.params) or "void"
    return f"extern {_RETURNS[entry.returns][0]} {entry.name}({params});"


def _rust_signature(entry: LaunchEntry) -> str:
    params = ", ".join(_TYPES[param][1] for param in entry.params)
    return f'unsafe extern "C" fn({params}){_RETURNS[entry.returns][1]}'


def _names(entries: tuple[LaunchEntry, ...], phase: str) -> list[str]:
    return [entry.name for entry in entries if entry.phase == phase]


def render_python(entries: tuple[LaunchEntry, ...], runner: RunnerProtocol) -> str:
    declarations = "\n".join(_c_declaration(entry) for entry in entries)
    return f'''"""The native launch ABI (generated; do not edit).

Generated by ``tools/gen_native_launch_abi.py`` from
``runtime/native_launch_abi.toml``. The AOT main stub declares these runtime
entry points and calls them in this order; the JIT runner resolves the same
entries from ``runtime/molt-jit/src/launch_abi_generated.rs``, and reads the
same command line and image format from it.
"""

# The JIT runner's command line: [<receipt flag> <path>] <image flag> <image> [args...].
JIT_RUNNER_IMAGE_FLAG = {runner.image_flag!r}
# Alone, this flag runs the runner's executable-memory admission probe.
JIT_RUNNER_PROBE_FLAG = {runner.probe_flag!r}
JIT_RUNNER_START_RECEIPT_FLAG = {runner.start_receipt_flag!r}
# What the runner writes to its start receipt right before the program starts.
JIT_START_RECEIPT = {runner.start_receipt!r}
# The second line of a JIT image starts with this marker.
JIT_IMAGE_MARKER = {runner.image_marker!r}

# Called in this order before the program's entry.
NATIVE_LAUNCH_START_SEQUENCE = {tuple(_names(entries, "start"))!r}
# Called after the entry returns: report a pending exception, then exit.
NATIVE_LAUNCH_FINISH_ENTRIES = {tuple(_names(entries, "finish"))!r}
# The C declarations of every launch entry.
NATIVE_LAUNCH_C_DECLARATIONS = {declarations!r}
'''


def _rust_str(value: str) -> str:
    return (
        '"' + value.encode("unicode_escape").decode("ascii").replace('"', '\\"') + '"'
    )


def render_rust(entries: tuple[LaunchEntry, ...], runner: RunnerProtocol) -> str:
    fields = "\n".join(
        f"    pub(crate) {entry.name}: {_rust_signature(entry)}," for entry in entries
    )
    resolved = "\n".join(
        f"            {entry.name}: unsafe {{ std::mem::transmute::<*const u8, "
        f'{_rust_signature(entry)}>(entry("{entry.name}")?) }},'
        for entry in entries
    )
    return f"""// @generated by tools/gen_native_launch_abi.py from
// runtime/native_launch_abi.toml. Do not edit.

/// The flag that names the image; everything after the image is the
/// program's. A JIT image's first line is `#!<runner> ` followed by it.
pub(crate) const IMAGE_FLAG: &str = {_rust_str(runner.image_flag)};
/// Alone, this flag runs the executable-memory admission probe.
pub(crate) const PROBE_FLAG: &str = {_rust_str(runner.probe_flag)};
/// The flag whose value is the start receipt's path.
pub(crate) const START_RECEIPT_FLAG: &str = {_rust_str(runner.start_receipt_flag)};
/// The runner's own flag for its one allocator restart (macOS).
#[cfg(target_os = "macos")]
pub(crate) const ALLOCATOR_RESTART_FLAG: &str = {_rust_str(runner.allocator_restart_flag)};
/// What the runner writes to its start receipt right before the program starts.
pub(crate) const START_RECEIPT: &str = {_rust_str(runner.start_receipt)};
/// The second line of a JIT image starts with this marker.
pub(crate) const IMAGE_MARKER: &str = {_rust_str(runner.image_marker)};

/// The runtime entry points a native program's launch calls, in the order
/// `runtime/native_launch_abi.toml` declares, with the C signatures the AOT
/// main stub declares.
pub(crate) struct RuntimeLaunchApi {{
{fields}
}}

impl RuntimeLaunchApi {{
    /// Resolve every launch entry with `lookup`, naming the first one missing.
    ///
    /// # Safety
    /// `lookup` must return the address of a function with the declared C
    /// signature for every name it resolves.
    pub(crate) unsafe fn resolve(
        lookup: impl Fn(&str) -> Option<*const u8>,
    ) -> Result<Self, String> {{
        let entry = |name: &str| {{
            lookup(name).ok_or_else(|| format!("the runtime image defines no `{{name}}`"))
        }};
        // SAFETY: the caller guarantees each address has its declared signature.
        Ok(Self {{
{resolved}
        }})
    }}
}}
"""


def render_all(
    entries: tuple[LaunchEntry, ...], runner: RunnerProtocol
) -> dict[Path, str]:
    return {
        OUT_PYTHON: ruff_format_source(render_python(entries, runner), path=OUT_PYTHON),
        OUT_RUST: rustfmt_source(
            render_rust(entries, runner), label="native launch ABI Rust"
        ),
    }


def generated_outputs() -> dict[Path, str]:
    """Each output path mapped to its exact generated text."""
    return render_all(load_entries(), load_runner())


def main(argv: list[str] | None = None) -> int:
    return generator_main(generated_outputs, argv, description=__doc__)


if __name__ == "__main__":
    raise SystemExit(main())
