"""Bind a producer's Python dependency to the runtime replaced by Molt.

An attested host import library is a build-provider fact, never a target link
input. Only the exact Meson dependency and interpreter-owned file may be
consumed. All unrelated libraries retain ordinary final-link admission.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path, PurePosixPath, PureWindowsPath
from typing import Any, cast

from molt.cli.source_extension_link_arguments import source_extension_link_arguments
from molt.cli.source_extension_link_requirements import (
    SourceExtensionLinkCyclicGroup,
    SourceExtensionLinkProvider,
    SourceExtensionLinkRequirements,
)
from molt.cli.source_extension_set_registry import SourceExtensionVariant
from molt.exact_json import loads_exact
from molt.file_hashing import _sha256_bytes
from molt.python_file_node_custody import verify_tree_file
from molt.python_runtime_identity import validate_python_runtime_identity
from molt.toolchain_identity import verify_stable_regular_file_identity


@dataclass(frozen=True)
class SourceExtensionPythonProvider:
    dependencies_path: Path
    dependencies_sha256: str
    argument: str | None
    identity: Mapping[str, Any] | None

    def project(
        self, arguments: Sequence[str]
    ) -> tuple[tuple[str, ...], dict[str, Any] | None]:
        remaining: list[str] = []
        consumed: list[str] = []
        for span in source_extension_link_arguments(arguments):
            if self.argument is not None and span.arguments == (self.argument,):
                consumed.extend(span.arguments)
            else:
                remaining.extend(span.arguments)
        receipt = (
            {**cast(Mapping[str, Any], self.identity), "consumed_link_args": consumed}
            if consumed
            else None
        )
        return tuple(remaining), receipt


def validate_static_python_provider_requirements(
    provider: Any,
    requirements: SourceExtensionLinkRequirements,
) -> None:
    """A consumed interpreter provider cannot also survive as a target input."""
    if provider is None:
        return
    library = provider.get("import_library") if isinstance(provider, Mapping) else None
    if (
        not isinstance(library, Mapping)
        or not isinstance(library.get("path"), str)
        or not isinstance(library.get("sha256"), str)
        or provider.get("target_triple") != requirements.target_triple
    ):
        raise ValueError(
            "source-plan Python provider has invalid target/input identity"
        )
    if any(item.sha256 == library["sha256"] for item in requirements.inputs):
        raise ValueError("consumed Python provider survives as a final-link input")
    name = PurePosixPath(library["path"])
    names = {name.name.casefold(), name.stem.casefold()}
    for item in requirements.items:
        atoms = (
            item.members
            if isinstance(item, SourceExtensionLinkCyclicGroup)
            else (item,)
        )
        if any(
            isinstance(atom, SourceExtensionLinkProvider)
            and atom.name.casefold() in names
            for atom in atoms
        ):
            raise ValueError("consumed Python provider survives as a final-link lookup")


@dataclass(frozen=True)
class _WindowsRuntimeImportLibrary:
    dll_name: str
    relative: str
    root_id: str
    sha256: str
    architecture: str
    runtime_closure_sha256: str
    entries: Mapping[str, Mapping[str, Any]]
    nodes: Mapping[str, Mapping[str, Any]]

    def receipt(self, variant: SourceExtensionVariant) -> dict[str, Any]:
        return {
            "schema_version": 1,
            "runtime_closure_sha256": self.runtime_closure_sha256,
            "runtime_library": self.dll_name,
            "runtime_architecture": self.architecture,
            "import_library": {
                "root": self.root_id,
                "path": self.relative,
                "sha256": self.sha256,
            },
            "target_python": variant.target_python.tag,
            "abi_tier": variant.abi_tier,
            "target_triple": variant.target_triple,
        }


def _meson_python_link_dependency(
    path: Path,
) -> tuple[str, Mapping[str, Any] | None, tuple[str, ...]]:
    data = path.read_bytes()
    dependencies = loads_exact(data.decode("utf-8"))
    if not isinstance(dependencies, list) or any(
        not isinstance(row, Mapping) for row in dependencies
    ):
        raise ValueError("Meson intro-dependencies must be an array of objects")
    matches = [row for row in dependencies if row.get("name") == "python"]
    digest = _sha256_bytes(data)
    if not matches:
        return digest, None, ()
    if len(matches) != 1:
        raise ValueError("Meson Python dependency is ambiguous")
    dependency = matches[0]
    arguments = dependency.get("link_args")
    if not isinstance(arguments, list) or any(
        not isinstance(arg, str) or not arg for arg in arguments
    ):
        raise ValueError("Meson Python dependency requires ordered string link_args")
    return digest, dependency, tuple(arguments)


def _select_windows_runtime_import_library(
    runtime: Mapping[str, Any],
) -> _WindowsRuntimeImportLibrary:
    if runtime["operating_system"] != "windows":
        raise ValueError(
            "Meson Python provider requires an attested absolute COFF import library; other provider forms are not admitted"
        )
    libraries = [
        row for row in runtime["explicit_files"] if row["role"] == "runtime-library"
    ]
    if len(libraries) != 1 or libraries[0]["kind"] != "root-reference":
        raise ValueError("Python provider has no tree-owned runtime-library role")
    library = libraries[0]
    runtime_path = PurePosixPath(library["path"])
    # The DLL and libs directory must share the base-prefix root that owns Lib.
    if (
        runtime_path.parent != PurePosixPath(".")
        or runtime_path.suffix.lower() != ".dll"
        or not any(
            row["role"] == "stdlib"
            and row["root"] == library["root"]
            and row["path"] == "Lib"
            for row in runtime["runtime_root_roles"]
        )
    ):
        raise ValueError(
            "Python provider runtime root does not establish the Windows base-prefix layout"
        )
    relative = (PurePosixPath("libs") / runtime_path.with_suffix(".lib")).as_posix()
    roots = {row["id"]: row for row in runtime["runtime_roots"]}
    entries = {row["path"]: row for row in roots[library["root"]]["entries"]}
    nodes = {row["id"]: row for row in runtime["file_nodes"]}
    entry = entries.get(relative)
    if entry is None or entry.get("kind") != "file" or entry.get("node") not in nodes:
        raise ValueError(
            "Python provider import library is absent from interpreter file custody"
        )
    return _WindowsRuntimeImportLibrary(
        dll_name=runtime_path.name,
        relative=relative,
        root_id=library["root"],
        sha256=nodes[entry["node"]]["sha256"],
        architecture=runtime["architecture"],
        runtime_closure_sha256=runtime["runtime_closure_sha256"],
        entries=entries,
        nodes=nodes,
    )


def _meson_provider_argument(arguments: Sequence[str], *, verify_live: bool) -> str:
    spans = source_extension_link_arguments(arguments)
    if len(spans) != 1 or not (
        spans[0].kind == "input"
        or (
            not verify_live
            and spans[0].arguments == (spans[0].value,)
            and spans[0].value.startswith("@python-base/")
        )
    ):
        raise ValueError(
            "Meson Python provider must name exactly one absolute import-library input"
        )
    return spans[0].value


def _validate_live_import_library(
    argument: str, *, python_base: str, library: _WindowsRuntimeImportLibrary
) -> None:
    base = Path(python_base).resolve(strict=True)
    supplied = Path(argument)
    if (
        not supplied.is_absolute()
        or supplied.resolve(strict=True) != base / library.relative
    ):
        raise ValueError(
            "Meson Python provider is not the selected interpreter's exact import library"
        )
    verified = verify_tree_file(
        base, library.relative, entries=library.entries, nodes=library.nodes
    )
    from molt.coff_import_library import validate_coff_import_library

    try:
        validate_coff_import_library(
            verified.path,
            dll_name=library.dll_name,
            architecture=library.architecture,
        )
    finally:
        verify_stable_regular_file_identity(
            verified.content, label="Python import provider"
        )


def source_extension_python_provider(
    *,
    dependencies_path: Path,
    runtime: object,
    variant: SourceExtensionVariant,
    python_base: str,
    verify_live: bool = False,
) -> SourceExtensionPythonProvider:
    """Read one dependency authority for live planning or portable receipt replay.

    Receipt replay passes the staged ``@python-base`` location. Live planning
    additionally revalidates the interpreter file node and COFF import image.
    No interpreter, installer, linker, or package configuration is executed.
    """
    digest, dependency, arguments = _meson_python_link_dependency(dependencies_path)
    empty = SourceExtensionPythonProvider(dependencies_path, digest, None, None)
    if not arguments:
        return empty
    assert dependency is not None
    runtime = cast(Mapping[str, Any], validate_python_runtime_identity(runtime))
    version = str(runtime["version"])
    if (
        dependency.get("type") != "system"
        or dependency.get("version") != variant.cpython
        or ".".join(version.split(".")[:2]) != variant.cpython
        or variant.abi_tier not in {"cpython-abi", "source-compat"}
        or runtime["py_debug"]
        or runtime["gil_disabled"]
    ):
        raise ValueError(
            "Meson Python provider differs from the selected interpreter/version/ABI contract"
        )
    library = _select_windows_runtime_import_library(runtime)
    argument = _meson_provider_argument(arguments, verify_live=verify_live)
    if verify_live:
        _validate_live_import_library(
            argument, python_base=python_base, library=library
        )
    elif PureWindowsPath(argument) != PureWindowsPath(python_base) / library.relative:
        raise ValueError(
            "recorded Meson Python provider escapes the interpreter's exact import-library role"
        )
    return SourceExtensionPythonProvider(
        dependencies_path, digest, argument, library.receipt(variant)
    )
