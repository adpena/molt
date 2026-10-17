"""What one Cargo build read, from that build's own per-unit outputs (HF-300).

Cargo reports each unit it built or found fresh in its ``compiler-artifact``
JSON messages. Beside each local unit's outputs, rustc wrote a dep-info file:
every file it read for that crate (``include_str!``, ``#[path]`` modules and
the rest) and every environment variable it read. Each build script's
``output`` file, beside the ``out_dir`` that ``build-script-executed`` reports,
holds the paths and variables it declared with ``rerun-if-changed`` and
``rerun-if-env-changed``. A script that declares neither reruns when any file
of its package changes, so its whole package counts. This is the input set
Cargo's own fingerprints use; docs/design/cargo_input_authority.md holds the
rules and their prior art.

The uplifted dep-info beside a final artifact is not enough: it leaves out
local packages that only registry crates reach (``vendor/rust/zlib-rs``) and
every environment input.
"""

from __future__ import annotations

from collections.abc import Iterable, Iterator, Mapping, Sequence
from dataclasses import dataclass
import json
import os
from pathlib import Path
import stat
from urllib.parse import urlparse
from urllib.request import url2pathname

# The largest dep-info or build-script output Molt reads; the runtime's
# largest unit dep-info is 76 KB.
_MAX_BYTES = 16 * 1024 * 1024
_ENV_DEP = "# env-dep:"
_RERUN_IF_CHANGED = ("cargo:rerun-if-changed=", "cargo::rerun-if-changed=")
_RERUN_IF_ENV_CHANGED = ("cargo:rerun-if-env-changed=", "cargo::rerun-if-env-changed=")
# Output suffixes rustc puts after a unit's ``<name>-<metadata>`` stem.
_OUTPUT_SUFFIXES = frozenset(
    {".rlib", ".rmeta", ".a", ".so", ".dylib", ".dll", ".lib", ".wasm", ".exe", ".pdb"}
)
_LIBRARY_KINDS = frozenset(
    {"lib", "rlib", "staticlib", "cdylib", "dylib", "proc-macro"}
)


class CargoBuildReadsError(ValueError):
    """A Cargo build's reads cannot be established from its outputs."""


@dataclass(frozen=True, slots=True)
class CargoBuildReads:
    """The project files and environment one Cargo build read."""

    # Project files, by real path, sorted.
    files: tuple[Path, ...]
    # Directories a build script named with rerun-if-changed: Cargo watches
    # their whole listing, not only the files present now.
    directories: tuple[Path, ...]
    # Variables rustc or a build script read, with rustc's recorded value
    # (None when only a build script declared the name).
    environment: tuple[tuple[str, str | None], ...]


def cargo_build_reads(
    cargo_stdout: str,
    *,
    workspace_root: Path,
    product_roots: Sequence[Path],
    toolchain_roots: Sequence[Path],
) -> CargoBuildReads:
    """Collect what the Cargo build that printed ``cargo_stdout`` read.

    Files under ``product_roots`` are outputs of build scripts, whose inputs
    are collected; files under ``toolchain_roots`` belong to the plan
    identity. Any other file outside ``workspace_root`` means the build read
    another workspace's sources, so it is refused.
    """
    root = Path(os.path.realpath(workspace_root))
    skipped = tuple(Path(os.path.realpath(path)) for path in product_roots)
    covered = tuple(Path(os.path.realpath(path)) for path in toolchain_roots)
    files: set[Path] = set()
    directories: set[Path] = set()
    environment: dict[str, str | None] = {}

    def admit(path: Path) -> None:
        resolved = Path(os.path.realpath(path))
        if _under_any(resolved, skipped) or _under_any(resolved, covered):
            return
        if not resolved.is_relative_to(root):
            raise CargoBuildReadsError(
                f"the Cargo build read a file outside {root}: {resolved}"
            )
        try:
            metadata = resolved.stat()
        except OSError as exc:
            raise CargoBuildReadsError(
                f"a Cargo build input is missing: {resolved}"
            ) from exc
        if stat.S_ISDIR(metadata.st_mode):
            directories.add(resolved)
            files.update(_files_under(resolved))
        elif stat.S_ISREG(metadata.st_mode):
            files.add(resolved)
        else:
            raise CargoBuildReadsError(f"a Cargo build input is not a file: {resolved}")

    messages = tuple(_messages(cargo_stdout))
    # Where this build put its units, and each package's build-script out_dir:
    # together they name the dep-info of a unit Cargo reports only uplifted.
    deps_dirs = {
        Path(filename).parent
        for message in messages
        if message.get("reason") == "compiler-artifact"
        for filename in _strings(message.get("filenames"))
        if Path(filename).parent.name == "deps"
    }
    out_dirs = {
        message.get("package_id"): message.get("out_dir")
        for message in messages
        if message.get("reason") == "build-script-executed"
    }
    units = 0
    for message in messages:
        reason = message.get("reason")
        package = _local_package_root(message.get("package_id"))
        if reason == "compiler-artifact":
            dep_info = _unit_dep_info(
                message, deps_dirs, out_dirs.get(message.get("package_id"))
            )
            if package is None:
                # A registry or git unit: Cargo.lock identifies its sources,
                # but the variables it read still decide its bytes.
                if dep_info is not None:
                    _, unit_environment = _read_dep_info(dep_info)
                    environment.update(unit_environment)
                continue
            if dep_info is None:
                raise CargoBuildReadsError(
                    f"Cargo reported no dep-info for {message.get('package_id')}"
                )
            entries, unit_environment = _read_dep_info(dep_info)
            environment.update(unit_environment)
            for entry in entries:
                path = Path(entry)
                admit(path if path.is_absolute() else root / path)
            units += 1
        elif reason == "build-script-executed":
            out_dir = message.get("out_dir")
            if not isinstance(out_dir, str):
                raise CargoBuildReadsError("a build-script message has no out_dir")
            changed, variables = _read_build_script_output(
                Path(out_dir).parent / "output"
            )
            for name in variables:
                environment.setdefault(name, None)
            if package is None:
                continue
            for entry in changed:
                path = Path(entry)
                admit(path if path.is_absolute() else package / path)
            if not changed and not variables:
                files.update(_package_files(package))
    if units == 0:
        raise CargoBuildReadsError("the Cargo build reported no local unit")
    return CargoBuildReads(
        tuple(sorted(files)),
        tuple(sorted(directories)),
        tuple(sorted(environment.items())),
    )


def paths_outside(
    paths: Iterable[Path], workspace_root: Path, roots: Iterable[Path]
) -> tuple[str, ...]:
    """The workspace-relative paths that no root covers."""
    root = Path(os.path.realpath(workspace_root))
    covering = tuple(Path(os.path.realpath(path)) for path in roots)
    return tuple(
        path.relative_to(root).as_posix()
        for path in paths
        if not _under_any(path, covering)
    )


def _messages(cargo_stdout: str) -> Iterator[Mapping[str, object]]:
    for line in cargo_stdout.splitlines():
        if not line.startswith("{"):
            continue
        try:
            message = json.loads(line)
        except ValueError:
            continue
        if isinstance(message, dict):
            yield message


def _unit_dep_info(
    message: Mapping[str, object], deps_dirs: Iterable[Path], out_dir: object
) -> Path | None:
    """The rustc dep-info of one unit: ``<crate>-<metadata>.d``.

    A library's outputs in ``deps`` carry the ``<crate>-<metadata>`` stem, with
    the dep-info beside them. A build script compiles in
    ``build/<package>-<metadata>/`` as ``build_script_build-<metadata>``, while
    Cargo reports its uplifted copy ``build-script-build``. A root artifact
    Cargo reports only uplifted (a staticlib or binary in the profile
    directory) has several ``<crate>-<metadata>.d`` candidates, one per feature
    and profile lane; the one this build used records this build's
    ``OUT_DIR``.
    """
    filenames = _strings(message.get("filenames"))
    target = message.get("target")
    if not filenames or not isinstance(target, Mapping):
        return None
    kinds = [kind for kind in _strings(target.get("kind")) if isinstance(kind, str)]
    library = any(kind in _LIBRARY_KINDS for kind in kinds)
    name = target.get("name")
    crate = name.replace("-", "_") if isinstance(name, str) else None
    for filename in filenames:
        output = Path(filename)
        if "custom-build" in kinds and crate is not None:
            metadata = output.parent.name.rpartition("-")[2]
            candidate = output.with_name(f"{crate}-{metadata}.d")
        else:
            stem = output.stem if output.suffix in _OUTPUT_SUFFIXES else output.name
            if library and stem.startswith("lib"):
                stem = stem[3:]
            candidate = output.with_name(stem + ".d")
        if output.parent.name in {"deps", "build"} or "custom-build" in kinds:
            if candidate.is_file():
                return candidate
    if crate is None:
        return None
    matches = [
        candidate
        for directory in deps_dirs
        for candidate in directory.glob(f"{crate}-*.d")
        if _records_out_dir(candidate, out_dir)
    ]
    if len(matches) > 1:
        raise CargoBuildReadsError(
            f"several dep-info files match {crate}: "
            + ", ".join(os.fspath(path) for path in sorted(matches))
        )
    return matches[0] if matches else None


def _records_out_dir(dep_info: Path, out_dir: object) -> bool:
    # A unit without a build script has no OUT_DIR; it is the lane's only
    # candidate only if Cargo kept one, which the caller cannot prove.
    if not isinstance(out_dir, str):
        return False
    expected = _ENV_DEP + "OUT_DIR=" + out_dir
    try:
        text = _read_bounded(dep_info)
    except CargoBuildReadsError:
        return False
    return any(line == expected for line in text.splitlines())


def _strings(value: object) -> list[str]:
    return (
        [item for item in value if isinstance(item, str)]
        if isinstance(value, list)
        else []
    )


def _read_bounded(path: Path) -> str:
    try:
        with path.open("rb") as handle:
            raw = handle.read(_MAX_BYTES + 1)
    except OSError as exc:
        raise CargoBuildReadsError(f"cannot read {path}: {exc}") from exc
    if len(raw) > _MAX_BYTES:
        raise CargoBuildReadsError(f"{path} is larger than {_MAX_BYTES} bytes")
    return raw.decode("utf-8", "surrogateescape")


def _read_dep_info(path: Path) -> tuple[tuple[str, ...], dict[str, str | None]]:
    """Parse a rustc dep-info file with Cargo's grammar.

    Only a space is escaped (``\\ ``); a token that ends in a backslash joins
    the next one, as Cargo's ``parse_rustc_dep_info`` reads it. The first rule
    lists the inputs; the empty rules after it repeat them.
    """
    entries: list[str] = []
    environment: dict[str, str | None] = {}
    first_rule = True
    for line in _read_bounded(path).splitlines():
        if line.startswith(_ENV_DEP):
            name, separator, value = line[len(_ENV_DEP) :].partition("=")
            environment[name] = _unescape_env_value(value) if separator else None
            continue
        target, separator, deps = line.partition(": ")
        if not separator or not first_rule:
            continue
        first_rule = False
        tokens = iter(token for token in deps.split(" ") if token)
        for token in tokens:
            while token.endswith("\\"):
                following = next(tokens, None)
                if following is None:
                    break
                token = token[:-1] + " " + following
            entries.append(token)
    if first_rule:
        raise CargoBuildReadsError(f"rustc dep-info names no target: {path}")
    return tuple(entries), environment


def _unescape_env_value(value: str) -> str:
    # rustc escapes backslashes and newlines in recorded values.
    result: list[str] = []
    escaped = False
    for character in value:
        if escaped:
            result.append(
                "\n" if character == "n" else "\r" if character == "r" else character
            )
            escaped = False
        elif character == "\\":
            escaped = True
        else:
            result.append(character)
    return "".join(result)


def _read_build_script_output(path: Path) -> tuple[tuple[str, ...], tuple[str, ...]]:
    changed: list[str] = []
    variables: list[str] = []
    for line in _read_bounded(path).splitlines():
        for prefix in _RERUN_IF_CHANGED:
            if line.startswith(prefix):
                changed.append(line[len(prefix) :])
        for prefix in _RERUN_IF_ENV_CHANGED:
            if line.startswith(prefix):
                variables.append(line[len(prefix) :])
    return tuple(changed), tuple(variables)


def _files_under(path: Path) -> Iterator[Path]:
    # Cargo scans a directory named by rerun-if-changed recursively.
    for directory, _children, names in os.walk(path):
        for name in names:
            candidate = Path(directory) / name
            if candidate.is_file():
                yield Path(os.path.realpath(candidate))


def _package_files(package: Path) -> Iterator[Path]:
    """Every file of one package, without nested packages or its target
    directory. Cargo lists the package through Git; this walk lists at least
    as much, so an extra file can only cost a rebuild."""
    for directory, children, names in os.walk(package):
        current = Path(directory)
        children[:] = [
            name
            for name in children
            if not (current == package and name == "target")
            and not (current / name / "Cargo.toml").is_file()
        ]
        for name in names:
            candidate = current / name
            if candidate.is_file():
                yield Path(os.path.realpath(candidate))


def _local_package_root(package_id: object) -> Path | None:
    # Cargo spells a path package as "path+file:///abs#name@1.0" or, before
    # 1.77, as "name 1.0 (path+file:///abs)". Registry and git packages have
    # no local sources.
    if not isinstance(package_id, str):
        return None
    start = package_id.find("path+file://")
    if start < 0:
        return None
    url = package_id[start + len("path+") :].rstrip(")")
    parsed = urlparse(url.split("#", 1)[0])
    return Path(os.path.realpath(url2pathname(parsed.path)))


def _under_any(path: Path, roots: Sequence[Path]) -> bool:
    return any(path == root or path.is_relative_to(root) for root in roots)
