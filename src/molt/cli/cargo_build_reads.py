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
import os
from pathlib import Path
import re
import stat
import tomllib
from urllib.parse import urlparse
from urllib.request import url2pathname

from molt.file_publication import metadata_is_link_like
from molt.cli.cargo_environment_inputs import CargoEnvironmentInputs
from molt.exact_json import loads_exact
from molt.process_environment import ProcessEnvironment
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
    environment: CargoEnvironmentInputs


@dataclass(frozen=True, slots=True)
class _RustcDepInfo:
    path: Path
    outputs: tuple[str, ...]
    files: tuple[str, ...]
    environment: dict[str, str | None]


@dataclass(frozen=True, slots=True)
class _BuildScriptInputs:
    out_dir: str
    paths: tuple[str, ...]
    variables: tuple[str, ...]
    generated: ProcessEnvironment


# Closed crate-compilation variables from Cargo's environment reference:
# https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-crates
# Build-script execution variables (CARGO_CFG_*, CARGO_FEATURE_*, TARGET, etc.)
# are not automatically rustc overrides. Unknown names remain ambient inputs.
_CARGO_PACKAGE_FIELDS = {
    "CARGO_PKG_NAME": "name",
    "CARGO_PKG_VERSION": "version",
    "CARGO_PKG_AUTHORS": "authors",
    "CARGO_PKG_DESCRIPTION": "description",
    "CARGO_PKG_HOMEPAGE": "homepage",
    "CARGO_PKG_REPOSITORY": "repository",
    "CARGO_PKG_LICENSE": "license",
    "CARGO_PKG_LICENSE_FILE": "license-file",
    "CARGO_PKG_RUST_VERSION": "rust-version",
    "CARGO_PKG_README": "readme",
}
_CARGO_VERSION_COMPONENTS = {
    "CARGO_PKG_VERSION_MAJOR": 1,
    "CARGO_PKG_VERSION_MINOR": 2,
    "CARGO_PKG_VERSION_PATCH": 3,
    "CARGO_PKG_VERSION_PRE": 4,
}


def _environment_name(name: object) -> str:
    if type(name) is not str or not name or "\0" in name or "=" in name:
        raise CargoBuildReadsError("Cargo reported an invalid environment name")
    return name


def _build_script_inputs(
    messages: Sequence[Mapping[str, object]], environment: ProcessEnvironment
) -> dict[str, tuple[_BuildScriptInputs, ...]]:
    result: dict[str, list[_BuildScriptInputs]] = {}
    for message in messages:
        if message.get("reason") != "build-script-executed":
            continue
        package = message.get("package_id")
        out_dir = message.get("out_dir")
        if not isinstance(package, str) or not isinstance(out_dir, str) or not out_dir:
            raise CargoBuildReadsError(
                "a build-script message has no package or out_dir"
            )
        rows = message.get("env", [])
        if not isinstance(rows, list):
            raise CargoBuildReadsError(
                "a build-script message has invalid generated env"
            )
        generated: dict[str, str] = {}
        for row in rows:
            if not isinstance(row, list) or len(row) != 2 or type(row[1]) is not str:
                raise CargoBuildReadsError(
                    "a build-script generated env row is invalid"
                )
            name = environment.rule.key(_environment_name(row[0]))
            if name in generated and generated[name] != row[1]:
                raise CargoBuildReadsError(
                    f"conflicting build-script generated input: {name}"
                )
            generated[name] = row[1]
        paths, variables = _read_build_script_output(Path(out_dir).parent / "output")
        for name in variables:
            _environment_name(name)
        observed = _BuildScriptInputs(
            out_dir,
            paths,
            variables,
            ProcessEnvironment(generated, rule=environment.rule),
        )
        previous = result.setdefault(package, [])
        if any(row.out_dir == out_dir and row != observed for row in previous):
            raise CargoBuildReadsError("conflicting build-script unit observations")
        if observed not in previous:
            previous.append(observed)
    return {package: tuple(rows) for package, rows in result.items()}


def _unit_generated_inputs(
    info: _RustcDepInfo,
    contexts: Sequence[_BuildScriptInputs],
    environment: ProcessEnvironment,
) -> ProcessEnvironment:
    observed = ProcessEnvironment(
        {name: value for name, value in info.environment.items() if value is not None},
        rule=environment.rule,
    )
    if "OUT_DIR" in observed:
        contexts = [row for row in contexts if row.out_dir == observed["OUT_DIR"]]
        if not contexts:
            raise CargoBuildReadsError(
                "rustc OUT_DIR has no matching build-script unit"
            )
    projected = []
    names = {environment.rule.key(name) for name in info.environment}
    for context in contexts:
        values = {
            name: value for name, value in context.generated.items() if name in names
        }
        if "OUT_DIR" in names:
            if "OUT_DIR" in values and values["OUT_DIR"] != context.out_dir:
                raise CargoBuildReadsError(
                    "build-script generated OUT_DIR conflicts with its unit"
                )
            values["OUT_DIR"] = context.out_dir
        projected.append(ProcessEnvironment(values, rule=environment.rule))
    if projected and any(values != projected[0] for values in projected[1:]):
        raise CargoBuildReadsError("rustc unit has ambiguous build-script environment")
    return projected[0] if projected else ProcessEnvironment({}, rule=environment.rule)


def _cargo_manifest(path: Path) -> Mapping[str, object]:
    try:
        return tomllib.loads(_read_bounded(path))
    except tomllib.TOMLDecodeError as exc:
        raise CargoBuildReadsError(f"invalid Cargo package manifest: {path}") from exc


def _package_environment(manifest: Path, workspace_root: Path) -> dict[str, str]:
    document = _cargo_manifest(manifest)
    package = document.get("package")
    if not isinstance(package, dict):
        raise CargoBuildReadsError(f"Cargo package manifest has no package: {manifest}")
    result: dict[str, str] = {}
    shared_package: Mapping[str, object] | None = None
    for name, field in _CARGO_PACKAGE_FIELDS.items():
        value = package.get(
            field, [] if field == "authors" else "0.0.0" if field == "version" else ""
        )
        if isinstance(value, dict) and value == {"workspace": True}:
            if shared_package is None:
                workspace = _cargo_manifest(workspace_root / "Cargo.toml").get(
                    "workspace"
                )
                shared = (
                    workspace.get("package") if isinstance(workspace, Mapping) else None
                )
                if not isinstance(shared, Mapping):
                    raise CargoBuildReadsError(
                        "Cargo workspace package fields are unavailable"
                    )
                shared_package = shared
            value = shared_package.get(field)
            if field in {"license-file", "readme"} and isinstance(value, str):
                value = os.path.relpath(workspace_root / value, manifest.parent)
        if (
            field == "authors"
            and isinstance(value, list)
            and all(type(item) is str for item in value)
        ):
            value = ":".join(value)
        if field == "readme" and (value is True or (field not in package)):
            value = next(
                (
                    name
                    for name in ("README.md", "README.txt", "README")
                    if (manifest.parent / name).is_file()
                ),
                "",
            )
        if field == "readme" and value is False:
            value = ""
        if type(value) is not str:
            raise CargoBuildReadsError(
                f"unsupported Cargo package environment field: {name}"
            )
        result[name] = value
    version = re.fullmatch(
        r"(\d+)\.(\d+)\.(\d+)(?:-([^+]+))?(?:\+[^+]+)?", result["CARGO_PKG_VERSION"]
    )
    if version is None:
        raise CargoBuildReadsError(f"Cargo package has an invalid version: {manifest}")
    result.update(
        {
            name: version.group(index) or ""
            for name, index in _CARGO_VERSION_COMPONENTS.items()
        }
    )
    result["CARGO_MANIFEST_DIR"] = os.fspath(manifest.parent)
    result["CARGO_MANIFEST_PATH"] = os.fspath(manifest)
    return result


def _ambient_unit_inputs(
    message: Mapping[str, object],
    info: _RustcDepInfo,
    contexts: Sequence[_BuildScriptInputs],
    environment: ProcessEnvironment,
    *,
    workspace_root: Path,
    package_values: dict[Path, dict[str, str]],
) -> set[str]:
    target = message.get("target")
    if not isinstance(target, Mapping):
        raise CargoBuildReadsError("a compiler artifact has no target")
    unit = f"{message.get('package_id')} target {target.get('name')}"
    try:
        generated = (
            _unit_generated_inputs(info, contexts, environment)
            if contexts and "custom-build" not in _strings(target.get("kind"))
            else ProcessEnvironment({}, rule=environment.rule)
        )
    except CargoBuildReadsError as exc:
        raise CargoBuildReadsError(f"Cargo unit {unit}: {exc}") from exc
    provided: dict[str, str] = {}
    target_name = target.get("name")
    if isinstance(target_name, str):
        provided["CARGO_CRATE_NAME"] = target_name.replace("-", "_")
        if "bin" in _strings(target.get("kind")) or (
            "example" in _strings(target.get("kind"))
            and "bin" in _strings(target.get("crate_types"))
        ):
            provided["CARGO_BIN_NAME"] = target_name
    package_names = {
        *_CARGO_PACKAGE_FIELDS,
        *_CARGO_VERSION_COMPONENTS,
        "CARGO_MANIFEST_DIR",
        "CARGO_MANIFEST_PATH",
    }
    if any(
        environment.rule.key(name) in package_names and name not in generated
        for name in info.environment
    ):
        raw_manifest = message.get("manifest_path")
        package = _local_package_root(message.get("package_id"))
        manifest = (
            Path(raw_manifest)
            if isinstance(raw_manifest, str)
            else package / "Cargo.toml"
            if package
            else None
        )
        if manifest is None:
            raise CargoBuildReadsError(
                f"Cargo unit {unit}: environment requires its package manifest"
            )
        if not manifest.is_absolute() or (
            package is not None
            and Path(os.path.realpath(manifest)) != package / "Cargo.toml"
        ):
            raise CargoBuildReadsError("Cargo unit manifest differs from its package")
        if manifest not in package_values:
            try:
                package_values[manifest] = _package_environment(
                    manifest, workspace_root
                )
            except CargoBuildReadsError as exc:
                raise CargoBuildReadsError(f"Cargo unit {unit}: {exc}") from exc
        provided.update(package_values[manifest])
    unit_values = ProcessEnvironment(provided, rule=environment.rule)
    names: set[str] = set()
    mismatched: set[str] = set()
    for raw_name, actual in info.environment.items():
        name = environment.rule.key(_environment_name(raw_name))
        if name in generated:
            expected = generated[name]
        elif name in unit_values:
            expected = unit_values[name]
        else:
            expected = environment.get(name)
            names.add(name)
        if actual != expected:
            mismatched.add(name)
    if mismatched:
        raise CargoBuildReadsError(
            f"rustc environment differs from its input owner for {unit}: "
            + ", ".join(sorted(mismatched))
        )
    return names


def cargo_build_reads(
    cargo_stdout: str,
    *,
    workspace_root: Path,
    dep_info_base: Path,
    product_roots: Sequence[Path],
    toolchain_roots: Sequence[Path],
    environment: ProcessEnvironment,
) -> CargoBuildReads:
    """Collect what the Cargo build that printed ``cargo_stdout`` read.

    Files under ``product_roots`` are outputs of build scripts, whose inputs
    are collected; files under ``toolchain_roots`` belong to the plan
    identity. Any other file outside ``workspace_root`` means the build read
    another workspace's sources, so it is refused. ``dep_info_base`` is the
    command owner's explicit base for rustc's relative per-unit paths; an
    isolated Cargo workspace can be nested within the source-admission root.
    The full successful build transcript must contain one terminal
    ``build-finished`` record; bounded diagnostic excerpts are not input evidence.
    """
    if not isinstance(environment, ProcessEnvironment):
        raise TypeError("Cargo build reads require a ProcessEnvironment")
    root = Path(os.path.realpath(workspace_root))
    base = Path(os.path.realpath(dep_info_base))
    skipped = tuple(Path(os.path.realpath(path)) for path in product_roots)
    covered = tuple(Path(os.path.realpath(path)) for path in toolchain_roots)
    files: set[Path] = set()
    directories: set[Path] = set()
    ambient_names: set[str] = set()

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
    scripts = _build_script_inputs(messages, environment)
    package_values: dict[Path, dict[str, str]] = {}
    units = 0
    for message in messages:
        reason = message.get("reason")
        if reason not in {"compiler-artifact", "build-script-executed"}:
            continue
        package_id = message.get("package_id")
        if not isinstance(package_id, str):
            raise CargoBuildReadsError("a Cargo unit message has no package_id")
        package = _local_package_root(package_id)
        if reason == "compiler-artifact":
            script_inputs = scripts.get(package_id, ())
            out_dirs = {script.out_dir for script in script_inputs}
            dep_info = _unit_dep_info(
                message,
                deps_dirs,
                next(iter(out_dirs)) if len(out_dirs) == 1 else None,
                dep_info_base=base,
            )
            if dep_info is None:
                raise CargoBuildReadsError(
                    f"Cargo reported no dep-info for {message.get('package_id')}"
                )
            ambient_names.update(
                _ambient_unit_inputs(
                    message,
                    dep_info,
                    script_inputs,
                    environment,
                    workspace_root=base,
                    package_values=package_values,
                )
            )
            if package is None:
                # Cargo.lock owns registry/git source bytes, not their ambient reads.
                continue
            for entry in dep_info.files:
                path = Path(entry)
                admit(path if path.is_absolute() else base / path)
            units += 1
        elif reason == "build-script-executed":
            script = next(
                row for row in scripts[package_id] if row.out_dir == message["out_dir"]
            )
            changed, variables = script.paths, script.variables
            ambient_names.update(variables)
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
        CargoEnvironmentInputs.capture(
            environment, ambient_names, local_unit_count=units
        ),
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
    """Read a complete successful Cargo build, preserving arbitrary tool text.

    Cargo's JSON protocol ends with build-finished, including with
    json-render-diagnostics. Other tools may print non-JSON lines. A malformed
    JSON candidate cannot be ignored: it could be a unit whose inputs we need.
    https://doc.rust-lang.org/cargo/reference/external-tools.html#json-messages
    """
    finished = False
    # The wire delimiter is LF (possibly CRLF), not Unicode line separators
    # that may occur literally inside JSON strings such as source pathnames.
    for number, line in enumerate(cargo_stdout.split("\n"), start=1):
        if not line.startswith("{"):
            continue
        try:
            message = loads_exact(line)
        except ValueError as exc:
            raise CargoBuildReadsError(
                f"malformed Cargo JSON on stdout line {number}"
            ) from exc
        if finished:
            raise CargoBuildReadsError(
                f"Cargo JSON follows build-finished on stdout line {number}"
            )
        if message.get("reason") == "build-finished":
            if message.get("success") is not True:
                raise CargoBuildReadsError(
                    "Cargo build-finished does not report success:true"
                )
            finished = True
        yield message
    if not finished:
        raise CargoBuildReadsError(
            "Cargo stdout has no successful terminal build-finished record"
        )


def _unit_dep_info(
    message: Mapping[str, object],
    deps_dirs: Iterable[Path],
    out_dir: object,
    *,
    dep_info_base: Path,
) -> _RustcDepInfo | None:
    """The rustc dep-info of one unit, with or without a metadata suffix.

    A library's outputs in ``deps`` carry the ``<crate>-<metadata>`` stem, with
    the dep-info beside them. A build script compiles in
    ``build/<package>-<metadata>/`` as ``build_script_build-<metadata>``, while
    Cargo reports its uplifted copy ``build-script-build``. A root artifact
    Cargo reports only uplifted (a staticlib or binary in the profile
    directory) can have ``<crate>.d`` and ``<crate>-<metadata>.d`` candidates
    across feature and profile lanes. Bind all candidates' populated output
    rules to Cargo's actual output bytes; several units can share ``OUT_DIR``.
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
        {
            path
            for directory in deps_dirs
            for pattern in (f"{crate}.d", f"{crate}-*.d")
            for path in directory.glob(pattern)
        }
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
    return _uplifted_dep_info(candidates, filenames, dep_info_base, library, crate)


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
    dep_info_base: Path,
    library: bool,
    crate: str,
) -> _RustcDepInfo:
    def absolute(value: str) -> Path:
        path = Path(value)
        return path if path.is_absolute() else dep_info_base / path

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
    # Cargo uses Rust str.lines(): only LF/CRLF delimit records. Other Unicode
    # separators remain literal parts of environment names and values.
    for line in re.split(r"\r?\n", _read_bounded(path)):
        if line.startswith(_ENV_DEP):
            name, separator, value = line[len(_ENV_DEP) :].partition("=")
            name = _environment_name(_unescape_env(name))
            actual = _unescape_env(value) if separator else None
            if name in environment and environment[name] != actual:
                raise CargoBuildReadsError(
                    f"conflicting rustc environment facts: {name}"
                )
            environment[name] = actual
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
    # BuildOutput::parse splits bytes only on LF, then trims each line.
    for line in _read_bounded(path).split("\n"):
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
