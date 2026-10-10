"""Public facade and standalone probe for exact CPython environment custody."""

from __future__ import annotations

import importlib.machinery
import importlib.util
import json
import sys
from collections.abc import Sequence
from pathlib import Path
from types import ModuleType
from typing import NoReturn


def _load_package_import_custody(path: Path) -> ModuleType:
    """Load one neutral authority; never import it through a foreign parent."""
    name = "_molt_package_import_custody"
    selected = path.resolve(strict=True)
    if name in sys.modules:
        loaded = sys.modules[name]
        spec = getattr(loaded, "__spec__", None)
        loader = getattr(spec, "loader", None)
        if (
            type(loaded) is not ModuleType
            or getattr(loaded, "__name__", None) != name
            or getattr(loaded, "__package__", None) != ""
            or getattr(loaded, "__file__", None) != str(selected)
            or getattr(spec, "name", None) != name
            or getattr(spec, "origin", None) != str(selected)
            or type(loader) is not importlib.machinery.SourceFileLoader
            or loader.name != name
            or loader.path != str(selected)
            or getattr(loaded, "__loader__", None) is not loader
        ):
            raise ImportError(
                f"package import custody already loaded from another authority; "
                f"selected {selected}, loaded {getattr(loaded, '__file__', None)!r}"
            )
        return loaded
    spec = importlib.util.spec_from_file_location(name, selected)
    if spec is None or spec.loader is None:
        raise ImportError(f"cannot load selected package import custody: {selected}")
    loaded = importlib.util.module_from_spec(spec)
    sys.modules[name] = loaded
    try:
        spec.loader.exec_module(loaded)
    except BaseException:
        sys.modules.pop(name, None)
        raise
    return loaded


_bootstrap_root: str | None = None
if not __package__:
    _selected_package = Path(__file__).resolve().parent
    _admission_was_present = "_molt_package_import_custody" in sys.modules
    try:
        _package_import_custody = _load_package_import_custody(
            _selected_package / "package_import_custody.py"
        )
        _package_import_custody.admit_loaded_package("molt", _selected_package)
    except (ImportError, OSError, RuntimeError) as exc:
        if not _admission_was_present:
            sys.modules.pop("_molt_package_import_custody", None)
        print(
            f"Python capture package source admission failed: {exc}; "
            "use an interpreter environment bound to the selected Molt package. "
            "Isolated site startup ignores PYTHONPATH; source-root admission "
            "does not replace an already loaded package.",
            file=sys.stderr,
        )
        raise SystemExit(2) from exc
    _bootstrap_root = str(_selected_package.parent)
    sys.path.insert(0, _bootstrap_root)

from molt.python_identity_common import (  # noqa: E402
    PythonEnvironmentIdentityError as PythonEnvironmentIdentityError,
)


def _parse_arguments():
    import argparse

    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_mutually_exclusive_group(required=True)
    modes.add_argument("--capture-runtime", action="store_true")
    modes.add_argument("--runtime-selection", action="store_true")
    modes.add_argument("--locate-active-environment", action="store_true")
    modes.add_argument("--capture-environment", type=Path, metavar="ROOT")
    modes.add_argument("--capture-active-environment", action="store_true")
    parser.add_argument("--runtime-session", action="store_true")
    parser.add_argument("--with-custody", action="store_true")
    parser.add_argument("--hash-workers", type=int, default=1)
    parser.add_argument("--admit-virtualenv-bootstrap", action="store_true")
    parser.add_argument("--admit-site-bootstrap", action="append", default=[])
    parser.add_argument("--admit-external-root", type=Path, action="append", default=[])
    args = parser.parse_args()
    admissions = (
        args.admit_virtualenv_bootstrap
        or args.admit_site_bootstrap
        or args.admit_external_root
    )
    if args.locate_active_environment and (
        args.with_custody or args.hash_workers != 1 or admissions
    ):
        parser.error("location accepts no capture or environment admission options")
    if args.runtime_selection and (
        args.with_custody or args.hash_workers != 1 or admissions
    ):
        parser.error("runtime selection accepts no capture or admission options")
    if args.capture_runtime and admissions:
        parser.error("runtime capture accepts no environment admission options")
    if args.runtime_session and (not args.capture_runtime or args.with_custody):
        parser.error("runtime session requires runtime capture without an envelope")
    return args


_arguments = _parse_arguments() if __name__ == "__main__" else None
if _arguments is not None and _arguments.locate_active_environment:
    from molt.python_environment_location import (  # noqa: E402
        locate_current_python_environment as locate_current_python_environment,
    )
else:
    # Runtime and environment captures must observe the same probe import
    # context. A lighter runtime-only import lane changes the loaded native
    # image census, making an identical interpreter acquire different recipe
    # identities during planning and environment attestation.
    from molt.python_capture import (  # noqa: E402
        PYTHON_CAPTURE_SCHEMA as PYTHON_CAPTURE_SCHEMA,
        validate_python_capture as validate_python_capture,
    )
    from molt.python_environment_custody import (  # noqa: E402
        PYTHON_ENVIRONMENT_CAPABILITY_SCHEMA as PYTHON_ENVIRONMENT_CAPABILITY_SCHEMA,
        PYTHON_ENVIRONMENT_IDENTITY_SCHEMA as PYTHON_ENVIRONMENT_IDENTITY_SCHEMA,
        capture_current_python_environment,
        python_environment_executable_files as python_environment_executable_files,
        validate_python_environment_identity as validate_python_environment_identity,
        virtualenv_site_bootstrap_relative_paths as virtualenv_site_bootstrap_relative_paths,
    )
    from molt.python_environment_location import (  # noqa: E402
        PYTHON_ENVIRONMENT_LOCATION_SCHEMA as PYTHON_ENVIRONMENT_LOCATION_SCHEMA,
        locate_current_python_environment as locate_current_python_environment,
        validate_python_environment_location as validate_python_environment_location,
    )
    from molt.python_runtime_identity import (  # noqa: E402
        PYTHON_RUNTIME_CAPABILITY_SCHEMA as PYTHON_RUNTIME_CAPABILITY_SCHEMA,
        PYTHON_RUNTIME_IDENTITY_SCHEMA as PYTHON_RUNTIME_IDENTITY_SCHEMA,
        capture_current_python_runtime as capture_current_python_runtime,
        current_python_runtime_selection as current_python_runtime_selection,
        validate_python_runtime_identity as validate_python_runtime_identity,
    )
    from molt.python_uv_lock_identity import (  # noqa: E402
        PYTHON_MARKER_ENVIRONMENT_FIELDS as PYTHON_MARKER_ENVIRONMENT_FIELDS,
        UV_LOCK_GROUP_CLOSURE_SCHEMA as UV_LOCK_GROUP_CLOSURE_SCHEMA,
        environment_matches_lock_closure as environment_matches_lock_closure,
        selected_uv_lock_group_closure as selected_uv_lock_group_closure,
        validate_uv_lock_group_closure as validate_uv_lock_group_closure,
    )

if _bootstrap_root is not None:
    try:
        sys.path.remove(_bootstrap_root)
    except ValueError:
        pass


def _probe_error(message: str) -> NoReturn:
    print(message, file=sys.stderr)
    raise SystemExit(2)


def python_identity_probe_arguments(
    arguments: Sequence[str], *, no_site: bool = False
) -> list[str]:
    """Launch this read-only authority with isolation and no bytecode writes.

    Isolation ignores PYTHONDONTWRITEBYTECODE, so the no-write policy must be
    an interpreter flag before either site startup or the authority imports.
    Interpreter/launcher selection belongs to the caller; this owns its suffix.
    """
    return [
        "-B",
        "-I",
        *(["-S"] if no_site else []),
        str(Path(__file__).resolve(strict=True)),
        *arguments,
    ]


def python_capture_authority_paths(
    *, source_root: Path | None = None
) -> tuple[Path, ...]:
    """Capture implementation inputs, optionally projected into compiler sources.

    The loaded probe may live in site-packages. Source-based build identities
    select their source root explicitly instead of deriving a repository layout
    from that installation's physical import path.
    """
    names = (
        "__init__",
        "_version",
        "_host_exit",
        "pytest_memory_guard_bootstrap",
        "source_root",
        "temporary_artifacts",
        "disk_capacity",
        "file_deletion",
        "file_locks",
        "custody_layout",
        "memory_guard_paths",
        "process_spawn",
        "dx",
        "environment_registry",
        "_environment_registry",
        "tool_releases",
        "path_custody",
        "python_environment_identity",
        "python_environment_location",
        "python_environment_custody",
        "python_external_custody",
        "python_private_names",
        "package_import_custody",
        "python_runtime_identity",
        "python_file_node_custody",
        "python_native_dependency_custody",
        "native_artifact_header",
        "native_target_shape",
        "python_native_locations",
        "python_identity_common",
        "python_uv_lock_identity",
        "python_capture",
        "toolchain_identity",
        "llvm_linker_roles",
        "file_hashing",
        "exact_json",
        "file_publication",
        "portable_paths",
    )
    root = (
        Path(__file__).resolve().parent
        if source_root is None
        else source_root / "src" / "molt"
    )
    # Standalone probes import the package bootstrap too. Its version resolver
    # consults this source-tree marker even when the proof's cwd is another repo.
    return (
        *(root / f"{name}.py" for name in names),
        root.parent / "sitecustomize.py",
        root.parent.parent / "pyproject.toml",
    )


def _serve_runtime_session(payload, context, *, requests, responses) -> None:
    """Verify through the producer's retained context; EOF revokes the session."""
    try:
        responses.write(
            json.dumps(
                {
                    "runtime": payload,
                    "startup_selection": current_python_runtime_selection(),
                },
                allow_nan=False,
                separators=(",", ":"),
                sort_keys=True,
            )
            + "\n"
        )
        responses.flush()
        for request in requests:
            if request != "verify\n":
                raise PythonEnvironmentIdentityError("invalid runtime session request")
            context.verify()
            responses.write(str(payload["runtime_closure_sha256"]) + "\n")
            responses.flush()
    finally:
        context.close()


def _main() -> None:
    args = _arguments
    assert args is not None
    if args.locate_active_environment:
        payload = locate_current_python_environment()
    elif args.runtime_selection:
        payload = current_python_runtime_selection()
    else:
        from molt.python_file_node_custody import PythonFileCaptureContext

        with PythonFileCaptureContext(hash_workers=args.hash_workers) as context:
            if args.capture_runtime:
                payload = capture_current_python_runtime(
                    capture_context=context, with_custody=args.with_custody
                )
            else:
                from molt.python_environment_location import _active_environment_prefix

                root = args.capture_environment or _active_environment_prefix()
                bootstrap = list(args.admit_site_bootstrap)
                if args.admit_virtualenv_bootstrap:
                    bootstrap.extend(virtualenv_site_bootstrap_relative_paths(root))
                payload = capture_current_python_environment(
                    root,
                    excluded_relative_paths=("molt-source-build-environment.json",),
                    admitted_site_bootstrap_paths=bootstrap,
                    admitted_external_roots=args.admit_external_root,
                    capture_context=context,
                    with_custody=args.with_custody,
                )
            if args.runtime_session:
                _serve_runtime_session(
                    payload, context, requests=sys.stdin, responses=sys.stdout
                )
                return
    print(json.dumps(payload, allow_nan=False, separators=(",", ":"), sort_keys=True))


if __name__ == "__main__":
    try:
        _main()
    except (OSError, ValueError) as exc:
        _probe_error(str(exc))
