from __future__ import annotations

import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys

from molt.cli.command_runtime import _run_completed_command
from molt.cli.native_link_plan import resolve_native_target_spec
from molt.native_artifact_header import (
    LINKED_IMAGE_KINDS,
    NativeArtifactError,
    read_native_artifact,
)
from molt.native_target_shape import NativeObjectFormat, native_artifact_shape


class _NativeBinaryInvalid(Exception):
    """A produced native binary is structurally invalid (bad object format)
    or is rejected/killed by the OS loader on a smoke probe.

    Raising this fails the build loudly. It exists to make the binary-
    corruption class (e.g. a mis-applied relocation flipping the Mach-O magic
    `0xfeedfacf` -> `0xfeedface`, yielding a kernel-SIGKILLed binary that still
    "linked successfully") non-shippable: a link that returns 0 but emits a
    structurally broken artifact must not be reported as a success.
    """


def _expected_binary_format_for_target(target_triple: str | None) -> str:
    target = resolve_native_target_spec(target_triple)
    return (
        "pe"
        if target.object_format is NativeObjectFormat.COFF
        else target.object_format.value
    )


def _validate_native_binary_format(binary: Path, target_triple: str | None) -> None:
    """Validate the linked header's format, image kind and exact target shape.

    ELF ET_DYN is a linked-image category (shared library or PIE), not an
    executable-loader claim. Optional smoke execution remains a separate proof.
    """
    try:
        target = resolve_native_target_spec(target_triple)
        shape = native_artifact_shape(
            target.arch, target_triple=target.triple, object_format=target.object_format
        )
        read_native_artifact(binary).admit(
            object_format=target.object_format,
            kinds=LINKED_IMAGE_KINDS,
            shape=shape,
            exact_target=True,
        )
    except (OSError, NativeArtifactError, RuntimeError) as exc:
        raise _NativeBinaryInvalid(
            f"Produced native binary {binary} failed header/target admission: {exc}"
        ) from exc


def validate_native_binary_architecture(binary: Path, target_triple: str) -> None:
    """Release-consumer projection of the same complete target admission."""
    _validate_native_binary_format(binary, target_triple)


def _smoke_probe_native_binary(binary: Path, target_triple: str | None) -> None:
    """Execute the produced binary briefly to confirm the OS loader accepts it,
    raising `_NativeBinaryInvalid` if the kernel rejects/kills it.

    Only runs when the target is host-executable (no cross-compile) and the
    host is not Windows (where a benign exec probe is unreliable). A structurally
    valid header can still be unloadable (bad load commands, a corrupt segment),
    so this catches loader-level corruption the magic check alone misses. The
    probe sends `MOLT_BUILD_VALIDITY_PROBE=1` so the program *may* exit early
    cooperatively; absent that, any non-signal termination (including the normal
    program running to completion) is accepted — only a loader-level kill
    (SIGKILL/SIGSEGV/SIGBUS/SIGILL on launch, or an Exec-format OSError) fails
    the build.
    """
    if not _target_is_host_executable(target_triple):
        return
    if sys.platform == "win32":
        return
    try:
        proc = _run_completed_command(
            [str(binary)],
            timeout=20,
            env={**os.environ, "MOLT_BUILD_VALIDITY_PROBE": "1"},
            cwd=binary.parent,
            capture_output=True,
            memory_guard_prefix="MOLT_BUILD",
            input="",
        )
    except OSError as exc:
        # ENOEXEC / "Exec format error" — the loader cannot run this image.
        raise _NativeBinaryInvalid(
            f"produced binary {binary} is not executable by the OS loader: {exc}"
        ) from exc
    except subprocess.TimeoutExpired:
        # The program ran (loaded fine) and merely outlived the probe window;
        # loading succeeded, which is all this probe verifies.
        return
    rc = proc.returncode
    # A negative returncode means the process was terminated by a signal.
    # Loader-level rejection manifests as SIGKILL (9, the Mach-O-magic-corruption
    # symptom), SIGSEGV (11), SIGBUS (10), or SIGILL (4) immediately on launch.
    if rc is not None and rc < 0:
        sig = -rc
        loader_fatal = {
            getattr(signal, "SIGKILL", 9),
            getattr(signal, "SIGSEGV", 11),
            getattr(signal, "SIGBUS", 10),
            getattr(signal, "SIGILL", 4),
        }
        if sig in loader_fatal:
            try:
                signame = signal.Signals(sig).name
            except ValueError:
                signame = f"signal {sig}"
            raise _NativeBinaryInvalid(
                f"produced binary {binary} was killed by {signame} on a smoke "
                f"probe — the OS loader rejected the image (corrupt header / "
                f"load commands). Failing the build."
            )


def _target_is_host_executable(target_triple: str | None) -> bool:
    """Only identical host/target facts justify a local smoke probe.

    Translation (for example Rosetta) must have explicit capability evidence;
    neither host architecture nor target spelling proves it is installed.
    """
    try:
        host = resolve_native_target_spec(None)
        target = resolve_native_target_spec(target_triple)
        host_shape = native_artifact_shape(host.arch, object_format=host.object_format)
        target_shape = native_artifact_shape(
            target.arch, target_triple=target.triple, object_format=target.object_format
        )
    except RuntimeError:
        return False
    return target.os == host.os and target_shape == host_shape


def _assert_native_binary_valid(binary: Path, target_triple: str | None) -> None:
    """Build-time output validity gate (cross-platform).

    Runs after a native link reports success and admits the complete fixed
    header, linked-image kind, and exact architecture/ABI shape through the
    shared native artifact authority. This is deterministic and side-effect-free;
    loader acceptance is a distinct optional proof. Failure raises
    `_NativeBinaryInvalid`, which the caller turns into a loud build failure.

    The deeper *smoke-exec* loader probe (`_smoke_probe_native_binary`) actually
    runs the produced image, so it can execute the user program's `main()` side
    effects at build time; it is therefore opt-in via `MOLT_BUILD_SMOKE_EXEC=1`
    (the validity gate in `tools/verify_native_binary_valid.sh` runs its own
    disposable corpus binaries directly, so it does not need the in-build probe).

    `MOLT_SKIP_BINARY_VALIDITY_CHECK=1` disables the whole gate (diagnostics /
    bring-up only).
    """
    if os.environ.get("MOLT_SKIP_BINARY_VALIDITY_CHECK") == "1":
        return
    _validate_native_binary_format(binary, target_triple)
    if os.environ.get("MOLT_BUILD_SMOKE_EXEC") == "1":
        _smoke_probe_native_binary(binary, target_triple)


def _darwin_binary_imports_validation_error(binary_path: Path) -> str | None:
    if sys.platform != "darwin":
        return None
    dyld_info = shutil.which("dyld_info")
    if dyld_info is None or not binary_path.exists():
        return None
    try:
        proc = _run_completed_command(
            [dyld_info, str(binary_path)],
            capture_output=True,
            timeout=10.0,
            env=None,
            cwd=binary_path.parent,
            memory_guard_prefix="MOLT_BUILD",
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    combined = "\n".join(
        part.strip() for part in (proc.stdout, proc.stderr) if part and part.strip()
    )
    needle = combined.lower()
    if "unknown imports_format" in needle or "unknown imports format" in needle:
        return combined or "dyld_info reported unknown imports format."
    return None


def _darwin_binary_magic_error(binary_path: Path) -> str | None:
    """Check image structure without guessing the cross-link target from the host."""
    if sys.platform != "darwin":
        return None
    try:
        read_native_artifact(binary_path).validate_format_and_kind(
            object_format=NativeObjectFormat.MACHO, kinds=LINKED_IMAGE_KINDS
        )
    except (OSError, NativeArtifactError) as exc:
        return str(exc)
    return None
