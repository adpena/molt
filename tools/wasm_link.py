#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
import shutil
import sys
from collections.abc import Mapping, Sequence
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

TOOLS_ROOT = Path(__file__).resolve().parent

SRC_ROOT = TOOLS_ROOT.parent / "src"


from molt.cli.runtime_wasm_generation import (  # noqa: E402
    RuntimeWasmGeneration,
    RuntimeWasmReceiptRole,
    RuntimeWasmTrust,
    read_trusted_runtime_wasm_generation,
)
from molt.toolchain_identity import (  # noqa: E402
    StableRegularFileIdentity,
    stable_regular_file_identity,
    verify_stable_regular_file_identity,
)
from molt.cli.python_source_closure import local_python_import_graph_transaction  # noqa: E402
from molt.cli.source_extension_link_requirements import (  # noqa: E402
    SourceExtensionLinkRequirements,
    read_source_extension_link_plan,
)
from molt.cli import wasm_toolchain  # noqa: E402
from molt.wasm_optimization import WASM_OPT_LEVELS  # noqa: E402
from molt.dx import configured_artifact_root, proof_scratch_root  # noqa: E402


def _default_dist_artifact_path(name: str) -> Path:
    external_root = configured_artifact_root(os.environ, relative_to=Path.cwd())
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
    trust: RuntimeWasmTrust,
) -> RuntimeWasmGeneration:
    """Verify both runtime members against the caller's trusted pair."""

    for path in (reloc, shared, generation_manifest):
        if ".." in path.parts:
            raise SystemExit(f"Runtime custody path contains '..': {path}")
    generation = read_trusted_runtime_wasm_generation(generation_manifest, trust)
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


def _link_admitted_standard_object(
    args: argparse.Namespace,
    *,
    wasm_ld: str,
    runtime: Path,
    app_object: Path,
    linked: Path,
    native_link_requirements: SourceExtensionLinkRequirements | None,
    input_versions: Sequence[StableRegularFileIdentity],
    deploy_runtime: Path,
) -> int:
    """Link an admitted LLVM wasm32 application object with wasm-ld.

    A split build also links the application as a wasm32 side module
    (`app.wasm`) beside the trusted shared runtime member
    (`molt_runtime.wasm`), which the host instantiates first and places the
    side module after (wasm/loader_bridge.js `placeSideModule`).
    """
    from tools.wasm_object_link import link_side_module, link_standard_object

    from molt.temporary_artifacts import OwnedTemporaryDirectory
    from tools.wasm_object_link import resolve_native_link

    phase_timings_ms: dict[str, float] = {}
    selection_roles: dict[str, Mapping[str, object]] = {}
    scratch_root = proof_scratch_root(TOOLS_ROOT.parent, create=True)
    scratch = OwnedTemporaryDirectory(prefix="molt-wasm-object-link-", dir=scratch_root)
    try:
        for identity in input_versions:
            verify_stable_regular_file_identity(
                identity, label="WASM input before link"
            )
        native = resolve_native_link(
            native_link_requirements
            or SourceExtensionLinkRequirements(
                "wasm32-unknown-unknown" if args.freestanding else "wasm32-wasip1"
            ),
            runtime=runtime,
            wasm_facts_scanner=args.wasm_facts_scanner,
            base_allowlist=TOOLS_ROOT / "wasm_allowed_imports.txt",
            scratch=scratch,
            evidence_root=args.failure_evidence_dir
            or linked.parent / "wasm-link-evidence",
            phase_timings_ms=phase_timings_ms,
        )
        status = link_standard_object(
            wasm_ld=wasm_ld,
            app_object=app_object,
            stdlib_archive=args.stdlib_archive,
            runtime=runtime,
            allowlist=native.allowlist,
            linked=linked,
            app_export_contract=args.app_export_contract,
            native_link_requirements=native.requirements,
            selection_roles=selection_roles,
            optimize=args.optimize,
            optimize_level=args.optimize_level,
            preserve_debug=args.preserve_debug_sections,
            phase_timings_ms=phase_timings_ms,
            freestanding=args.freestanding,
        )
        if status == 0 and args.split_runtime:
            split_dir = args.split_output_dir or linked.parent
            status = link_side_module(
                wasm_ld=wasm_ld,
                app_object=app_object,
                stdlib_archive=args.stdlib_archive,
                app=split_dir / "app.wasm",
                app_export_contract=args.app_export_contract,
                native_link_requirements=native.requirements,
                selection_roles=selection_roles,
                preserve_debug=args.preserve_debug_sections,
                phase_timings_ms=phase_timings_ms,
            )
            if status == 0:
                staged_runtime = split_dir / ".molt_runtime.wasm.staged"
                shutil.copyfile(deploy_runtime, staged_runtime)
                staged_runtime.replace(split_dir / "molt_runtime.wasm")
        if status == 0 and selection_roles:
            from molt.cli.link_selection_admission import write_link_selection
            from molt.link_outputs import link_selection_path

            selection = link_selection_path(linked)
            staged_selection = selection.with_name(f".{selection.name}.staged")
            write_link_selection(staged_selection, selection_roles)
            staged_selection.replace(selection)
        for identity in input_versions:
            verify_stable_regular_file_identity(identity, label="WASM input after link")
        return status
    except (OSError, ValueError) as exc:
        print(f"WASM standard-object link failed: {exc}", file=sys.stderr)
        return 1
    finally:
        scratch.cleanup()
        if args.phase_timings_file is not None:
            args.phase_timings_file.parent.mkdir(parents=True, exist_ok=True)
            args.phase_timings_file.write_text(
                json.dumps(phase_timings_ms, sort_keys=True) + "\n", encoding="utf-8"
            )


@local_python_import_graph_transaction()
def main() -> int:
    parser = argparse.ArgumentParser(
        description="Link an LLVM wasm32 application object with the Molt runtime.",
    )
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--runtime-shared", type=Path, required=True)
    parser.add_argument("--runtime-generation", type=Path, required=True)
    parser.add_argument("--runtime-family-digest", required=True)
    parser.add_argument(
        "--runtime-receipt-role",
        required=True,
        choices=[role.value for role in RuntimeWasmReceiptRole],
    )
    parser.add_argument("--input", type=Path, default=_default_input_path())
    parser.add_argument(
        "--stdlib-archive",
        type=Path,
        default=None,
        help="Shared stdlib archive of an LLVM wasm32 application object",
    )
    parser.add_argument("--output", type=Path, default=_default_output_path())
    parser.add_argument(
        "--failure-evidence-dir",
        type=Path,
        help="Keep rejected WASM bytes here (default: output parent/wasm-link-evidence)",
    )
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
    args = parser.parse_args()

    runtime = args.runtime
    output = args.input
    linked = args.output
    try:
        expected_inputs: dict[Path, str] = {}
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
    except (OSError, ValueError) as exc:
        print(f"Invalid producer-bound WASM input: {exc}", file=sys.stderr)
        return 1

    if not runtime.exists():
        print(f"Runtime wasm not found: {runtime}", file=sys.stderr)
        return 1
    generation = _verify_runtime_generation(
        reloc=runtime,
        shared=args.runtime_shared,
        generation_manifest=args.runtime_generation,
        trust=RuntimeWasmTrust(
            args.runtime_family_digest,
            RuntimeWasmReceiptRole(args.runtime_receipt_role),
        ),
    )
    runtime = generation.reloc
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

    try:
        wasm_ld = str(wasm_toolchain.resolve_wasm_linker().path)
    except wasm_toolchain.WasmLinkerContractError as exc:
        print(f"WASM linker is unavailable: {exc}", file=sys.stderr)
        return 1

    return _link_admitted_standard_object(
        args,
        wasm_ld=wasm_ld,
        runtime=runtime,
        app_object=output,
        linked=linked,
        native_link_requirements=native_link_requirements,
        input_versions=input_versions,
        deploy_runtime=generation.shared,
    )


if __name__ == "__main__":
    raise SystemExit(main())
