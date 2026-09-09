"""Public facade and standalone probe for exact CPython environment custody."""

from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import NoReturn

_bootstrap_root: str | None = None
if not __package__:
    _bootstrap_root = str(Path(__file__).resolve().parents[1])
    sys.path.insert(0, _bootstrap_root)

from molt.python_identity_common import (  # noqa: E402
    PythonEnvironmentIdentityError as PythonEnvironmentIdentityError,
)


def _parse_arguments():
    import argparse

    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_mutually_exclusive_group(required=True)
    modes.add_argument("--capture-runtime", action="store_true")
    modes.add_argument("--locate-active-environment", action="store_true")
    modes.add_argument("--capture-environment", type=Path, metavar="ROOT")
    modes.add_argument("--capture-active-environment", action="store_true")
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
    if args.capture_runtime and admissions:
        parser.error("runtime capture accepts no environment admission options")
    return args


_arguments = _parse_arguments() if __name__ == "__main__" else None
if _arguments is not None and _arguments.capture_runtime:
    from molt.python_runtime_identity import (  # noqa: E402
        capture_current_python_runtime as capture_current_python_runtime,
    )
elif _arguments is not None and _arguments.locate_active_environment:
    from molt.python_environment_location import (  # noqa: E402
        locate_current_python_environment as locate_current_python_environment,
    )
else:
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


def python_capture_authority_paths() -> tuple[Path, ...]:
    """The single source closure for selected-Python capture and validation."""
    names = (
        "__init__",
        "_version",
        "_host_exit",
        "pytest_memory_guard_bootstrap",
        "memory_guard_paths",
        "process_spawn",
        "dx",
        "path_custody",
        "python_environment_identity",
        "python_environment_location",
        "python_environment_custody",
        "python_external_custody",
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
        "file_hashing",
        "exact_json",
        "file_publication",
        "portable_paths",
    )
    root = Path(__file__).resolve().parent
    # Standalone probes import the package bootstrap too. Its version resolver
    # consults this source-tree marker even when the proof's cwd is another repo.
    return (
        *(root / f"{name}.py" for name in names),
        root.parent / "sitecustomize.py",
        root.parent.parent / "pyproject.toml",
    )


def _main() -> None:
    args = _arguments
    assert args is not None
    if args.locate_active_environment:
        payload = locate_current_python_environment()
    else:
        from molt.python_file_node_custody import PythonFileCaptureContext
        from molt.python_capture import python_capture_payload

        context = PythonFileCaptureContext(hash_workers=args.hash_workers)
        if args.capture_runtime:
            payload = capture_current_python_runtime(capture_context=context)
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
            )
        if args.with_custody:
            payload = python_capture_payload(payload, context)
    print(json.dumps(payload, allow_nan=False, separators=(",", ":"), sort_keys=True))


if __name__ == "__main__":
    try:
        _main()
    except (OSError, ValueError) as exc:
        _probe_error(str(exc))
