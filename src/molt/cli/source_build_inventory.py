"""Build-tool discovery projected from one realized Python environment receipt."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from pathlib import Path, PurePosixPath
from types import MappingProxyType
from typing import cast

from packaging.utils import canonicalize_name
from packaging.requirements import Requirement

from molt.python_environment_custody import validate_python_environment_identity
from molt.python_file_node_custody import (
    VerifiedTreeFile,
    resolve_native_tree_path,
    verify_tree_file,
    verify_tree_files,
)
from molt.python_runtime_identity import runtime_explicit_file_content
from molt.toolchain_identity import (
    StableRegularFileIdentity,
    stable_regular_file_identity,
)
from molt.python_uv_lock_identity import (
    environment_matches_lock_closure,
    validate_uv_lock_group_closure,
)
from molt.cli.source_build_requirements import (
    ResolvedBuildRequirement,
    realized_build_requirement,
)


class SourceBuildInventory:
    """Validated distribution and file ownership, with live content-bound tools."""

    def __init__(self, custody: Mapping[str, object], root: Path) -> None:
        identity = validate_python_environment_identity(
            custody.get("realized_environment")
        )
        lock = validate_uv_lock_group_closure(custody.get("lock_closure"))
        if not environment_matches_lock_closure(identity, lock):
            raise ValueError(
                "build inventory differs from the locked dependency closure"
            )
        self.marker_environment = MappingProxyType(
            dict(cast(Mapping[str, str], lock["marker_environment"]))
        )
        self._activated_extras = MappingProxyType(
            {
                str(row["name"]): frozenset(cast(Sequence[str], row["extras"]))
                for row in cast(Sequence[Mapping[str, object]], lock["packages"])
            }
        )
        self.identity = MappingProxyType(
            {key: identity[key] for key in ("operating_system", "scripts_root")}
        )
        self.root = root.absolute()
        if self.root.resolve(strict=True) != self.root:
            raise ValueError(f"build environment root uses path indirection: {root}")
        self.python_executable = self.root / str(
            cast(Mapping[str, object], identity["selected_executable"])["path"]
        )
        self.distributions = MappingProxyType(
            {
                str(row["name"]): MappingProxyType(
                    {
                        "name": row["name"],
                        "version": row["version"],
                        "entry_points": tuple(
                            MappingProxyType(dict(item))
                            for item in cast(list[dict[str, str]], row["entry_points"])
                        ),
                        "installed_files": tuple(
                            MappingProxyType(dict(item))
                            for item in cast(
                                list[dict[str, object]], row["installed_files"]
                            )
                        ),
                        "console_scripts": MappingProxyType(
                            {
                                name: tuple(paths)
                                for name, paths in cast(
                                    dict[str, list[str]], row["console_scripts"]
                                ).items()
                            }
                        ),
                    }
                )
                for row in cast(list[dict[str, object]], identity["distributions"])
            }
        )
        tree = cast(dict[str, object], identity["tree"])
        self._nodes = MappingProxyType(
            {
                str(row["id"]): MappingProxyType(dict(row))
                for row in cast(list[dict[str, object]], tree["file_nodes"])
            }
        )
        self._entries = MappingProxyType(
            {
                str(row["path"]): MappingProxyType(
                    {
                        **row,
                        "access": MappingProxyType(
                            dict(cast(Mapping[str, bool], row["access"]))
                        ),
                    }
                )
                for row in cast(list[dict[str, object]], tree["entries"])
            }
        )
        self._owned_paths = {
            name: frozenset(
                str(row["path"])
                for row in cast(
                    Sequence[Mapping[str, object]], owner["installed_files"]
                )
            )
            for name, owner in self.distributions.items()
        }
        selected = cast(Mapping[str, object], identity["selected_executable"])
        self._python_kind = selected["kind"]
        content = (
            self._nodes[str(selected["node"])]
            if selected["kind"] == "tree-reference"
            else runtime_explicit_file_content(
                cast(Mapping[str, object], identity["runtime"]), "base-executable"
            )
        )
        assert content is not None  # validated environment owns this runtime role
        self._python_content = {key: content[key] for key in ("size", "sha256")}

    def distribution(self, name: str) -> Mapping[str, object] | None:
        return self.distributions.get(canonicalize_name(name))

    def requirement(
        self, raw: str, requirement: Requirement
    ) -> ResolvedBuildRequirement | None:
        return realized_build_requirement(
            raw,
            requirement,
            self.distribution(requirement.name),
            activated_extras=self._activated_extras.get(
                canonicalize_name(requirement.name), ()
            ),
        )

    def file(self, relative: str, *, distribution: str) -> VerifiedTreeFile:
        if relative not in self._owned_paths.get(canonicalize_name(distribution), ()):
            raise ValueError(
                f"build distribution {distribution!r} does not own {relative!r}"
            )
        return verify_tree_file(
            self.root, relative, entries=self._entries, nodes=self._nodes
        )

    def files(self, *, distribution: str) -> tuple[VerifiedTreeFile, ...]:
        """Bind all installed generator files once without rescanning per file."""
        paths = self._owned_paths.get(canonicalize_name(distribution))
        if not paths:
            raise ValueError(f"build distribution {distribution!r} has no owned files")
        return verify_tree_files(
            self.root, tuple(sorted(paths)), entries=self._entries, nodes=self._nodes
        )

    def python_identity(self, *, base_executable: Path) -> StableRegularFileIdentity:
        """Bind a copied Windows launcher or POSIX base-runtime symlink alike."""
        relative = self.python_executable.relative_to(self.root).as_posix()
        if self._python_kind == "tree-reference":
            return verify_tree_file(
                self.root, relative, entries=self._entries, nodes=self._nodes
            ).content
        path = resolve_native_tree_path(self.root, relative)
        base = base_executable.resolve(strict=True)
        if not path.is_symlink() or path.resolve(strict=True) != base:
            raise ValueError("build interpreter differs from its base-runtime role")
        identity = stable_regular_file_identity(base, label="build interpreter")
        if (
            identity.size != self._python_content["size"]
            or identity.sha256 != self._python_content["sha256"]
        ):
            raise ValueError("build interpreter content differs from its receipt")
        return identity

    def executable_names(self, *, distribution: str) -> tuple[str, ...]:
        """Project command names from entry points and executable owned files."""
        owner = self.distribution(distribution)
        if owner is None:
            return ()
        names = set(cast(Mapping[str, Sequence[str]], owner["console_scripts"]))
        for row in cast(Sequence[Mapping[str, object]], owner["installed_files"]):
            relative = str(row["path"])
            filename = PurePosixPath(relative).name
            if self.identity["operating_system"] == "windows":
                if not filename.endswith(".exe"):
                    continue
                names.add(filename.removesuffix(".exe"))
            elif cast(Mapping[str, bool], self._entries[relative]["access"])[
                "executable"
            ]:
                names.add(filename)
        return tuple(sorted(names))

    def executable(self, name: str, *, distribution: str) -> VerifiedTreeFile:
        """Resolve a declared launcher or a unique distribution-owned payload.

        Entry-point metadata owns launcher selection when present. Wheels may
        instead install native commands directly, including in the scripts
        directory, without declaring a Python entry point. Never discover these
        through PATH or infer their role from their installation directory.
        """
        owner = self.distribution(distribution)
        scripts = (
            cast(Mapping[str, Sequence[str]], owner["console_scripts"]) if owner else {}
        )
        if name in scripts:
            paths = scripts[name]
        else:
            paths = (
                [
                    str(row["path"])
                    for row in cast(
                        Sequence[Mapping[str, object]], owner["installed_files"]
                    )
                ]
                if owner
                else []
            )
        operating_system = self.identity["operating_system"]
        # A Windows entry point may also own its Python companion. Only the
        # platform's executable launcher is a command; companions stay in custody.
        expected_name = name + ".exe" if operating_system == "windows" else name
        candidates = [
            path for path in paths if PurePosixPath(path).name == expected_name
        ]
        if len(candidates) != 1:
            raise ValueError(
                f"build distribution {distribution!r} has {len(candidates)} executable candidates for {name!r}"
            )
        return self.file(candidates[0], distribution=distribution)
