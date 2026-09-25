"""Admit one explicit differential payload root without moving proof custody."""

from __future__ import annotations

import os
import stat
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping, MutableMapping

from molt import disk_capacity
from molt.build_state_layout import build_state_root
from molt.exact_json import canonical_json_bytes, dumps_exact, loads_exact
from molt.file_publication import durable_remove_path, is_link_like, resolve_owned_path
from tools.proof_queue_pkg import cargo_output_layout

ROOT_ENV = "MOLT_DIFF_GUEST_OUTPUT_ROOT"
IDENTITY_ENV = "MOLT_DIFF_GUEST_OUTPUT_IDENTITY"
OUTPUT_KEYS = (
    "MOLT_DIFF_TMPDIR",
    "MOLT_DIFF_CARGO_TARGET_DIR",
    "CARGO_TARGET_DIR",
    "MOLT_COMPAT_SCRATCH_ROOT",
)


def keep_artifacts(environment: Mapping[str, str]) -> bool:
    return environment.get("MOLT_DIFF_KEEP", "").strip().lower() in {
        "1",
        "true",
        "yes",
        "on",
    }


def _declaration(environment: Mapping[str, str]) -> dict[str, object] | None:
    raw = environment.get(ROOT_ENV, "").strip()
    bound = environment.get(IDENTITY_ENV, "").strip()
    if not raw:
        if bound:
            raise ValueError("differential guest output identity has no selected root")
        return None
    if bound:
        try:
            declaration = cargo_output_layout.declared_root(loads_exact(bound))
        except (TypeError, ValueError) as exc:
            raise ValueError("differential guest output identity is malformed") from exc
        if declaration is None:
            raise ValueError("differential guest output identity is malformed")
        if resolve_owned_path(Path(raw)) != Path(str(declaration["path"])):
            raise ValueError("differential guest output selector changed")
        cargo_output_layout.validate_root(declaration)
        return declaration
    # First admission captures this exact object once. Subsequent validation
    # never replaces it with a second declaration.
    return cargo_output_layout.declare_root(raw)


def _validate(
    declaration: Mapping[str, object],
    *,
    repo_root: Path,
    custody_root: Path,
    environment: Mapping[str, str],
) -> Path:
    if not environment.get("MOLT_EXT_ROOT", "").strip():
        raise ValueError(
            "selected differential guest output requires canonical MOLT_EXT_ROOT"
        )
    root = Path(str(declaration["path"]))
    artifact = resolve_owned_path(
        Path(environment.get("MOLT_EXT_ROOT") or repo_root).expanduser()
    )
    state = environment.get("MOLT_BUILD_STATE_DIR", "").strip()
    if state:
        state_path = Path(state).expanduser()
        if not state_path.is_absolute():
            state_path = repo_root / state_path
        if not resolve_owned_path(state_path).is_relative_to(artifact):
            raise ValueError(
                "selected differential build state escaped canonical custody"
            )
    custody = resolve_owned_path(custody_root)
    if not custody.is_relative_to(artifact):
        raise ValueError(
            "differential custody must remain under canonical artifact root"
        )
    layout = cargo_output_layout.CargoOutputLayout.create(
        result_root=custody, declaration=declaration, source_root=repo_root
    )
    protected = [artifact, artifact / ".molt_cache"]
    protected.extend(
        Path(environment[name])
        for name in ("MOLT_CACHE", "MOLT_TARGET_ROOT", "UV_CACHE_DIR")
        if environment.get(name)
    )
    layout.validate(protected_roots=protected)
    layout.validate_environment(environment)
    return root


def selected_root(
    environment: Mapping[str, str], *, repo_root: Path, custody_root: Path
) -> Path | None:
    declaration = _declaration(environment)
    if declaration is None:
        return None
    return _validate(
        declaration,
        repo_root=repo_root,
        custody_root=custody_root,
        environment=environment,
    )


def _derived(root: Path) -> dict[str, str]:
    target = str(root / "cargo-target")
    return {
        "MOLT_DIFF_TMPDIR": str(root / "guest-tmp"),
        "MOLT_DIFF_CARGO_TARGET_DIR": target,
        "CARGO_TARGET_DIR": target,
        "MOLT_COMPAT_SCRATCH_ROOT": str(root / "compat-scratch"),
    }


def isolated_state_root(
    *, target: Path, environment: Mapping[str, str], repo_root: Path
) -> Path:
    """An isolated target cannot inherit another target's explicit control root."""
    return build_state_root(
        project_root=repo_root,
        cargo_target=target,
        environment={
            key: value
            for key, value in environment.items()
            if key != "MOLT_BUILD_STATE_DIR"
        },
    )


def projected_target(environment: Mapping[str, str], root: Path) -> Path:
    """Only the shared target or an explicitly named in-root isolation target."""
    default = root / "cargo-target"
    cargo = environment.get("CARGO_TARGET_DIR", str(default))
    diff = environment.get("MOLT_DIFF_CARGO_TARGET_DIR", str(default))
    if cargo == diff == str(default):
        return default
    if cargo != diff:
        raise ValueError("differential target selectors disagree")
    target = resolve_owned_path(Path(cargo))
    mode = environment.get("MOLT_DIFF_TARGET_MODE", "")
    if mode == "dyld":
        namespace = root / "guest-tmp" / "dyld_quarantine"
        if target.name == "target" and target.parent.parent == namespace:
            return target
    if mode == "isolated-retry":
        namespace = root / "guest-tmp"
        if (
            target.name == "target"
            and target.parent.parent == namespace
            and target.parent.name.startswith("molt_diff_retry_")
        ):
            return target
    raise ValueError("differential Cargo target escaped selected guest output root")


def admit(
    environment: MutableMapping[str, str],
    *,
    repo_root: Path,
    custody_root: Path,
    explicit_outputs: Mapping[str, str] | None = None,
) -> Path | None:
    declaration = _declaration(environment)
    if declaration is None:
        return None
    root = _validate(
        declaration,
        repo_root=repo_root,
        custody_root=custody_root,
        environment=environment,
    )
    derived = _derived(root)
    if environment.get(IDENTITY_ENV):
        # Re-admission validates the existing selection; it cannot silently
        # restore changed paths or erase a deliberately isolated target.
        enforce_child(environment, repo_root=repo_root)
        target = str(projected_target(environment, root))
        derived.update(CARGO_TARGET_DIR=target, MOLT_DIFF_CARGO_TARGET_DIR=target)
    for key, old in (explicit_outputs or {}).items():
        if (
            key in derived
            and old
            and resolve_owned_path(Path(old)) != Path(derived[key])
        ):
            raise ValueError(f"{key} conflicts with selected differential output root")
    disk_capacity.require_build_capacity(
        (
            Path(derived["CARGO_TARGET_DIR"]),
            root / "guest-tmp",
            root / "compat-scratch",
        ),
        env=environment,
    )
    cargo_output_layout.validate_root(declaration)
    environment[IDENTITY_ENV] = canonical_json_bytes(declaration).decode("utf-8")
    environment.update(derived)
    return root


def enforce_child(
    environment: MutableMapping[str, str], *, repo_root: Path
) -> Path | None:
    artifact = Path(environment.get("MOLT_EXT_ROOT") or repo_root)
    custody = Path(environment.get("MOLT_DIFF_ROOT") or artifact / "tmp" / "diff")
    root = selected_root(environment, repo_root=repo_root, custody_root=custody)
    if root is None:
        return None
    projected_target(environment, root)
    for key, value in _derived(root).items():
        if key in {"CARGO_TARGET_DIR", "MOLT_DIFF_CARGO_TARGET_DIR"}:
            continue
        if environment.get(key) != value:
            raise ValueError(f"{key} escaped selected differential output root")
    return root


def selected_for_compat(
    environment: Mapping[str, str], *, repo_root: Path
) -> Path | None:
    return enforce_child(dict(environment), repo_root=repo_root)


def _directory_identity(path: Path) -> tuple[int, int]:
    metadata = path.lstat()
    if not stat.S_ISDIR(metadata.st_mode) or not metadata.st_ino or is_link_like(path):
        raise ValueError(f"guest output is not a direct directory: {path}")
    return metadata.st_dev, metadata.st_ino


@dataclass(frozen=True, slots=True)
class GuestOutputLease:
    path: Path
    parent: Path
    boundary: Path
    parent_identity: tuple[int, int]
    leaf_identity: tuple[int, int]

    def retire(
        self, *, environment: Mapping[str, str], repo_root: Path, keep: bool = False
    ) -> str | None:
        """Report cleanup failure, never adopt or delete a changed leaf."""
        if keep:
            return None
        try:
            selected_for_compat(environment, repo_root=repo_root)
            if (
                resolve_owned_path(self.path) != self.path
                or self.path.parent != self.parent
                or not self.path.is_relative_to(self.boundary)
                or _directory_identity(self.parent) != self.parent_identity
                or _directory_identity(self.path) != self.leaf_identity
            ):
                raise ValueError(f"guest output lease identity changed: {self.path}")
            durable_remove_path(self.path, retirement_scope=self.path.name)
            return None
        except (OSError, ValueError, RuntimeError) as error:
            artifact = Path(environment.get("MOLT_EXT_ROOT") or repo_root)
            custody = Path(
                environment.get("MOLT_DIFF_ROOT") or artifact / "tmp" / "diff"
            )
            receipt = custody / "guest_output_cleanup_failures.jsonl"
            diagnostic = f"guest output cleanup failed at {self.path}: {error}"
            try:
                receipt = resolve_owned_path(receipt)
                receipt.parent.mkdir(parents=True, exist_ok=True)
                with receipt.open("a", encoding="utf-8") as handle:
                    handle.write(
                        dumps_exact(
                            {
                                "schema": "molt.guest-output-cleanup-failure.v1",
                                "path": str(self.path),
                                "error": str(error),
                                "run_id": environment.get("MOLT_DIFF_RUN_ID", ""),
                            },
                            indent=None,
                        )
                        + "\n"
                    )
                    handle.flush()
                    os.fsync(handle.fileno())
            except (OSError, ValueError, RuntimeError) as receipt_error:
                diagnostic += f"; receipt write failed at {receipt}: {receipt_error}"
            return diagnostic


def new_guest_leaf(
    parent: Path,
    *,
    prefix: str,
    boundary: Path,
    environment: Mapping[str, str],
    repo_root: Path,
) -> GuestOutputLease:
    selected_for_compat(environment, repo_root=repo_root)
    parent = resolve_owned_path(parent)
    boundary = resolve_owned_path(boundary)
    if not parent.is_relative_to(boundary):
        raise ValueError("guest output parent escaped admitted boundary")
    parent.mkdir(parents=True, exist_ok=True)
    parent_identity = _directory_identity(parent)
    path = Path(tempfile.mkdtemp(prefix=prefix, dir=parent))
    if resolve_owned_path(path).parent != parent:
        raise ValueError("guest output leaf escaped its parent")
    return GuestOutputLease(
        path, parent, boundary, parent_identity, _directory_identity(path)
    )


def claim_new_guest_leaf(
    path: Path, *, boundary: Path, environment: Mapping[str, str], repo_root: Path
) -> GuestOutputLease:
    """Claim an exact target-addressed control directory only when newly made."""
    selected_for_compat(environment, repo_root=repo_root)
    path = resolve_owned_path(path)
    parent = resolve_owned_path(path.parent)
    boundary = resolve_owned_path(boundary)
    if not path.is_relative_to(boundary):
        raise ValueError("guest control state escaped canonical boundary")
    parent.mkdir(parents=True, exist_ok=True)
    parent_identity = _directory_identity(parent)
    path.mkdir(exist_ok=False)
    return GuestOutputLease(
        path, parent, boundary, parent_identity, _directory_identity(path)
    )
