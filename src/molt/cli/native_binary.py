from __future__ import annotations

import os
import platform
from pathlib import Path
import shutil
import signal
import subprocess
import sys

from molt.cli.command_runtime import _run_completed_command


class _NativeBinaryInvalid(Exception):
    """A produced native binary is structurally invalid (bad object format)
    or is rejected/killed by the OS loader on a smoke probe.

    Raising this fails the build loudly. It exists to make the binary-
    corruption class (e.g. a mis-applied relocation flipping the Mach-O magic
    `0xfeedfacf` -> `0xfeedface`, yielding a kernel-SIGKILLed binary that still
    "linked successfully") non-shippable: a link that returns 0 but emits a
    structurally broken artifact must not be reported as a success.
    """


# Expected leading magic bytes per object format. Mach-O and PE are exact
# 4-/2-byte signatures; ELF is the 4-byte `\x7fELF` regardless of class/endian.
# 64-bit Mach-O (the only valid form molt emits for arm64/x86_64) is
# `0xFEEDFACF` little-endian -> bytes CF FA ED FE. The 32-bit magic
# `0xFEEDFACE` -> CE FA ED FE is the exact corruption signature this check
# rejects.
_MACHO64_MAGIC_LE = bytes((0xCF, 0xFA, 0xED, 0xFE))


_MACHO64_MAGIC_BE = bytes((0xFE, 0xED, 0xFA, 0xCF))


_MACHO32_MAGIC_LE = bytes((0xCE, 0xFA, 0xED, 0xFE))


_MACHO32_MAGIC_BE = bytes((0xFE, 0xED, 0xFA, 0xCE))


_MACHO_FAT_MAGICS = (
    bytes((0xCA, 0xFE, 0xBA, 0xBE)),  # FAT_MAGIC
    bytes((0xBE, 0xBA, 0xFE, 0xCA)),  # FAT_CIGAM
    bytes((0xCA, 0xFE, 0xBA, 0xBF)),  # FAT_MAGIC_64
    bytes((0xBF, 0xBA, 0xFE, 0xCA)),  # FAT_CIGAM_64
)


_ELF_MAGIC = bytes((0x7F, 0x45, 0x4C, 0x46))  # \x7fELF


_PE_MZ_MAGIC = bytes((0x4D, 0x5A))  # MZ


def _expected_binary_format_for_target(target_triple: str | None) -> str:
    """Map a target triple (or the host, when None) to its object format:
    one of 'macho', 'elf', 'pe'."""
    triple = (target_triple or "").lower()
    if "apple" in triple or "darwin" in triple or "macos" in triple or "ios" in triple:
        return "macho"
    if "windows" in triple or "msvc" in triple or triple.endswith("-pc"):
        return "pe"
    if (
        "linux" in triple
        or "android" in triple
        or "freebsd" in triple
        or "netbsd" in triple
        or "openbsd" in triple
        or "wasi" in triple
        or "none" in triple
    ):
        return "elf"
    if triple:
        # Unknown explicit triple: default to ELF (the GNU/SysV default) rather
        # than guessing the host format — cross targets are almost always ELF.
        return "elf"
    if sys.platform == "darwin":
        return "macho"
    if sys.platform == "win32":
        return "pe"
    return "elf"


def _validate_native_binary_format(binary: Path, target_triple: str | None) -> None:
    """Validate the produced native binary's object-file magic against the
    format the target expects, raising `_NativeBinaryInvalid` on mismatch.

    This is the structural half of the build-time output validity check: it is
    intentionally cheap (reads the first bytes) and runs on every native link so
    a relocation that corrupts the header (the resolver-corruption class) cannot
    pass as a successful build.
    """
    if not binary.exists():
        raise _NativeBinaryInvalid(
            f"native link reported success but produced no output at {binary}"
        )
    try:
        with binary.open("rb") as handle:
            head = handle.read(4)
    except OSError as exc:
        raise _NativeBinaryInvalid(
            f"could not read produced binary {binary} for validity check: {exc}"
        ) from exc
    if len(head) < 4:
        raise _NativeBinaryInvalid(
            f"produced binary {binary} is truncated ({len(head)} bytes); "
            f"expected a valid object header"
        )
    fmt = _expected_binary_format_for_target(target_triple)
    if fmt == "macho":
        if head in (_MACHO64_MAGIC_LE, _MACHO64_MAGIC_BE):
            return
        if head in _MACHO_FAT_MAGICS:
            return  # universal binary container — its slices are validated by the loader
        if head in (_MACHO32_MAGIC_LE, _MACHO32_MAGIC_BE):
            raise _NativeBinaryInvalid(
                f"produced Mach-O {binary} has 32-bit magic "
                f"{head.hex()} (0xFEEDFACE) — corrupt header for a 64-bit "
                f"target (a mis-applied relocation flipped CF->CE). "
                f"This binary would be SIGKILLed by the kernel; failing the build."
            )
        raise _NativeBinaryInvalid(
            f"produced binary {binary} is not a valid Mach-O: leading bytes "
            f"{head.hex()} (want 0xFEEDFACF / CF FA ED FE)"
        )
    if fmt == "elf":
        if head == _ELF_MAGIC:
            return
        raise _NativeBinaryInvalid(
            f"produced binary {binary} is not a valid ELF: leading bytes "
            f"{head.hex()} (want \\x7fELF / 7F 45 4C 46)"
        )
    if fmt == "pe":
        if head[:2] == _PE_MZ_MAGIC:
            return
        raise _NativeBinaryInvalid(
            f"produced binary {binary} is not a valid PE/COFF: leading bytes "
            f"{head[:2].hex()} (want MZ / 4D 5A)"
        )
    # Unreachable: _expected_binary_format_for_target only returns the 3 formats.
    raise _NativeBinaryInvalid(
        f"internal: unknown expected format {fmt!r} for target {target_triple!r}"
    )


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
    """True when a binary for `target_triple` can run on the build host, so a
    smoke-exec probe is meaningful. Conservative: only returns True when the
    target's OS+arch both match the host (or the target is None = host build)."""
    if target_triple is None:
        return True
    triple = target_triple.lower()
    if "wasm" in triple or "wasi" in triple:
        return False
    host_os_ok = (
        (sys.platform == "darwin" and ("apple" in triple or "darwin" in triple))
        or (sys.platform.startswith("linux") and "linux" in triple)
        or (sys.platform == "win32" and ("windows" in triple or "msvc" in triple))
    )
    if not host_os_ok:
        return False
    machine = platform.machine().lower()
    host_is_arm64 = machine in ("arm64", "aarch64")
    host_is_x86_64 = machine in ("x86_64", "amd64")
    target_is_arm64 = triple.startswith("aarch64") or triple.startswith("arm64")
    target_is_x86_64 = triple.startswith("x86_64") or triple.startswith("amd64")
    if host_is_arm64 and target_is_arm64:
        return True
    if host_is_x86_64 and target_is_x86_64:
        return True
    # macOS arm64 can run x86_64 under Rosetta 2; treat as runnable.
    if sys.platform == "darwin" and host_is_arm64 and target_is_x86_64:
        return True
    return False


def _assert_native_binary_valid(binary: Path, target_triple: str | None) -> None:
    """Build-time output validity gate (cross-platform).

    Runs after a native link reports success and validates the produced binary's
    object-file magic against the target format (Mach-O 0xFEEDFACF / ELF / PE).
    This is deterministic and side-effect-free, and it rejects the resolver-
    corruption class (a header flipped to the 32-bit magic 0xFEEDFACE) that
    otherwise links "successfully" and is then SIGKILLed by the kernel. Failure
    raises `_NativeBinaryInvalid`, which the caller turns into a loud build
    failure.

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
    """Return an error string when a purported Mach-O binary is obviously invalid.

    This is a hard correctness check: we should not claim a successful build when the
    linker returned 0 but produced a non-Mach-O output file (observed as all-zero data
    artifacts under some linker/toolchain configurations).
    """

    if sys.platform != "darwin":
        return None
    try:
        header = binary_path.read_bytes()[:4]
    except OSError as exc:
        return f"Failed to read output binary: {exc}"
    if len(header) < 4:
        return "Output binary is truncated (missing Mach-O header)."
    magic = int.from_bytes(header, "big", signed=False)
    # Accept thin and fat Mach-O headers (32/64-bit). We only need to reject
    # obviously-invalid outputs (e.g. all-zero placeholders).
    if magic in {
        0xFEEDFACE,
        0xFEEDFACF,
        0xCEFAEDFE,
        0xCFFAEDFE,
        0xCAFEBABE,
        0xBEBAFECA,
    }:
        return None
    return f"Output binary is not Mach-O (header=0x{magic:08x})."
