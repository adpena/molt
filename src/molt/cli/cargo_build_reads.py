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
from contextlib import ExitStack
from dataclasses import dataclass
import json
import os
from pathlib import Path
import stat
from urllib.parse import urlparse
from urllib.request import url2pathname

from molt.file_publication import metadata_is_link_like
from molt.toolchain_identity import (
    StableRegularFileHandle,
    open_stable_regular_file,
    stable_regular_file_handle_identity,
)

# Bound metadata reads separately from the streamed artifact comparisons.
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
# Cargo target_info::file_types labels these import/export and external-debug
# outputs Auxiliary/DebugInfo; none can establish a library's primary identity.
_AUXILIARY_SUFFIXES = (
    ".pdb",
    ".dsym",
    ".dwp",
    ".dll.lib",
    ".dll.exp",
    ".dll.a",
    ".wasm.map",
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


@dataclass(frozen=True, slots=True)
class _RustcDepInfo:
    path: Path
    outputs: tuple[str, ...]
    files: tuple[str, ...]
    environment: dict[str, str | None]


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

    def admit(path: Path, *, whole_package: bool = False) -> None:
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
            contents = (
                _package_files(resolved) if whole_package else _files_under(resolved)
            )
            for entry in contents:
                admit(entry)
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
                message,
                deps_dirs,
                out_dirs.get(message.get("package_id")),
                workspace_root=root,
            )
            if package is None:
                # A registry or git unit: Cargo.lock identifies its sources,
                # but the variables it read still decide its bytes.
                if dep_info is not None:
                    environment.update(dep_info.environment)
                continue
            if dep_info is None:
                raise CargoBuildReadsError(
                    f"Cargo reported no dep-info for {message.get('package_id')}"
                )
            environment.update(dep_info.environment)
            for entry in dep_info.files:
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
                admit(package, whole_package=True)
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
    message: Mapping[str, object],
    deps_dirs: Iterable[Path],
    out_dir: object,
    *,
    workspace_root: Path,
) -> _RustcDepInfo | None:
    """The rustc dep-info of one unit: ``<crate>-<metadata>.d``.

    A library's outputs in ``deps`` carry the ``<crate>-<metadata>`` stem, with
    the dep-info beside them. A build script compiles in
    ``build/<package>-<metadata>/`` as ``build_script_build-<metadata>``, while
    Cargo reports its uplifted copy ``build-script-build``. A root artifact
    Cargo reports only uplifted (a staticlib or binary in the profile
    directory) has several ``<crate>-<metadata>.d`` candidates, one per feature
    and profile lane. Bind its populated output rules to Cargo's actual
    output bytes; several units can share the same ``OUT_DIR``.
    """
    filenames = _strings(message.get("filenames"))
    target = message.get("target")
    if not filenames or not isinstance(target, Mapping):
        return None
    kinds = [kind for kind in _strings(target.get("kind")) if isinstance(kind, str)]
    library = any(kind in _LIBRARY_KINDS for kind in kinds)
    executable = message.get("executable")
    if not library and isinstance(executable, str):
        filenames = [executable]
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
                return _read_dep_info(candidate)
    if crate is None:
        return None
    candidates = []
    for path in sorted(
        {path for directory in deps_dirs for path in directory.glob(f"{crate}-*.d")}
    ):
        # Inspect all records before selection; an unreadable candidate cannot
        # establish uniqueness. OUT_DIR narrows candidates only when recorded.
        candidate = _read_dep_info(path)
        if (
            isinstance(out_dir, str)
            and "OUT_DIR" in candidate.environment
            and candidate.environment["OUT_DIR"] != out_dir
        ):
            continue
        candidates.append(candidate)
    if not candidates:
        return None
    return _uplifted_dep_info(candidates, filenames, workspace_root, library, crate)


def _output_role(path: Path, library: bool) -> str | None:
    # Binary names may contain dots; their executable role does not depend on
    # an extension. Cargo's executable field excludes ancillary debug outputs.
    if not library:
        return "executable"
    suffix = path.suffix.lower()
    return (
        None
        if suffix == ".d" or path.name.lower().endswith(_AUXILIARY_SUFFIXES)
        else suffix
    )


def _uplifted_dep_info(
    candidates: Sequence[_RustcDepInfo],
    filenames: Sequence[str],
    root: Path,
    library: bool,
    crate: str,
) -> _RustcDepInfo:
    def absolute(value: str) -> Path:
        path = Path(value)
        return path if path.is_absolute() else root / path

    try:
        with ExitStack() as custody:
            outputs = [
                (
                    role,
                    custody.enter_context(
                        open_stable_regular_file(absolute(name), label="Cargo output")
                    ),
                )
                for name in filenames
                if (role := _output_role(Path(name), library)) is not None
            ]
            digests: dict[Path, str] = {}

            def matches_output(path: Path, output: StableRegularFileHandle) -> bool:
                try:
                    metadata = path.lstat()
                except FileNotFoundError:
                    # Cargo can retain dep-info after retiring an old output.
                    return False
                if not stat.S_ISREG(metadata.st_mode) or metadata_is_link_like(
                    metadata
                ):
                    raise CargoBuildReadsError(
                        f"dep-info output is not a direct file: {path}"
                    )
                if metadata.st_size != output.stat.st_size:
                    return False
                with open_stable_regular_file(
                    path, label="dep-info output", expected_path_stat=metadata
                ) as candidate:
                    if candidate.stat.st_ino and os.path.samestat(
                        candidate.stat, output.stat
                    ):
                        return True
                    if output.path not in digests:
                        digests[output.path] = stable_regular_file_handle_identity(
                            output, label="Cargo output"
                        ).sha256
                    return (
                        stable_regular_file_handle_identity(
                            candidate, label="dep-info output"
                        ).sha256
                        == digests[output.path]
                    )

            matches = []
            for candidate in candidates:
                targets: dict[str, list[Path]] = {}
                for name in candidate.outputs:
                    path = absolute(name)
                    if os.path.normcase(os.path.abspath(path)) == os.path.normcase(
                        os.path.abspath(candidate.path)
                    ):
                        continue
                    role = _output_role(path, library)
                    if role is not None:
                        targets.setdefault(role, []).append(path)
                if outputs and all(
                    any(matches_output(path, output) for path in targets.get(role, ()))
                    for role, output in outputs
                ):
                    matches.append(candidate)
            if not matches:
                raise CargoBuildReadsError(
                    f"no dep-info output matches Cargo's {crate} outputs: "
                    + ", ".join(filenames)
                )
            selected = matches[0]
            if len(matches) == 1:
                return selected
            closure = (
                frozenset(os.path.realpath(absolute(name)) for name in selected.files),
                selected.environment,
            )
            for candidate in matches[1:]:
                if (
                    frozenset(
                        os.path.realpath(absolute(name)) for name in candidate.files
                    ),
                    candidate.environment,
                ) != closure:
                    raise CargoBuildReadsError(
                        f"several dep-info files match {crate} output bytes with different reads: "
                        + ", ".join(os.fspath(item.path) for item in matches)
                    )
            return selected
    except (OSError, ValueError) as exc:
        if isinstance(exc, CargoBuildReadsError):
            raise
        raise CargoBuildReadsError(
            f"cannot bind Cargo's {crate} output: {exc}"
        ) from exc


def _strings(value: object) -> list[str]:
    return (
        [item for item in value if isinstance(item, str)]
        if isinstance(value, list)
        else []
    )


def _read_bounded(path: Path) -> str:
    try:
        with open_stable_regular_file(path, label="Cargo input record") as opened:
            raw = opened.stream.read(_MAX_BYTES + 1)
    except (OSError, ValueError) as exc:
        raise CargoBuildReadsError(f"cannot read {path}: {exc}") from exc
    if len(raw) > _MAX_BYTES:
        raise CargoBuildReadsError(f"{path} is larger than {_MAX_BYTES} bytes")
    return raw.decode("utf-8", "surrogateescape")


def _dep_words(value: str, path: Path) -> tuple[str, ...]:
    words = []
    tokens = iter(value.split())
    for token in tokens:
        while token.endswith("\\"):
            following = next(tokens, None)
            if following is None:
                raise CargoBuildReadsError(
                    f"rustc dep-info has a trailing backslash: {path}"
                )
            token = token[:-1] + " " + following
        words.append(token)
    return tuple(words)


def _read_dep_info(path: Path) -> _RustcDepInfo:
    """Parse a rustc dep-info file with Cargo's grammar.

    Only a space is escaped (``\\ ``); a token that ends in a backslash joins
    the next one, as Cargo's ``parse_rustc_dep_info`` reads it. The first rule
    lists the inputs. Other populated rules name the same reads for each
    emitted output; the empty rules after them repeat the source names.
    """
    entries: list[str] = []
    outputs: list[str] = []
    environment: dict[str, str | None] = {}
    first_rule = True
    for line in _read_bounded(path).splitlines():
        if line.startswith(_ENV_DEP):
            name, separator, value = line[len(_ENV_DEP) :].partition("=")
            environment[_unescape_env(name)] = (
                _unescape_env(value) if separator else None
            )
            continue
        target, separator, deps = line.partition(": ")
        if not separator:
            continue
        files = _dep_words(deps, path)
        if not files:
            continue
        if first_rule:
            entries.extend(files)
            first_rule = False
        elif set(entries) != set(files):
            raise CargoBuildReadsError(
                f"rustc dep-info output rules have different reads: {path}"
            )
        outputs.extend(_dep_words(target, path))
    if first_rule:
        raise CargoBuildReadsError(f"rustc dep-info names no target: {path}")
    return _RustcDepInfo(path, tuple(outputs), tuple(entries), environment)


def _unescape_env(value: str) -> str:
    # Cargo decodes exactly these escapes in both variable names and values.
    result: list[str] = []
    escaped = False
    for character in value:
        if escaped:
            if character not in ("n", "r", "\\"):
                raise CargoBuildReadsError(
                    f"unknown rustc environment escape: {character!r}"
                )
            result.append(
                "\n" if character == "n" else "\r" if character == "r" else "\\"
            )
            escaped = False
        elif character == "\\":
            escaped = True
        else:
            result.append(character)
    if escaped:
        raise CargoBuildReadsError("unterminated rustc environment escape")
    return "".join(result)


def _read_build_script_output(path: Path) -> tuple[tuple[str, ...], tuple[str, ...]]:
    changed: list[str] = []
    variables: list[str] = []
    for line in _read_bounded(path).splitlines():
        line = line.strip()
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
    # File URL authorities carry UNC hosts; dropping one silently changes the
    # package root on Windows. localhost denotes this host in a file URI.
    path = (
        "//" + parsed.netloc + parsed.path
        if parsed.netloc and parsed.netloc != "localhost"
        else parsed.path
    )
    return Path(os.path.realpath(url2pathname(path)))


def _under_any(path: Path, roots: Sequence[Path]) -> bool:
    return any(path == root or path.is_relative_to(root) for root in roots)
