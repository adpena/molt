#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
import sys
import time
from collections.abc import Callable, Mapping, Sequence
from pathlib import Path

TOOLS_ROOT = Path(__file__).resolve().parent
if str(TOOLS_ROOT) not in sys.path:
    sys.path.insert(0, str(TOOLS_ROOT))
SRC_ROOT = TOOLS_ROOT.parent / "src"
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

from molt.cli.runtime_wasm_generation import (  # noqa: E402
    RuntimeWasmExpectedPair,
    RuntimeWasmGeneration,
    read_runtime_wasm_generation,
)
from molt.toolchain_identity import (  # noqa: E402
    StableRegularFileChangedError,
    StableRegularFileError,
    StableRegularFileIdentity,
    snapshot_stable_regular_file,
    stable_regular_file_identity,
    verify_stable_regular_file_identity,
)
from molt.link_outputs import validate_link_output_paths, wasm_link_output_paths  # noqa: E402
from molt.cli.link_fingerprints import FinalLinkReceiptRequest  # noqa: E402
from molt.cli.python_source_closure import local_python_import_graph_transaction  # noqa: E402
from molt.cli.source_extension_link_requirements import (  # noqa: E402
    SourceExtensionLinkInput,
    SourceExtensionLinkRequirements,
    map_source_extension_link_inputs,
    read_source_extension_link_plan,
)
from molt.wasm_optimization import WASM_OPT_LEVELS  # noqa: E402
import wasm_link_command as _link_command  # noqa: E402
import wasm_link_native_inputs as _native_inputs  # noqa: E402
from wasm_link_fact_provider import make_rust_wasm_facts_provider  # noqa: E402
from wasm_link_format import WASM_MAGIC, WASM_VERSION  # noqa: E402
from wasm_link_pipeline import (  # noqa: E402
    RuntimeLinkInputRole,
    run_wasm_ld_with_custodied_inputs as _run_wasm_ld_with_custodied_inputs,
)
import wasm_link_runtime_data as _runtime_data  # noqa: E402


def _default_runtime_path() -> Path:
    env_root = os.environ.get("MOLT_WASM_RUNTIME_DIR")
    if env_root:
        return Path(env_root).expanduser() / "molt_runtime.wasm"
    ext_root = os.environ.get("MOLT_EXT_ROOT")
    external_root = Path(ext_root).expanduser() if ext_root else None
    if external_root is not None and external_root.is_dir():
        return external_root / "wasm" / "molt_runtime.wasm"
    return Path("wasm/molt_runtime.wasm")


def _default_dist_artifact_path(name: str) -> Path:
    ext_root = os.environ.get("MOLT_EXT_ROOT")
    external_root = Path(ext_root).expanduser() if ext_root else None
    if external_root is not None and external_root.is_dir():
        return external_root / "dist" / name
    return Path("dist") / name


def _default_input_path() -> Path:
    return _default_dist_artifact_path("output.wasm")


def _default_output_path() -> Path:
    return _default_dist_artifact_path("output_linked.wasm")


def _verify_runtime_generation(
    *,
    reloc: Path,
    shared: Path,
    generation_manifest: Path,
    expected_identity: Path,
) -> RuntimeWasmGeneration:
    """Verify both runtime members against caller-produced trusted identity."""

    for path in (reloc, shared, generation_manifest, expected_identity):
        if ".." in path.parts:
            raise SystemExit(f"Runtime custody path contains '..': {path}")
    try:
        expected_pair = RuntimeWasmExpectedPair.read(expected_identity)
    except ValueError as exc:
        raise SystemExit(f"Trusted runtime pair identity is invalid: {exc}") from exc
    generation = read_runtime_wasm_generation(
        generation_manifest,
        expected_shared_identity=expected_pair.shared,
        expected_reloc_identity=expected_pair.reloc,
    )
    if generation is None:
        raise SystemExit(
            "Runtime generation does not match the trusted caller identity: "
            f"{generation_manifest}"
        )
    if reloc.resolve(strict=False) != generation.reloc.resolve(
        strict=False
    ) or shared.resolve(strict=False) != generation.shared.resolve(strict=False):
        raise SystemExit(
            "Runtime link inputs are not the immutable members selected by "
            f"{generation_manifest}"
        )
    return generation


def _snapshot_link_input(
    source: Path,
    snapshot_root: Path,
    *,
    label: str,
    attempts: int = 100,
    retry_delay_seconds: float = 0.05,
    required_prefix: bytes | None = None,
    accept_path: Callable[[Path], bool] | None = None,
    expected_identity: StableRegularFileIdentity | None = None,
    expected_sha256: str | None = None,
    snapshot_directory: Path | None = None,
) -> Path:
    """Capture one complete immutable linker input from a mutable build path."""
    source = source.expanduser().absolute()
    snapshot_root = snapshot_root.expanduser().absolute()
    if expected_identity is not None and source != expected_identity.path:
        raise OSError(f"Linker input path crossed its trusted identity: {source}")
    if source.name.endswith(".runtime-wasm-member") and expected_identity is None:
        raise OSError(
            f"Immutable runtime member requires its trusted content identity: {source}"
        )
    if attempts <= 0:
        raise ValueError("linker input snapshot attempts must be positive")
    if retry_delay_seconds < 0:
        raise ValueError("linker input snapshot retry delay must be non-negative")
    if required_prefix is not None and not isinstance(required_prefix, bytes):
        raise TypeError("linker input required prefix must be bytes")
    last_observation = "unreadable"
    snapshot_root.mkdir(parents=True, exist_ok=True)
    snapshot_dir = (
        snapshot_root / label
        if snapshot_directory is None
        else snapshot_directory.expanduser().absolute()
    )
    if not snapshot_dir.is_relative_to(snapshot_root):
        raise OSError(
            f"Linker input snapshot directory escaped its custody root: {snapshot_dir}"
        )
    snapshot_dir.mkdir(parents=True, exist_ok=True)
    snapshot = snapshot_dir / source.name
    for _attempt in range(attempts):
        try:
            captured = snapshot_stable_regular_file(
                source,
                snapshot,
                label=f"linker input {label}",
                capture_prefix_bytes=len(required_prefix or b""),
            )
        except StableRegularFileChangedError as exc:
            last_observation = str(exc)
            if retry_delay_seconds:
                time.sleep(retry_delay_seconds)
            continue
        except (OSError, StableRegularFileError) as exc:
            raise OSError(f"Failed to snapshot linker input {label}: {exc}") from exc
        if expected_identity is not None and captured.source != expected_identity:
            captured.discard()
            raise OSError(
                f"Linker input crossed its trusted content identity: {source}"
            )
        if (
            captured.source.sha256 != captured.snapshot.sha256
            or captured.source.size != captured.snapshot.size
        ):
            captured.discard()
            raise OSError(f"Failed to attest linker input snapshot: {snapshot}")
        if expected_sha256 is not None and captured.source.sha256 != expected_sha256:
            captured.discard()
            raise ValueError(
                f"link requirement checksum mismatch for {source}: "
                f"expected {expected_sha256}, got {captured.source.sha256}"
            )
        if required_prefix is not None and captured.prefix != required_prefix:
            captured.discard()
            raise OSError(
                "Failed to snapshot linker input "
                f"{label}: stable prefix failed linker input contract"
            )
        if accept_path is not None:
            try:
                accepted_path = accept_path(snapshot)
            except BaseException:
                captured.discard()
                raise
            if not accepted_path:
                captured.discard()
                raise OSError(
                    "Failed to snapshot linker input "
                    f"{label}: stable snapshot failed linker metadata preflight"
                )
        return snapshot
    raise OSError(
        f"Linker input remained mutable while snapshotting {label}: "
        f"{source} ({last_observation})"
    )


def _run_wasm_ld(
    wasm_ld: str,
    runtime: Path,
    output: Path,
    linked: Path,
    *,
    runtime_role: RuntimeLinkInputRole,
    allowlist_override: Path | None = None,
    optimize: bool = False,
    optimize_level: str = "Oz",
    freestanding: bool = False,
    split_runtime: bool = False,
    split_output_dir: Path | None = None,
    deploy_runtime_override: Path | None = None,
    deploy_runtime_imports: Sequence[str] | None = None,
    native_link_requirements: SourceExtensionLinkRequirements | None = None,
    preserve_debug_sections: bool = False,
    phase_timings_file: Path | None = None,
    wasm_facts_scanner: Path,
    app_export_contract_path: Path | None = None,
    runtime_identity: StableRegularFileIdentity | None = None,
    deploy_runtime_identity: StableRegularFileIdentity | None = None,
    link_receipt: FinalLinkReceiptRequest | None = None,
    expected_inputs: Mapping[Path, str] | None = None,
    additional_inputs: Sequence[Path] = (),
) -> int:
    from molt.temporary_artifacts import OwnedTemporaryDirectory

    expected_target = "wasm32-unknown-unknown" if freestanding else "wasm32-wasip1"
    phase_timings_ms: dict[str, float] = {}
    expected = dict(expected_inputs or {})
    output_paths_admitted = False

    def expected_digest(path: Path) -> str | None:
        if not expected:
            return None
        resolved = path.resolve(strict=True)
        if resolved not in expected:
            raise ValueError(f"WASM link input has no producer identity: {path}")
        return expected[resolved]

    try:
        tool_path = Path(wasm_ld).resolve()
        tool_identity = (
            stable_regular_file_identity(tool_path, label="WASM linker")
            if expected
            else None
        )
        tool_digest = expected_digest(tool_path)
        if tool_identity is not None and tool_digest != tool_identity.sha256:
            raise ValueError("WASM linker changed after producer admission")
        native_link_requirements = (
            native_link_requirements or SourceExtensionLinkRequirements(expected_target)
        )
        if native_link_requirements.target_triple != expected_target:
            raise ValueError(
                "native WASM link requirements target mismatch: "
                f"{native_link_requirements.target_triple} != {expected_target}"
            )
        deploy_runtime = (
            _runtime_data._resolve_deploy_runtime(deploy_runtime_override)
            if split_runtime
            else None
        )
        # One complete alias check before paths are replaced by private snapshots.
        native_paths = tuple(
            Path(item.path) for item in native_link_requirements.inputs
        )
        manifest_paths = tuple(
            path.with_name(path.name + ".extension_manifest.json")
            for path in native_paths
        )
        publication_inputs = (
            runtime,
            output,
            wasm_facts_scanner,
            tool_path,
            *((deploy_runtime,) if deploy_runtime is not None else ()),
            *native_paths,
            *manifest_paths,
            *additional_inputs,
            *expected,
            *((app_export_contract_path,) if app_export_contract_path else ()),
            *((allowlist_override,) if allowlist_override else ()),
        )
        publication_outputs = wasm_link_output_paths(
            linked,
            optimize=optimize,
            external_selection=bool(native_link_requirements.items),
            split_output_dir=(split_output_dir or linked.parent)
            if split_runtime
            else None,
        )
        if link_receipt is not None:
            publication_outputs["receipt"] = link_receipt.path
        if phase_timings_file is not None:
            publication_outputs["timings"] = phase_timings_file
        validate_link_output_paths(publication_outputs, inputs=publication_inputs)
        output_paths_admitted = True
        with OwnedTemporaryDirectory(prefix="molt-wasm-link-custody-") as tmp:
            snapshot_root = Path(tmp)
            runtime_snapshot_root = snapshot_root / "runtime-pair"
            facts_provider = make_rust_wasm_facts_provider(
                wasm_facts_scanner,
                snapshot_root,
                phase_timings_ms,
                expected_sha256=expected_digest(wasm_facts_scanner),
                evidence_root=linked.parent,
            )

            def admit_runtime(path: Path) -> bool:
                # Validate the exact immutable snapshot once, at its custody
                # boundary. The pipeline consumes this admitted input and must
                # not relink it again merely to repeat the same admission.
                started = time.perf_counter()
                try:
                    error = _link_command._preflight_relocatable_runtime(
                        wasm_ld, path, snapshot_root
                    )
                    if error is not None:
                        raise ValueError(error)
                    return True
                finally:
                    phase_timings_ms["wasm_reloc_preflight"] = round(
                        (time.perf_counter() - started) * 1000.0, 6
                    )
                    phase_timings_ms["wasm_reloc_preflight_invocations"] = 1.0

            runtime_snapshot = _snapshot_link_input(
                runtime,
                runtime_snapshot_root,
                label="selected",
                expected_identity=runtime_identity,
                accept_path=admit_runtime if runtime_role == "reloc" else None,
                retry_delay_seconds=0.25,
            )
            runtime_snapshot = (
                runtime_snapshot_root / runtime_snapshot.parent.name / runtime.name
            )
            output_snapshot = _snapshot_link_input(
                output,
                snapshot_root,
                label="app",
                expected_sha256=expected_digest(output),
                required_prefix=WASM_MAGIC + WASM_VERSION,
            )
            app_export_contract_snapshot = None
            if app_export_contract_path is not None:
                app_export_contract_snapshot = _snapshot_link_input(
                    app_export_contract_path,
                    snapshot_root,
                    label="app-export-contract",
                    expected_sha256=expected_digest(app_export_contract_path),
                )
            native_snapshots: dict[Path, tuple[Path, str]] = {}

            def snapshot_native_input(
                item: SourceExtensionLinkInput,
            ) -> SourceExtensionLinkInput:
                source = Path(item.path)
                if not source.is_absolute():
                    raise ValueError(f"local link input must be absolute: {item.path}")
                try:
                    source = source.resolve(strict=True)
                except OSError as exc:
                    raise ValueError(
                        f"Native WASM link input is unavailable: {item.path}: {exc}"
                    ) from exc
                previous = native_snapshots.get(source)
                if previous is not None:
                    snapshot, digest = previous
                    if digest != item.sha256:
                        raise ValueError(
                            f"conflicting link requirement checksum claims for {source}"
                        )
                else:
                    index = len(native_snapshots)
                    snapshot = _snapshot_link_input(
                        source,
                        snapshot_root,
                        label=f"native-{index}",
                        expected_sha256=item.sha256,
                    )
                    manifest = source.with_name(
                        source.name + ".extension_manifest.json"
                    )
                    if manifest.exists():
                        _snapshot_link_input(
                            manifest,
                            snapshot_root,
                            label=f"native-{index}-manifest",
                            expected_sha256=expected_digest(manifest),
                            snapshot_directory=snapshot.parent,
                        )
                    native_snapshots[source] = (snapshot, item.sha256)
                return SourceExtensionLinkInput(
                    str(snapshot), item.sha256, item.loading
                )

            snapshot_requirements = map_source_extension_link_inputs(
                native_link_requirements,
                snapshot_native_input,
            )
            resolved_requirements = _native_inputs._resolve_native_link_requirements(
                snapshot_requirements,
                facts_provider=facts_provider,
                source_paths={
                    snapshot: source
                    for source, (snapshot, _digest) in native_snapshots.items()
                },
            )
            admitted_inputs = set(snapshot_requirements.inputs)
            snapshot_requirements = map_source_extension_link_inputs(
                resolved_requirements,
                lambda item: (
                    item if item in admitted_inputs else snapshot_native_input(item)
                ),
            )
            deploy_runtime_snapshot = None
            if deploy_runtime is not None:
                deploy_runtime_snapshot = _snapshot_link_input(
                    deploy_runtime,
                    snapshot_root,
                    label="deploy-runtime",
                    expected_identity=deploy_runtime_identity,
                )
            if tool_identity is not None:
                verify_stable_regular_file_identity(
                    tool_identity, label="WASM linker before execution"
                )
            result = _run_wasm_ld_with_custodied_inputs(
                wasm_ld,
                runtime_snapshot,
                output_snapshot,
                linked,
                runtime_role=runtime_role,
                allowlist_override=allowlist_override,
                optimize=optimize,
                optimize_level=optimize_level,
                freestanding=freestanding,
                split_runtime=split_runtime,
                split_output_dir=split_output_dir,
                deploy_runtime_override=deploy_runtime_snapshot,
                deploy_runtime_imports=deploy_runtime_imports,
                native_link_requirements=snapshot_requirements,
                preserve_debug_sections=preserve_debug_sections,
                phase_timings_ms=phase_timings_ms,
                wasm_facts_scanner=wasm_facts_scanner,
                wasm_facts_scanner_sha256=expected_digest(wasm_facts_scanner),
                facts_provider=facts_provider,
                app_export_contract_path=app_export_contract_snapshot,
                link_receipt=link_receipt,
            )
            if tool_identity is not None:
                verify_stable_regular_file_identity(
                    tool_identity, label="WASM linker after execution"
                )
            return result
    except (OSError, ValueError) as exc:
        print(f"Failed to establish wasm linker input custody: {exc}", file=sys.stderr)
        return 1
    finally:
        # One publisher covers both custody/preflight failures and the complete
        # link. The pipeline updates this same operation-owned timing record.
        if phase_timings_file is not None and output_paths_admitted:
            phase_timings_file.parent.mkdir(parents=True, exist_ok=True)
            phase_timings_file.write_text(
                json.dumps(phase_timings_ms, sort_keys=True) + "\n",
                encoding="utf-8",
            )


@local_python_import_graph_transaction()
def main() -> int:
    parser = argparse.ArgumentParser(
        description="Attempt to link Molt output/runtime into a single WASM module.",
    )
    parser.add_argument("--runtime", type=Path, default=_default_runtime_path())
    parser.add_argument("--runtime-shared", type=Path, required=True)
    parser.add_argument("--runtime-generation", type=Path, required=True)
    parser.add_argument("--runtime-expected-identity", type=Path, required=True)
    parser.add_argument("--input", type=Path, default=_default_input_path())
    parser.add_argument("--output", type=Path, default=_default_output_path())
    parser.add_argument(
        "--freestanding",
        action="store_true",
        default=False,
        help="Stub out WASI imports post-link for freestanding deployment",
    )
    parser.add_argument(
        "--optimize",
        action="store_true",
        default=False,
        help="Run wasm-opt after linking (requires Binaryen)",
    )
    parser.add_argument(
        "--optimize-level",
        default="Oz",
        choices=WASM_OPT_LEVELS,
        help="wasm-opt optimization level (O1/O2/O3/O4/Os/Oz, default: Oz)",
    )
    parser.add_argument(
        "--split-runtime",
        action="store_true",
        default=False,
        help="Generate app.wasm + molt_runtime.wasm instead of a single linked binary",
    )
    parser.add_argument(
        "--split-output-dir",
        type=Path,
        default=None,
        help="Directory for split-runtime output files (default: same as --output parent)",
    )
    parser.add_argument(
        "--deploy-runtime",
        type=Path,
        default=None,
        dest="deploy_runtime_override",
        help="Override the deploy runtime wasm path (non-relocatable variant)",
    )
    parser.add_argument(
        "--native-link-plan",
        type=Path,
        help="JSON plan containing typed, checksummed native link requirements",
    )
    parser.add_argument(
        "--preserve-debug-sections",
        action="store_true",
        help="Preserve name and DWARF sections while still removing final-link metadata",
    )
    parser.add_argument("--phase-timings-file", type=Path, default=None)
    parser.add_argument("--wasm-facts-scanner", type=Path, required=True)
    parser.add_argument(
        "--expected-input",
        nargs=2,
        action="append",
        default=[],
        metavar=("PATH", "SHA256"),
        help="Producer-bound input bytes; repeat for each admitted input",
    )
    parser.add_argument("--app-export-contract", type=Path, required=True)
    parser.add_argument(
        "--link-receipt-request",
        type=Path,
        help="Private input fingerprint request; receipt is published with final outputs",
    )
    args = parser.parse_args()

    runtime = args.runtime
    output = args.input
    linked = args.output
    try:
        link_receipt = (
            FinalLinkReceiptRequest.read(args.link_receipt_request)
            if args.link_receipt_request is not None
            else None
        )
        expected_inputs = {}
        input_versions = []
        for name, digest in args.expected_input:
            path = Path(name).resolve(strict=True)
            if len(digest) != 64 or any(c not in "0123456789abcdef" for c in digest):
                raise ValueError(f"invalid expected input SHA-256: {name}")
            if path in expected_inputs:
                raise ValueError(f"duplicate expected input identity: {name}")
            expected_inputs[path] = digest
            identity = stable_regular_file_identity(
                path, label="producer-bound WASM input"
            )
            if identity.sha256 != digest:
                raise ValueError(f"WASM input changed after producer admission: {path}")
            input_versions.append(identity)
        additional_inputs = tuple(
            path
            for path in (
                args.runtime_shared,
                args.runtime_generation,
                args.runtime_expected_identity,
                args.native_link_plan,
                args.link_receipt_request,
            )
            if path is not None
        )
    except (OSError, ValueError, UnicodeError) as exc:
        print(f"Invalid final link receipt request: {exc}", file=sys.stderr)
        return 1

    if not runtime.exists():
        print(f"Runtime wasm not found: {runtime}", file=sys.stderr)
        return 1
    generation = _verify_runtime_generation(
        reloc=runtime,
        shared=args.runtime_shared,
        generation_manifest=args.runtime_generation,
        expected_identity=args.runtime_expected_identity,
    )
    runtime = generation.reloc
    deploy_runtime_imports: tuple[str, ...] | None = None
    if args.split_runtime:
        try:
            deploy_runtime_imports = generation.shared_runtime_import_names()
        except ValueError as exc:
            print(
                f"Runtime generation has no derivable shared-member ABI: {exc}",
                file=sys.stderr,
            )
            return 1
    if args.deploy_runtime_override is not None and (
        args.deploy_runtime_override.resolve(strict=False)
        != generation.shared.resolve(strict=False)
    ):
        print(
            "Explicit deploy runtime is not the shared member selected by the "
            "trusted generation.",
            file=sys.stderr,
        )
        return 1
    if not output.exists():
        print(f"Output wasm not found: {output}", file=sys.stderr)
        return 1
    native_link_requirements = None
    if args.native_link_plan is not None:
        try:
            expected_target = (
                "wasm32-unknown-unknown" if args.freestanding else "wasm32-wasip1"
            )
            native_link_requirements = read_source_extension_link_plan(
                args.native_link_plan, expected_target_triple=expected_target
            )
        except (OSError, ValueError) as exc:
            print(f"Invalid native WASM link plan: {exc}", file=sys.stderr)
            return 1
    linked.parent.mkdir(parents=True, exist_ok=True)

    wasm_ld = _link_command._find_wasm_ld()
    if not wasm_ld:
        print(
            "wasm-ld not found; install LLVM to enable single-module linking.",
            file=sys.stderr,
        )
        return 1

    try:
        for identity in input_versions:
            verify_stable_regular_file_identity(
                identity, label="WASM input before link"
            )
        result = _run_wasm_ld(
            wasm_ld,
            runtime,
            output,
            linked,
            runtime_role="reloc",
            expected_inputs=expected_inputs,
            additional_inputs=additional_inputs,
            optimize=args.optimize,
            optimize_level=args.optimize_level,
            freestanding=args.freestanding,
            split_runtime=args.split_runtime,
            split_output_dir=args.split_output_dir,
            deploy_runtime_override=generation.shared if args.split_runtime else None,
            deploy_runtime_imports=deploy_runtime_imports,
            native_link_requirements=native_link_requirements,
            preserve_debug_sections=args.preserve_debug_sections,
            phase_timings_file=args.phase_timings_file,
            wasm_facts_scanner=args.wasm_facts_scanner,
            app_export_contract_path=args.app_export_contract,
            runtime_identity=generation.reloc_member_identity,
            link_receipt=link_receipt,
            deploy_runtime_identity=(
                generation.shared_member_identity if args.split_runtime else None
            ),
        )
        for identity in input_versions:
            verify_stable_regular_file_identity(identity, label="WASM input after link")
        return result
    except (OSError, ValueError) as exc:
        print(f"WASM input custody failed: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
