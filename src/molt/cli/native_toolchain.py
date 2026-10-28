from __future__ import annotations

from molt.compilation_target import (
    native_target_spec,
    target_triple as project_target_triple,
    Target,
)

import contextlib
from molt import file_publication
from molt.artifact_publication import discard_staged_output
import os
import platform
import subprocess
import sys
from pathlib import Path
from collections.abc import Callable

from molt.cli.compiler_metadata import _compiler_root
from molt.cli.config_resolution import _coerce_bool
from molt.cli.link_fingerprints import FinalLinkReceiptRequest
from molt.native_target import NativeTargetSpec, resolve_native_target_spec
from molt.cli.command_runtime import _run_completed_command


def _codesign_binary(binary_path: Path, *, identifier: str) -> None:
    """Ad-hoc codesign a binary on macOS under a caller-fixed identifier.

    Without ``--identifier`` codesign derives the signing identifier from the
    file name, so signing a uniquely named staging file embeds that random name
    and makes the published bytes differ on every build. A failed signature is
    a publication failure: an unsigned or stale-signed arm64 binary is killed
    by the kernel at its first exec.
    """
    if sys.platform != "darwin":
        return
    if not identifier:
        raise ValueError("codesign identifier must be non-empty")
    try:
        result = _run_completed_command(
            ["codesign", "--force", "--sign", "-", "--identifier", identifier]
            + [str(binary_path)],
            capture_output=True,
            env=None,
            cwd=binary_path.parent,
            memory_guard_prefix="MOLT_BUILD",
            timeout=10,
        )
    except subprocess.TimeoutExpired as exc:
        raise OSError(f"codesign timed out for {binary_path}") from exc
    if result.returncode != 0:
        detail = (result.stderr or result.stdout or "").strip()
        raise OSError(
            f"codesign failed for {binary_path} (exit {result.returncode}): {detail}"
        )


def _run_bolt_post_link(
    *,
    bolt_requested: bool,
    bolt_training_cmd: str | None,
    target: Target,
    input_binary: str | None = None,
    output: str | None,
    out_dir: str | None,
    build_rc: int,
    json_output: bool,
    receipt: FinalLinkReceiptRequest | None = None,
    link_selection: tuple[Path, Path] | None = None,
    link_input_artifacts: dict[str, tuple[Path, Path]] | None = None,
    validate_inputs: Callable[[], None] | None = None,
) -> int:
    """Run BOLT post-link optimization after a successful native build.

    Returns 0 when BOLT was not requested or ran successfully, or a nonzero
    return code on failure.
    """
    if not bolt_requested:
        return 0
    if build_rc != 0:
        return 0  # build already failed — skip BOLT

    owned_input: Path | None = None
    owned_selection: Path | None = None
    owned_captures: list[Path] = []
    try:
        for stage, destination in (link_input_artifacts or {}).values():
            if (
                file_publication.is_owned_staged_file_path(
                    stage, destination, purpose="link-inputs", suffix=stage.suffix
                )
                and stage.is_file()
                and stage.stat().st_nlink == 1
            ):
                owned_captures.append(stage)
        # Determine the output binary path.  When --output is given we can
        # resolve it directly; otherwise BOLT requires it explicitly because
        # the default output path lives inside internal build state.
        if output:
            output_path = Path(output).expanduser()
            binary_path = (
                Path(input_binary).expanduser() if input_binary else output_path
            )
            if not binary_path.is_absolute():
                base = Path(out_dir) if out_dir else Path.cwd()
                binary_path = base / binary_path
            if not output_path.is_absolute():
                base = Path(out_dir) if out_dir else Path.cwd()
                output_path = base / output_path
        else:
            if not json_output:
                print(
                    "Error: --bolt requires an explicit --output path so BOLT "
                    "can locate the binary.",
                    file=sys.stderr,
                )
            return 1

        if not binary_path.exists():
            msg = f"BOLT: output binary not found at {binary_path}"
            if not json_output:
                print(msg, file=sys.stderr)
            return 1

        try:
            binary_path = file_publication.canonical_file_leaf(
                binary_path, create_parent=False, role="BOLT input"
            )
            output_path = file_publication.canonical_file_leaf(
                output_path, create_parent=False, role="BOLT output"
            )
            if (
                file_publication.is_owned_staged_file_path(
                    binary_path,
                    output_path,
                    purpose="native-link",
                    suffix=output_path.suffix or ".tmp",
                )
                and binary_path.stat().st_nlink == 1
            ):
                owned_input = binary_path
            if link_selection is not None:
                selection = file_publication.canonical_file_leaf(
                    link_selection[0], create_parent=False, role="BOLT link selection"
                )
                destination = file_publication.canonical_file_leaf(
                    link_selection[1], create_parent=False, role="BOLT selection output"
                )
                if (
                    file_publication.is_owned_staged_file_path(
                        selection, destination, purpose="native-link"
                    )
                    and selection.stat().st_nlink == 1
                ):
                    owned_selection = selection
        except (OSError, ValueError) as exc:
            if not json_output:
                print(f"BOLT: {exc}", file=sys.stderr)
            return 1

        target_triple = project_target_triple(target)
        try:
            target_spec = native_target_spec(target)
            host_spec = resolve_native_target_spec(
                None,
                host_platform=sys.platform,
                host_arch=platform.machine(),
            )
        except RuntimeError as exc:
            if not json_output:
                print(f"BOLT: {exc}", file=sys.stderr)
            return 1
        if error := target_spec.bolt_support_error:
            if not json_output:
                print(f"BOLT: {error}", file=sys.stderr)
            return 1
        if (target_spec.os, target_spec.arch) != (host_spec.os, host_spec.arch):
            if not json_output:
                print(
                    "BOLT: cross-target training is unsupported because the "
                    "instrumented target binary must execute on the build host; "
                    f"target={target_spec.os}/{target_spec.arch}, "
                    f"host={host_spec.os}/{host_spec.arch}",
                    file=sys.stderr,
                )
            return 1

        # Locate the BOLT wrapper script.
        bolt_script = _compiler_root() / "tools" / "bolt_optimize.sh"
        if not bolt_script.exists():
            msg = f"BOLT script not found: {bolt_script}"
            if not json_output:
                print(msg, file=sys.stderr)
            return 1

        # Build the bolt command.
        bolt_cmd: list[str] = ["bash", str(bolt_script), str(binary_path)]
        if bolt_training_cmd:
            bolt_cmd.append(bolt_training_cmd)

        if not json_output:
            print(
                f"==> Running BOLT post-link optimization on {binary_path}...",
                file=sys.stderr,
            )

        try:
            bolt_proc = _run_completed_command(
                bolt_cmd,
                cwd=binary_path.parent,
                env=None,
                capture_output=True,
                memory_guard_prefix="MOLT_BUILD",
                timeout=300,  # 5 min ceiling
            )
        except FileNotFoundError:
            if not json_output:
                print("BOLT: bash not found", file=sys.stderr)
            return 1
        except subprocess.TimeoutExpired:
            if not json_output:
                print("BOLT: optimization timed out (300s)", file=sys.stderr)
            return 1

        if bolt_proc.returncode != 0:
            if not json_output:
                stderr_text = (
                    bolt_proc.stderr
                    if isinstance(bolt_proc.stderr, str)
                    else (
                        bolt_proc.stderr.decode("utf-8", errors="replace")
                        if bolt_proc.stderr
                        else ""
                    )
                )
                if stderr_text:
                    print(stderr_text, file=sys.stderr)
                print("BOLT optimization failed", file=sys.stderr)
            return bolt_proc.returncode

        # Replace the original binary with the BOLT-optimized one.
        bolt_binary = Path(f"{binary_path}.bolt")
        if not bolt_binary.is_file():
            if not json_output:
                print(
                    f"BOLT: optimizer exited successfully but did not produce {bolt_binary}",
                    file=sys.stderr,
                )
            return 1
        from molt.cli.build_results import _finalize_native_link_candidate

        finalize_error = _finalize_native_link_candidate(
            candidate=bolt_binary,
            output_binary=output_path,
            target_triple=target_triple,
            strip=os.environ.get("MOLT_KEEP_SYMBOLS") != "1",
            receipt=receipt,
            link_selection=link_selection,
            link_input_artifacts=link_input_artifacts,
            validate_inputs=validate_inputs,
        )
        if finalize_error is not None:
            if not json_output:
                print(f"BOLT: {finalize_error}", file=sys.stderr)
            return 1
        if not json_output:
            print(
                f"==> BOLT-optimized binary installed: {output_path}",
                file=sys.stderr,
            )

        return 0
    finally:
        if owned_input is not None:
            # The native-link producer issued this destination-bound private
            # input. Its derived optimizer output shares that private lifetime.
            bolt_output = Path(f"{owned_input}.bolt")
            with contextlib.suppress(OSError, ValueError):
                bolt_output = file_publication.canonical_file_leaf(
                    bolt_output, create_parent=False, role="BOLT candidate"
                )
                if bolt_output.is_file() and bolt_output.stat().st_nlink == 1:
                    discard_staged_output(bolt_output)
            discard_staged_output(owned_input)
        if owned_selection is not None:
            discard_staged_output(owned_selection)
        for stage in owned_captures:
            discard_staged_output(stage)


def _append_darwin_runtime_frameworks(
    args: list[str],
    *,
    target: NativeTargetSpec,
) -> None:
    """Append runtime frameworks to an already selected native link driver.

    The driver owns SDK and cross-target search paths; this projection never
    selects another SDK beside that choice.
    """
    if target.os == "macos":
        args.extend(["-framework", "Security", "-framework", "CoreFoundation"])
        if _coerce_bool(os.environ.get("MOLT_RUNTIME_GPU_METAL"), False):
            args.extend(["-framework", "Metal", "-lobjc"])
        if _coerce_bool(os.environ.get("MOLT_RUNTIME_GPU_WEBGPU"), False):
            args.extend(
                [
                    "-framework",
                    "Metal",
                    "-framework",
                    "Foundation",
                    "-framework",
                    "QuartzCore",
                    "-framework",
                    "AppKit",
                    "-lobjc",
                ]
            )
