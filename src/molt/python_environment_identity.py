"""Public facade and standalone probe for exact CPython environment custody."""

from __future__ import annotations

import importlib.machinery
import importlib.util
import json
import sys
from collections.abc import Mapping, Sequence
from io import TextIOWrapper
from pathlib import Path
from types import ModuleType
from typing import Any, NoReturn, cast


if __name__ == "__main__":
    # Probe receivers decode diagnostic pipes as UTF-8, including refusals
    # before package admission. Windows redirected stderr otherwise uses the
    # locale codec and can escape or corrupt a selected Unicode package home.
    # This is the standalone wire contract, never an imported-facade side effect.
    cast(TextIOWrapper, sys.stderr).reconfigure(
        encoding="utf-8", errors="backslashreplace"
    )


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
    modes.add_argument(
        "--editable-project", nargs=2, metavar=("ENVIRONMENT", "PROJECT")
    )
    modes.add_argument("--capture-environment", type=Path, metavar="ROOT")
    modes.add_argument("--capture-active-environment", action="store_true")
    parser.add_argument("--runtime-session", action="store_true")
    parser.add_argument("--wheel-tag-source", type=Path)
    parser.add_argument("--input-report-log-dir")
    parser.add_argument("--with-custody", action="store_true")
    parser.add_argument("--publish-bytecode", action="store_true")
    parser.add_argument("--hash-workers", type=int, default=1)
    parser.add_argument("--admit-virtualenv-bootstrap", action="store_true")
    parser.add_argument("--admit-site-bootstrap", action="append", default=[])
    parser.add_argument("--admit-external-root", type=Path, action="append", default=[])
    parser.add_argument("--admit-published-environment", action="store_true")
    parser.add_argument("--development-checkout", type=Path)
    parser.add_argument("--development-python")
    parser.add_argument("--development-group", action="append", default=[])
    args = parser.parse_args()
    if (
        args.development_checkout is not None
        or args.development_python is not None
        or args.development_group
    ) and (not args.admit_published_environment or args.development_checkout is None):
        parser.error("development intent requires published admission and a checkout")
    if args.admit_published_environment and (
        not (args.capture_environment or args.capture_active_environment)
        or args.publish_bytecode
    ):
        parser.error("published admission requires a read-only environment capture")
    if args.wheel_tag_source is not None and (
        not args.capture_runtime or args.with_custody or args.runtime_session
    ):
        parser.error("wheel tag capture requires a plain runtime capture")
    if args.publish_bytecode and (
        not args.capture_environment or args.with_custody or args.runtime_session
    ):
        parser.error(
            "bytecode publication requires one unpublished environment capture"
        )
    admissions = (
        args.admit_virtualenv_bootstrap
        or args.admit_site_bootstrap
        or args.admit_external_root
    )
    if (args.locate_active_environment or args.editable_project) and (
        args.with_custody or args.hash_workers != 1 or admissions
    ):
        parser.error("location accepts no capture or environment admission options")
    if args.runtime_selection and (
        args.with_custody or args.hash_workers != 1 or admissions
    ):
        parser.error("runtime selection accepts no capture or admission options")
    if args.capture_runtime and admissions:
        parser.error("runtime capture accepts no environment admission options")
    if args.runtime_session and not args.capture_runtime:
        parser.error("runtime session requires runtime capture")
    return args


_arguments = _parse_arguments() if __name__ == "__main__" else None
if _arguments is not None and (
    _arguments.locate_active_environment or _arguments.editable_project
):
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
        SOURCE_BUILD_ENVIRONMENT_MANIFEST,
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


class PythonIdentityVerifier:
    """One owned read namespace and serial guarded-child lifetime.

    Every launch resets closure before the core executor can create a child.
    Unknown closure poisons this owner permanently: a swallowed exception or a
    later successful child can neither authorize another launch nor cleanup.
    Pure argv helpers receive ``prefix`` only; they never acquire this owner.
    """

    def __init__(self, *, prefix: str = "molt-python-verifier-") -> None:
        from threading import Lock
        from molt.temporary_artifacts import OwnedTemporaryDirectory

        self._allocation = OwnedTemporaryDirectory(prefix=prefix)
        self.prefix = Path(self._allocation.name).resolve(strict=True)
        self._launch_lock = Lock()
        self._entered = False
        self._finished = False
        self._closed = True  # No child has started.
        self._failure: BaseException | None = None
        self.last_guarded_result: Any = None

    def __enter__(self) -> "PythonIdentityVerifier":
        if self._entered or self._finished:
            raise RuntimeError("Python verifier owner cannot be reentered")
        self._entered = True
        return self

    def __exit__(
        self, kind: object, error: BaseException | None, traceback: object
    ) -> None:
        del kind, traceback
        self._finished = True
        self._entered = False
        if self._launch_lock.locked():
            self._closed = False
            if self._failure is None:
                self._failure = RuntimeError(
                    "Python verifier exited during a child launch"
                )
        if not self._closed or self._failure is not None:
            self._allocation.retain()
            primary = self._failure if self._failure is not None else error
            if primary is None:
                primary = RuntimeError(
                    "Python verifier has no proven process-tree closure"
                )
                self._failure = primary
            primary.add_note(
                "Python verifier cache retained pending process-tree closure: "
                f"{self.prefix}"
            )
            if error is not primary:
                raise primary
            return
        try:
            self._allocation.cleanup()
        except BaseException as cleanup_error:
            if error is None:
                raise
            error.add_note(
                f"Python verifier scratch cleanup also failed at {self.prefix}: "
                f"{type(cleanup_error).__name__}: {cleanup_error}"
            )

    def run(
        self,
        command: Sequence[str],
        *,
        memory_guard_prefix: str,
        env: Mapping[str, str] | None = None,
        cwd: str | Path | None = None,
        capture_output: bool = False,
        input: str | bytes | None = None,
        timeout: float | None = None,
        text: bool | None = True,
        check: bool = False,
        encoding: str | None = None,
        errors: str | None = None,
        guard_source_root: Path | None = None,
        progress_label: str | None = None,
    ) -> Any:
        """Use the core guard explicitly; metadata flags cannot bypass it."""
        from molt import process_guard
        from molt.process_environment import ProcessEnvironment

        if not self._entered or self._finished:
            raise RuntimeError("Python verifier child requires its active owner")
        if self._failure is not None:
            raise self._failure
        if not isinstance(memory_guard_prefix, str) or not memory_guard_prefix.strip():
            raise ValueError("Python verifier requires an explicit memory-guard prefix")
        if isinstance(command, (str, bytes)):
            raise TypeError("Python verifier child requires typed argv")
        if not command:
            raise ValueError("Python verifier child command must not be empty")
        if not self._launch_lock.acquire(blocking=False):
            raise RuntimeError("Python verifier child launches must be serial")
        try:
            if not self._closed:
                raise RuntimeError(
                    "Python verifier previous child has no proven closure"
                )
            try:
                self._closed = False
                self.last_guarded_result = None
                # Detach before the core executor can create a child; only a
                # terminal closed result permits explicit context cleanup.
                self._allocation.retain()
                result = process_guard.run_completed_command(
                    command,
                    memory_guard_prefix=memory_guard_prefix,
                    env=None if env is None else ProcessEnvironment(env),
                    cwd=cwd,
                    capture_output=capture_output,
                    input=input,
                    timeout=timeout,
                    text=text,
                    check=check,
                    encoding=encoding,
                    errors=errors,
                    guard_source_root=guard_source_root,
                    progress_label=progress_label,
                )
                self.last_guarded_result = result
                self._closed = getattr(result, "descendants_closed", None) is True
                if not self._closed:
                    primary = RuntimeError(
                        "Python verifier child returned without process-tree closure"
                    )
                    setattr(primary, "guarded_result", result)
                    self._failure = primary
                    raise primary
                return result
            except BaseException as primary:
                self.last_guarded_result = getattr(primary, "guarded_result", None)
                self._closed = (
                    self.last_guarded_result is not None
                    and getattr(self.last_guarded_result, "descendants_closed", None)
                    is True
                )
                if not self._closed:
                    self._failure = primary
                raise
        finally:
            self._launch_lock.release()


def python_identity_verifier_arguments(verifier_bytecode_prefix: Path) -> list[str]:
    """Select the empty owned read namespace before arbitrary verifier imports.

    Isolation ignores PYTHONDONTWRITEBYTECODE, so the no-write policy must be
    an interpreter flag before either site startup or the authority imports.
    The caller retains the fresh prefix's owner until child closure.
    """
    from molt.python_file_node_custody import _is_junction

    prefix = Path(verifier_bytecode_prefix).absolute()
    if (
        not prefix.is_dir()
        or prefix.is_symlink()
        or _is_junction(prefix)
        or any(prefix.iterdir())
    ):
        raise PythonEnvironmentIdentityError(
            "Python verifier requires a fresh empty owned bytecode prefix"
        )
    return [
        "-B",
        "-I",
        "-X",
        f"pycache_prefix={prefix}",
    ]


def python_identity_probe_arguments(
    arguments: Sequence[str],
    *,
    verifier_bytecode_prefix: Path,
    no_site: bool = False,
    source_root: Path | None = None,
) -> list[str]:
    """Select the reader explicitly; external-root grants never choose code."""
    from molt import input_report

    log_dir = input_report.worker_log_dir()
    probe_path = (
        _python_capture_package_path(source_root) / "python_environment_identity.py"
    ).resolve(strict=True)
    return [
        *python_identity_verifier_arguments(verifier_bytecode_prefix),
        *(["-S"] if no_site else []),
        str(probe_path),
        *arguments,
        *(["--input-report-log-dir", log_dir] if log_dir is not None else []),
    ]


def _python_capture_package_path(source_root: Path | None) -> Path:
    """Select reader code independently of grants to captured external roots."""
    return (
        Path(__file__).resolve().parent
        if source_root is None
        else source_root / "src" / "molt"
    )


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
        "process_model",
        "windows_process_snapshot",
        "backend_executable_names",
        "host_tool_products",
        "windows_process_api",
        "dx",
        "checkout_state",
        "checkout_provisioning",
        "build_state_layout",
        "backend_daemon_paths",
        "host_capacity",
        "cargo_execution_policy",
        "flag_strings",
        "process_environment",
        "default_paths",
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
        "python_interpreter",
        "target_python",
        "target_python_policy",
        "_python_release_generated",
        "verified_subset",
        "release_matrix",
        "compiler_analysis/__init__",
        "compiler_analysis/python_syntax",
        "source_build_environment",
        "source_build_environment_schema",
        "source_build_requirements",
        "process_guard",
        "toolchain_identity",
        "input_report",
        "llvm_linker_roles",
        "file_hashing",
        "exact_json",
        "file_publication",
        "portable_paths",
    )
    root = _python_capture_package_path(source_root)
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
            identity = (
                payload["identity"]
                if payload.get("schema") == PYTHON_CAPTURE_SCHEMA
                else payload
            )
            responses.write(str(identity["runtime_closure_sha256"]) + "\n")
            responses.flush()
    finally:
        context.close()


def _main() -> None:
    args = _arguments
    assert args is not None
    from molt import input_report

    input_report.worker_initializer(args.input_report_log_dir)
    if args.editable_project:
        from molt.python_environment_location import locate_editable_project

        root, project = args.editable_project
        payload = locate_editable_project(Path(root), project)
    elif args.locate_active_environment:
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
                if args.wheel_tag_source is not None:
                    from molt.python_uv_lock_identity import capture_python_wheel_target

                    payload = {
                        "runtime": payload,
                        "wheel_target": capture_python_wheel_target(
                            args.wheel_tag_source, payload, context
                        ),
                    }
                    context.verify()
            else:
                from molt.python_environment_location import _active_environment_prefix

                root = args.capture_environment or _active_environment_prefix()
                bootstrap = list(args.admit_site_bootstrap)
                if args.admit_virtualenv_bootstrap:
                    bootstrap.extend(virtualenv_site_bootstrap_relative_paths(root))
                payload = capture_current_python_environment(
                    root,
                    excluded_relative_paths=(SOURCE_BUILD_ENVIRONMENT_MANIFEST,),
                    admitted_site_bootstrap_paths=bootstrap,
                    admitted_external_roots=args.admit_external_root,
                    capture_context=context,
                    with_custody=args.with_custody,
                    publish_bytecode=args.publish_bytecode,
                )
                if args.admit_published_environment:
                    # The source-bound guardian has no site dependencies. This
                    # selected-image child already captured their exact bytes;
                    # keep all recipe/lock/WHEEL checks in their existing owner.
                    from molt.source_build_environment import (
                        admit_published_environment,
                    )

                    realized = (
                        cast(dict[str, object], payload["identity"])
                        if args.with_custody
                        else payload
                    )
                    if args.development_checkout is not None:
                        from molt.source_build_environment import (
                            load_development_environment,
                        )

                        load_development_environment(
                            root,
                            repo_root=args.development_checkout,
                            python=args.development_python,
                            groups=tuple(args.development_group),
                        )
                    admit_published_environment(root, realized)
                    context.verify()
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
