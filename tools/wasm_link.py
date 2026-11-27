#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
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
    RuntimeWasmRelocAdmission,
    RuntimeWasmReceiptRole,
    RuntimeWasmTrust,
    read_trusted_runtime_wasm_generation,
    read_trusted_runtime_wasm_reloc,
)
from molt.artifact_publication import copy_reconstructible_artifact  # noqa: E402
from molt.toolchain_identity import (  # noqa: E402
    StableRegularFileIdentity,
    executable_content_path,
    stable_executable_probe,
    stable_regular_file_identity,
    verify_stable_regular_file_identity,
)
from molt.cli.python_source_closure import local_python_import_graph_transaction  # noqa: E402
from molt.cli.source_extension_link_requirements import (  # noqa: E402
    SourceExtensionLinkRequirements,
    read_source_extension_link_plan,
)
from molt.cli import wasm_link_inputs, wasm_toolchain  # noqa: E402
from molt.link_outputs import wasm_link_output_paths  # noqa: E402
from molt.llvm_linker_roles import executable_selects_linker_role  # noqa: E402
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
    shared: Path | None,
    generation_manifest: Path,
    trust: RuntimeWasmTrust,
) -> tuple[RuntimeWasmRelocAdmission, StableRegularFileIdentity | None]:
    """Verify the runtime inputs selected by the link mode."""
    for path in (reloc, generation_manifest, shared):
        if path is not None and ".." in path.parts:
            raise ValueError(f"Runtime custody path contains '..': {path}")
    deploy_runtime = None
    if shared is None:
        admitted = read_trusted_runtime_wasm_reloc(generation_manifest, trust)
    else:
        generation = read_trusted_runtime_wasm_generation(generation_manifest, trust)
        if generation is None:
            admitted = None
        else:
            assert generation.receipt_identity is not None
            admitted = RuntimeWasmRelocAdmission(
                generation.reloc_member_identity,
                generation.receipt_identity,
                generation.shared,
            )
            deploy_runtime = generation.shared_member_identity
            if shared.resolve() != deploy_runtime.path.resolve():
                raise ValueError(
                    f"Shared runtime input is not selected by {generation_manifest}"
                )
    if admitted is None:
        raise ValueError(
            "Runtime generation does not match the trusted caller identity: "
            f"{generation_manifest}"
        )
    if reloc.resolve() != admitted.reloc_member_identity.path.resolve():
        raise ValueError(
            f"Relocatable runtime input is not selected by {generation_manifest}"
        )
    return admitted, deploy_runtime


def _link_admitted_standard_object(
    args: argparse.Namespace,
    *,
    wasm_ld: Path,
    wasm_ld_identity: StableRegularFileIdentity | None,
    runtime_admission: RuntimeWasmRelocAdmission,
    app_object: Path,
    linked: Path | None,
    split_output_dir: Path | None,
    native_link_requirements: SourceExtensionLinkRequirements | None,
    input_versions: Sequence[StableRegularFileIdentity],
    side_sdk_archives: Sequence[Path],
    deploy_runtime: StableRegularFileIdentity | None,
) -> int:
    """Link an admitted LLVM wasm32 application object with wasm-ld.

    A split build links the application as a wasm32 side module
    (`app.wasm`) beside the trusted shared runtime member
    (`molt_runtime.wasm`), which the host instantiates first and places the
    side module after (wasm/loader_bridge.js `placeSideModule`).
    """
    from tools.wasm_object_link import link_side_module, link_standard_object

    from molt.temporary_artifacts import OwnedTemporaryDirectory
    from tools.wasm_object_link import resolve_native_link

    runtime = runtime_admission.reloc_member_identity.path
    phase_timings_ms: dict[str, float] = {}
    selection_roles: dict[str, Mapping[str, object]] = {}
    scratch_root = proof_scratch_root(TOOLS_ROOT.parent, create=True)
    scratch = OwnedTemporaryDirectory(prefix="molt-wasm-object-link-", dir=scratch_root)
    try:
        outputs = wasm_link_output_paths(
            linked,
            split_output_dir=split_output_dir,
            inputs=(
                app_object,
                *(
                    (runtime_admission.shared_path,)
                    if runtime_admission.shared_path is not None
                    else ()
                ),
                *(identity.path for identity in input_versions),
            ),
            external_selection=bool(
                native_link_requirements and native_link_requirements.items
            ),
            optimize=args.optimize,
        )
        primary = outputs["app"] if split_output_dir is not None else outputs["linked"]
        for path in outputs.values():
            path.parent.mkdir(parents=True, exist_ok=True)
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
            or primary.parent / "wasm-link-evidence",
            phase_timings_ms=phase_timings_ms,
        )
        with stable_executable_probe(
            wasm_ld, label="WASM linker", identity=wasm_ld_identity
        ) as (entrypoint, _):
            status = 0
            if linked is not None:
                status = link_standard_object(
                    wasm_ld=str(entrypoint),
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
            if status == 0 and split_output_dir is not None:
                status = link_side_module(
                    wasm_ld=str(entrypoint),
                    app_object=app_object,
                    stdlib_archive=args.stdlib_archive,
                    app=outputs["app"],
                    app_export_contract=args.app_export_contract,
                    native_link_requirements=native.requirements,
                    sdk_archives=side_sdk_archives,
                    selection_roles=selection_roles,
                    preserve_debug=args.preserve_debug_sections,
                    phase_timings_ms=phase_timings_ms,
                )
        if status == 0 and split_output_dir is not None:
            assert deploy_runtime is not None
            copy_reconstructible_artifact(deploy_runtime, outputs["runtime"])
        if status == 0 and selection_roles:
            from molt.cli.link_selection_admission import write_link_selection

            selection = outputs["selection"]
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
    parser.add_argument(
        "--runtime-shared", type=Path, help="Immutable shared member for split output"
    )
    parser.add_argument("--runtime-generation", type=Path, required=True)
    parser.add_argument("--runtime-family-digest", required=True)
    parser.add_argument(
        "--runtime-receipt-role",
        required=True,
        choices=[role.value for role in RuntimeWasmReceiptRole],
    )
    parser.add_argument("--runtime-receipt-sha256", required=True)
    parser.add_argument("--input", type=Path, default=_default_input_path())
    parser.add_argument(
        "--stdlib-archive",
        type=Path,
        default=None,
        help="Shared stdlib archive of an LLVM wasm32 application object",
    )
    parser.add_argument(
        "--output", type=Path, help="Emit a monolithic linked module at this path"
    )
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
        help="Run wasm-opt on the monolithic linked module (requires Binaryen)",
    )
    parser.add_argument(
        "--optimize-level",
        default="Oz",
        choices=WASM_OPT_LEVELS,
        help="Post-link optimization policy (default: Oz)",
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
        help="Directory for split-runtime output files",
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
    parser.add_argument(
        "--wasm-linker",
        nargs=2,
        type=Path,
        metavar=("ENTRYPOINT", "CONTENT"),
        help="Use the admitted linker entrypoint and producer-bound content",
    )
    parser.add_argument("--app-export-contract", type=Path, required=True)
    args = parser.parse_args()
    if args.split_runtime and args.runtime_shared is None:
        parser.error("--split-runtime requires --runtime-shared")
    if not args.split_runtime and args.runtime_shared is not None:
        parser.error("--runtime-shared requires --split-runtime")

    runtime = args.runtime
    output = args.input
    linked = (
        args.output if args.split_runtime else args.output or _default_output_path()
    )
    split_output_dir = (
        args.split_output_dir or (linked or _default_output_path()).parent
        if args.split_runtime
        else None
    )
    wasm_ld: Path | None = None
    try:
        expected_inputs: dict[Path, str] = {}
        wasm_ld_identity: StableRegularFileIdentity | None = None
        linker_content = args.wasm_linker[1] if args.wasm_linker is not None else None
        side_sdk_archives = (
            wasm_link_inputs.wasm_side_module_sdk_archives()
            if args.split_runtime
            else ()
        )
        sdk_versions = {identity.path: identity for identity in side_sdk_archives}
        input_versions = list(side_sdk_archives)
        for name, digest in args.expected_input:
            path = Path(name).resolve(strict=True)
            if len(digest) != 64 or any(c not in "0123456789abcdef" for c in digest):
                raise ValueError(f"invalid expected input SHA-256: {name}")
            if path in expected_inputs:
                raise ValueError(f"duplicate expected input identity: {name}")
            expected_inputs[path] = digest
            identity = sdk_versions.get(path)
            if identity is None:
                identity = stable_regular_file_identity(
                    path, label="producer-bound WASM input"
                )
                input_versions.append(identity)
            if identity.sha256 != digest:
                raise ValueError(f"WASM input changed after producer admission: {path}")
            if path == linker_content:
                wasm_ld_identity = identity
        if expected_inputs and sdk_versions.keys() - expected_inputs.keys():
            raise ValueError("selected side-module SDK differs from producer admission")
        if args.wasm_linker is not None:
            wasm_ld, _ = args.wasm_linker
            if wasm_ld_identity is None or not executable_selects_linker_role(
                wasm_ld, "wasm-ld"
            ):
                raise ValueError(
                    "WASM linker has no producer-bound executable identity"
                )
            if (
                executable_content_path(wasm_ld, label="WASM linker")
                != wasm_ld_identity.path
            ):
                raise ValueError(
                    "WASM linker entrypoint changed after producer admission"
                )
    except (OSError, ValueError) as exc:
        print(f"Invalid producer-bound WASM input: {exc}", file=sys.stderr)
        return 1

    try:
        admitted_runtime, deploy_runtime = _verify_runtime_generation(
            reloc=runtime,
            shared=args.runtime_shared,
            generation_manifest=args.runtime_generation,
            trust=RuntimeWasmTrust(
                args.runtime_family_digest,
                RuntimeWasmReceiptRole(args.runtime_receipt_role),
                args.runtime_receipt_sha256,
            ),
        )
        input_versions.extend(
            (admitted_runtime.reloc_member_identity, admitted_runtime.receipt_identity)
        )
        if deploy_runtime is not None:
            input_versions.append(deploy_runtime)
    except (OSError, ValueError) as exc:
        print(f"Invalid WASM runtime admission: {exc}", file=sys.stderr)
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
    if wasm_ld is None:
        try:
            wasm_ld = wasm_toolchain.resolve_wasm_linker(_REPO_ROOT).path
        except wasm_toolchain.WasmLinkerContractError as exc:
            print(f"WASM linker is unavailable: {exc}", file=sys.stderr)
            return 1

    return _link_admitted_standard_object(
        args,
        wasm_ld=wasm_ld,
        wasm_ld_identity=wasm_ld_identity,
        runtime_admission=admitted_runtime,
        app_object=output,
        linked=linked,
        split_output_dir=split_output_dir,
        native_link_requirements=native_link_requirements,
        input_versions=input_versions,
        side_sdk_archives=tuple(sdk_versions),
        deploy_runtime=deploy_runtime,
    )


if __name__ == "__main__":
    raise SystemExit(main())
