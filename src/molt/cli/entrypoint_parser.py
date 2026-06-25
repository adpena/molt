from __future__ import annotations

import argparse

from molt.debug import DebugSubcommand
from molt.cli import factgraph as _factgraph
from molt.cli.arg_helpers import (
    _BuildHelpFormatter,
    _MoltHelpFormatter,
    _add_debug_shared_selector_args,
)
from molt.cli.build_output_layout import (
    _BUILD_OR_DEPLOY_PROFILE_CHOICES,
    _BUILD_PROFILE_CHOICES,
    _DEPLOY_PROFILE_CHOICES,
)
from molt.cli.toolchain_validation import _VALIDATE_SUITE_CHOICES


def _build_entrypoint_parser() -> argparse.ArgumentParser:
    from molt import __version__

    parser = argparse.ArgumentParser(
        prog="molt",
        usage="molt [-h] [--version] <command> [options]",
        description="The Molt Python compiler",
        formatter_class=_MoltHelpFormatter,
        epilog=(
            "Run 'molt <command> --help' for more information on a command.\n"
            "\n"
            "Examples:\n"
            "  molt build app.py                  Build a Python program\n"
            "  molt run app.py                    Build and run\n"
            "  molt run app.py --release          Build optimized and run\n"
            "  molt build app.py --target wasm    Build for WebAssembly\n"
            "  molt deploy cloudflare app.py      Deploy to Cloudflare Workers\n"
            "  molt test                          Run test suites\n"
        ),
    )
    parser.add_argument("--version", action="version", version=f"molt {__version__}")
    # Don't require command; show help when no args (like `go` with no args).
    subparsers = parser.add_subparsers(dest="command", title="commands")

    build_parser = subparsers.add_parser(
        "build",
        help="Build a Python program",
        description="Compile a Python file to a native binary, WASM module, Luau script, or MLIR text.",
        formatter_class=_BuildHelpFormatter,
        epilog=(
            "Examples:\n"
            "  molt build app.py                      Build with default settings\n"
            "  molt build app.py --release             Optimized release build\n"
            "  molt build app.py --target wasm         Build for WebAssembly\n"
            "  molt build app.py --target luau         Build for Luau/Roblox\n"
            "  molt build app.py --target mlir         Emit MLIR text (requires LLVM 22)\n"
            "  molt build --module mypackage           Build a package by module name\n"
        ),
    )
    build_parser.add_argument("file", nargs="?", help="Path to Python source")
    build_parser.add_argument(
        "--module",
        help="Entry module name (uses pkg.__main__ when present).",
    )
    build_parser.add_argument(
        "--target",
        default=None,
        help=(
            "Build target: native (default), wasm, luau, mlir, or a target triple "
            "(e.g., aarch64-unknown-linux-gnu, x86_64-unknown-linux-musl)."
        ),
    )
    build_parser.add_argument(
        "--release",
        action="store_true",
        default=False,
        help="Optimized release build (alias for --build-profile release).",
    )
    build_parser.add_argument(
        "--codec",
        choices=["msgpack", "cbor", "json"],
        default=None,
        help="Structured codec for parse calls (default from config or msgpack).",
    )
    build_parser.add_argument(
        "--type-hints",
        choices=["ignore", "trust", "check"],
        default=None,
        help="Apply type annotations to guide lowering and specialization.",
    )
    build_parser.add_argument(
        "--fallback",
        choices=["error", "bridge"],
        default=None,
        help="Fallback policy for unsupported constructs.",
    )
    build_parser.add_argument(
        "--type-facts",
        help="Path to type facts JSON from `molt check`.",
    )
    build_parser.add_argument(
        "--python-version",
        default=None,
        help=(
            "Target Python semantics (3.12, 3.13, or 3.14). Defaults from "
            "[tool.molt.build] or project.requires-python."
        ),
    )
    build_parser.add_argument(
        "--pgo-profile",
        help="Path to a Molt profile artifact (molt_profile.json) for PGO hints.",
    )
    build_parser.add_argument(
        "--pgo-collect",
        action="store_true",
        default=False,
        help=(
            "Instrument the compiled binary to collect PGO counters at runtime. "
            "The instrumented binary writes branch counts, call counts, and loop "
            "iteration counts to a profile JSON file on exit."
        ),
    )
    build_parser.add_argument(
        "--pgo-collect-output",
        help=(
            "Output path for the PGO collection profile (default: "
            "molt_pgo_collected.json in the project root). Only used with --pgo-collect."
        ),
    )
    build_parser.add_argument(
        "--runtime-feedback",
        help=(
            "Path to a Molt runtime feedback artifact "
            "(molt_runtime_feedback.json) for measured hot-function promotion hints."
        ),
    )
    build_parser.add_argument(
        "--output",
        help=(
            "Output path for the native binary or wasm artifact "
            "(relative to --out-dir when set, otherwise the project root for explicit paths; "
            "default final artifacts land under dist/ when omitted). "
            "If the path is a directory (or ends with a path separator), "
            "the default filename is used within that directory."
        ),
    )
    build_parser.add_argument(
        "--out-dir",
        help=(
            "Output directory for final artifacts (binary/wasm/object). "
            "Intermediates stay under MOLT_HOME/build/<entry> by default. "
            "Native binaries otherwise default to MOLT_BIN."
        ),
    )
    build_parser.add_argument(
        "--sysroot",
        help=(
            "Sysroot path for native linking (relative paths resolve under the project "
            "root; defaults to MOLT_SYSROOT or MOLT_CROSS_SYSROOT when set)."
        ),
    )
    build_parser.add_argument(
        "--emit",
        choices=["bin", "obj", "wasm"],
        default=None,
        help="Select which artifact to emit (native: bin/obj, wasm: wasm).",
    )
    build_parser.add_argument(
        "--linked",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Emit a linked wasm artifact (output_linked.wasm) alongside output.wasm.",
    )
    build_parser.add_argument(
        "--linked-output",
        help=(
            "Output path for the linked wasm artifact "
            "(relative to --out-dir when set, otherwise the project root for explicit paths; "
            "the default linked artifact lands under dist/ when omitted)."
        ),
    )
    build_parser.add_argument(
        "--require-linked",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require linked wasm output for wasm targets (fails if linking is unavailable).",
    )
    build_parser.add_argument(
        "--wasm-opt-level",
        choices=["Oz", "O3"],
        default="Oz",
        help=(
            "WASM optimization profile: Oz for size-focused (default, "
            "recommended for browser deployment), O3 for speed-focused "
            "(recommended for server/edge deployment)."
        ),
    )
    build_parser.add_argument(
        "--precompile",
        action="store_true",
        default=False,
        help=(
            "After linking, run wasmtime compile to produce a precompiled "
            ".cwasm artifact for 10-50x faster startup in production."
        ),
    )
    build_parser.add_argument(
        "--snapshot",
        action="store_true",
        default=False,
        help=(
            "Generate a molt.snapshot.json header alongside the WASM output "
            "for sub-millisecond cold starts on edge platforms. "
            "Records mount plan, capabilities, and module hash metadata."
        ),
    )
    build_parser.add_argument(
        "--split-runtime",
        action="store_true",
        default=False,
        help=(
            "Produce separate runtime and app WASM modules instead of a single "
            "linked binary. The runtime is tree-shaken to include only the "
            "builtins and runtime exports your program uses, then both split "
            "artifacts are deforested with post-link cleanup and wasm-opt. "
            "Outputs app.wasm + molt_runtime.wasm + worker.js + manifest.json."
        ),
    )
    build_parser.add_argument(
        "--wasm-profile",
        choices=["full", "pure"],
        default="full",
        help=(
            "WASM import profile: full (default) registers all host imports; "
            "pure omits IO/ASYNC/TIME imports for minimal pure-computation modules."
        ),
    )
    build_parser.add_argument(
        "--stdlib-profile",
        choices=["full", "micro"],
        default=None,
        help="Runtime stdlib profile (full=all modules, micro=core only for smallest binary)",
    )
    build_parser.add_argument(
        "--emit-ir",
        help="Write the lowered IR JSON to a file path.",
    )
    build_parser.add_argument(
        "--build-profile",
        choices=_BUILD_PROFILE_CHOICES,
        default=None,
        help="Build profile for backend/runtime (default: release).",
    )
    build_parser.add_argument(
        "--backend",
        choices=["cranelift", "llvm", "auto"],
        default="auto",
        help="Compilation backend (auto=cranelift; llvm is opt-in and requires an LLVM toolchain).",
    )
    build_parser.add_argument(
        "--profile",
        choices=_BUILD_OR_DEPLOY_PROFILE_CHOICES,
        default=None,
        help=(
            "Build profile (dev/release) or legacy deployment platform/profile "
            "(cloudflare/browser/wasi/fastly)."
        ),
    )
    build_parser.add_argument(
        "--platform",
        choices=_DEPLOY_PROFILE_CHOICES,
        default=None,
        help="Deployment platform/profile (sets optimization defaults for the target platform).",
    )
    build_parser.add_argument(
        "--deterministic",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require deterministic inputs (lockfiles).",
    )
    build_parser.add_argument(
        "--deterministic-warn",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Warn instead of failing when deterministic lockfile checks fail.",
    )
    build_parser.add_argument(
        "--portable",
        action="store_true",
        default=False,
        help="Use baseline ISA (no host-specific CPU features). Ensures cross-machine reproducible codegen at ~5-15%% runtime cost.",
    )
    build_parser.add_argument(
        "--trusted",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Disable capability checks for trusted deployments (native only).",
    )
    build_parser.add_argument(
        "--cache",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Enable build cache under MOLT_CACHE (defaults to the OS cache).",
    )
    build_parser.add_argument(
        "--cache-dir",
        help="Override the build cache directory (default: MOLT_CACHE).",
    )
    build_parser.add_argument(
        "--cache-report",
        action="store_true",
        help="Print cache hit/miss details even without --verbose.",
    )
    build_parser.add_argument(
        "--rebuild",
        action="store_true",
        help="Disable the build cache (alias for --no-cache).",
    )
    build_parser.add_argument(
        "--respect-pythonpath",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Include PYTHONPATH entries as module roots during compilation.",
    )
    build_parser.add_argument(
        "--capabilities",
        help="Capability profiles/tokens or path to manifest (toml/json).",
    )
    build_parser.add_argument(
        "--capability-manifest",
        help="Path to a capability manifest file (toml/json/yaml) for build-time configuration.",
    )
    build_parser.add_argument(
        "--require-signed-manifest",
        action="store_true",
        default=False,
        help="Reject unsigned capability manifests. Requires --capability-manifest.",
    )
    build_parser.add_argument(
        "--audit-log",
        metavar="SINK:OUTPUT",
        help="Enable audit logging (e.g., 'jsonl:stderr', 'stderr:stderr')",
    )
    build_parser.add_argument(
        "--io-mode",
        choices=["real", "virtual", "callback"],
        default=None,
        help="IO mode: real (default), virtual (sandbox), callback (host-mediated)",
    )
    build_parser.add_argument(
        "--type-gate",
        action="store_true",
        default=False,
        help="Reject compilation if capability-touching code paths contain untyped variables",
    )
    build_parser.add_argument(
        "--diagnostics",
        action=argparse.BooleanOptionalAction,
        default=None,
        help=(
            "Enable compile diagnostics payloads (phase timings, module reasons, "
            "frontend/midend summaries)."
        ),
    )
    build_parser.add_argument(
        "--diagnostics-file",
        help=(
            "Optional path for compile diagnostics JSON (relative paths resolve "
            "under the build artifacts root). Implies --diagnostics."
        ),
    )
    build_parser.add_argument(
        "--diagnostics-verbosity",
        choices=["summary", "default", "full"],
        default=None,
        help=(
            "Select stderr build diagnostics detail level. "
            "JSON/file diagnostics remain complete."
        ),
    )
    build_parser.add_argument(
        "--lib-path",
        action="append",
        default=[],
        help="Additional directories to search for Python packages (repeatable).",
    )
    build_parser.add_argument(
        "--bolt",
        action="store_true",
        default=False,
        help=(
            "Run BOLT post-link optimization on the output binary. "
            "Instruments, profiles with a training run, and reorders "
            "functions/basic blocks for optimal icache utilization. "
            "Requires llvm-bolt (brew install llvm / apt install llvm-bolt). "
            "Native targets only."
        ),
    )
    build_parser.add_argument(
        "--bolt-training-cmd",
        default=None,
        help=(
            "Custom training command for BOLT profiling (default: run the "
            "output binary with no arguments). Only used with --bolt."
        ),
    )
    build_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    build_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    _factgraph.add_factgraph_parser(
        subparsers,
        formatter_class=_BuildHelpFormatter,
        build_profile_choices=_BUILD_PROFILE_CHOICES,
    )

    extension_parser = subparsers.add_parser(
        "extension",
        help="Build and audit C extensions compiled against libmolt.",
    )
    extension_subparsers = extension_parser.add_subparsers(
        dest="extension_command", required=True
    )
    extension_build_parser = extension_subparsers.add_parser(
        "build",
        help="Compile a C extension against libmolt and emit a wheel + sidecar.",
    )
    extension_build_parser.add_argument(
        "--project",
        help="Project directory containing pyproject.toml (default: cwd).",
    )
    extension_build_parser.add_argument(
        "--out-dir",
        help="Output directory for wheel + extension_manifest.json (default: dist/).",
    )
    extension_build_parser.add_argument(
        "--molt-abi",
        help=(
            "Molt C-API ABI version override "
            "(default: tool.molt.extension.molt_c_api_version or MOLT_C_API_VERSION)."
        ),
    )
    extension_build_parser.add_argument(
        "--target",
        help="Target triple for extension build (default: native host target).",
    )
    extension_build_parser.add_argument(
        "--capabilities",
        help=(
            "Capabilities allowlist/profiles override "
            "(default: tool.molt.extension.capabilities)."
        ),
    )
    extension_build_parser.add_argument(
        "--deterministic",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require deterministic lockfile and reproducible wheel checks.",
    )
    extension_build_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    extension_build_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    extension_audit_parser = extension_subparsers.add_parser(
        "audit",
        help="Audit an extension manifest and wheel for ABI/capability compatibility.",
    )
    extension_audit_parser.add_argument(
        "--path",
        required=True,
        help="Path to a wheel, extension_manifest.json, or directory containing it.",
    )
    extension_audit_parser.add_argument(
        "--require-capabilities",
        action="store_true",
        help="Fail when the manifest capability list is empty.",
    )
    extension_audit_parser.add_argument(
        "--require-abi",
        help="Require an exact molt_c_api_version match.",
    )
    extension_audit_parser.add_argument(
        "--require-checksum",
        action="store_true",
        help="Require wheel and extension checksums in the manifest.",
    )
    extension_audit_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    extension_audit_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    extension_scan_parser = extension_subparsers.add_parser(
        "scan",
        help=(
            "Scan extension sources for unsupported Py* C-API usage "
            "against include/molt/Python.h."
        ),
    )
    extension_scan_parser.add_argument(
        "--project",
        help="Project directory containing pyproject.toml (default: cwd).",
    )
    extension_scan_parser.add_argument(
        "--source",
        action="append",
        help=(
            "Source path to scan (repeatable). If omitted, uses "
            "tool.molt.extension.sources from pyproject.toml."
        ),
    )
    extension_scan_parser.add_argument(
        "--fail-on-missing",
        action="store_true",
        help="Return non-zero if unsupported Py* C-API symbols are detected.",
    )
    extension_scan_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    extension_scan_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    internal_batch_parser = subparsers.add_parser(
        "internal-batch-build-server",
        help=argparse.SUPPRESS,
    )
    internal_batch_parser.add_argument(
        "--json", action="store_true", help=argparse.SUPPRESS
    )
    internal_batch_parser.add_argument(
        "--verbose", action="store_true", help=argparse.SUPPRESS
    )

    debug_parser = subparsers.add_parser(
        "debug",
        help="Inspect and retain canonical compiler debug artifacts.",
    )
    debug_subparsers = debug_parser.add_subparsers(
        dest="debug_subcommand",
        title="debug commands",
        required=True,
    )
    for debug_subcommand in DebugSubcommand:
        subparser = debug_subparsers.add_parser(
            debug_subcommand.value,
            help=f"Run canonical `{debug_subcommand.value}` debug flow.",
        )
        _add_debug_shared_selector_args(subparser)
        if debug_subcommand == DebugSubcommand.IR:
            subparser.add_argument("source", help="Python source file to compile.")
            subparser.add_argument(
                "--stage",
                choices=["pre-midend", "post-midend", "all"],
                default="all",
                help="Which compilation stage(s) to dump.",
            )
        if debug_subcommand == DebugSubcommand.REPRO:
            subparser.add_argument(
                "source", help="Python source file to execute as a repro."
            )
        if debug_subcommand == DebugSubcommand.TRACE:
            subparser.add_argument("source", help="Python source file to trace.")
        if debug_subcommand == DebugSubcommand.TRACE:
            subparser.add_argument(
                "--family",
                action="append",
                help=(
                    "Trace family to enable. Repeat for multiple families; "
                    "defaults to all supported trace families."
                ),
            )
            subparser.add_argument(
                "--rebuild",
                action="store_true",
                help="Force a no-cache rebuild before executing the traced repro.",
            )
            subparser.add_argument(
                "--assert-no-pending-on-success",
                action="store_true",
                help="Enable the success-path pending-exception trap during traced execution.",
            )
        if debug_subcommand == DebugSubcommand.REPRO:
            subparser.add_argument(
                "--compare",
                action="store_true",
                help="Compare the repro against CPython instead of only running Molt.",
            )
            subparser.add_argument(
                "--python",
                help="Python executable used for compare mode.",
            )
            subparser.add_argument(
                "--rebuild",
                action="store_true",
                help="Force a no-cache rebuild before executing the repro.",
            )
        if debug_subcommand in {
            DebugSubcommand.REDUCE,
            DebugSubcommand.BISECT,
        }:
            subparser.add_argument(
                "input_path",
                help="Source or prior manifest path to inspect.",
            )
            subparser.add_argument(
                "--oracle-json",
                help="Canonical reduction/bisection oracle as a JSON object.",
            )
            subparser.add_argument(
                "--oracle-file",
                help="Path to a JSON file containing the canonical oracle.",
            )
            subparser.add_argument(
                "--eval-command",
                help=(
                    "Command executed for each candidate. It receives context via "
                    "MOLT_DEBUG_EVAL_* environment variables and may emit JSON on stdout."
                ),
            )
            subparser.add_argument(
                "--eval-timeout",
                type=int,
                default=30,
                help="Per-candidate evaluator timeout in seconds.",
            )
        if debug_subcommand == DebugSubcommand.BISECT:
            subparser.add_argument(
                "--passes",
                help="Comma-separated pass list for first-bad-pass bisection.",
            )
            subparser.add_argument(
                "--baseline-json",
                help="Baseline backend/profile/IC configuration as JSON.",
            )
            subparser.add_argument(
                "--failing-json",
                help="Known failing backend/profile/IC configuration as JSON.",
            )
        if debug_subcommand == DebugSubcommand.VERIFY:
            subparser.add_argument(
                "--require-probe-execution",
                action="store_true",
                help="Require required differential probes to have executed successfully.",
            )
            subparser.add_argument(
                "--probe-rss-metrics",
                help="Path to rss_metrics.jsonl from differential runs.",
            )
            subparser.add_argument(
                "--probe-run-id",
                help="Optional differential run_id to validate for probe execution.",
            )
            subparser.add_argument(
                "--failure-queue",
                help="Path to the differential failure queue file.",
            )
        if debug_subcommand == DebugSubcommand.DIFF:
            subparser.add_argument(
                "summary_path",
                help="Path to a diff summary.json artifact to inspect.",
            )
            subparser.add_argument(
                "--failure-queue",
                help="Optional path to the diff failure queue file.",
            )
        if debug_subcommand == DebugSubcommand.PERF:
            subparser.add_argument(
                "files",
                nargs="+",
                help="Profile JSON/log files containing runtime feedback.",
            )

    check_parser = subparsers.add_parser(
        "check",
        help="Type-check without compiling",
        description=(
            "Analyze a Python file or package and emit type facts without compiling.\n"
            "Type facts can be fed into `molt build --type-facts` for guided specialization."
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "Examples:\n"
            "  molt check src/app.py                  Type-check a file\n"
            "  molt check src/                        Type-check a package directory\n"
            "  molt check src/app.py --strict         Emit strict-tier type facts\n"
            "  molt check src/app.py --output facts.json\n"
            "                                         Write facts to a custom path\n"
        ),
    )
    check_parser.add_argument("path", help="Python file or package directory")
    check_parser.add_argument(
        "--output",
        default="type_facts.json",
        help="Output path for type facts JSON.",
    )
    check_parser.add_argument(
        "--strict",
        action="store_true",
        help="Mark facts as trusted (strict tier).",
    )
    check_parser.add_argument(
        "--deterministic",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require deterministic inputs (lockfiles).",
    )
    check_parser.add_argument(
        "--deterministic-warn",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Warn instead of failing when deterministic lockfile checks fail.",
    )
    check_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    check_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    run_parser = subparsers.add_parser(
        "run",
        help="Build and run a Python program",
        description=(
            "Compile a Python file with Molt and execute it.\n"
            "Supports native, WASM (via wasmtime), and Luau (via lune) targets."
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "Examples:\n"
            "  molt run app.py                       Build and run natively\n"
            "  molt run app.py --release              Optimized build and run\n"
            "  molt run app.py --target wasm          Build and run with wasmtime\n"
            "  molt run app.py --target luau          Build and run with lune\n"
            "  molt run app.py --target mlir          Build and JIT via MLIR\n"
            "  molt run app.py -- --arg1 val          Pass args to your script\n"
        ),
    )
    run_parser.add_argument("file", nargs="?", help="Path to Python source")
    run_parser.add_argument(
        "--module",
        help="Entry module name (uses pkg.__main__ when present).",
    )
    run_parser.add_argument(
        "--target",
        default=None,
        help=(
            "Build target: native (default), wasm (build + run with wasmtime), "
            "luau (build + run with lune), mlir (build + JIT via MLIR), "
            "or a target triple."
        ),
    )
    run_parser.add_argument(
        "--release",
        action="store_true",
        default=False,
        help="Optimized release build (alias for --build-profile release).",
    )
    run_parser.add_argument(
        "--build-arg",
        action="append",
        default=[],
        help="Extra args passed to `molt build`.",
    )
    run_parser.add_argument(
        "--python-version",
        default=None,
        help=("Target Python semantics for the build side (3.12, 3.13, or 3.14)."),
    )
    run_parser.add_argument(
        "--profile",
        "--build-profile",
        choices=["dev", "release"],
        default=None,
        help="Build profile passed to `molt build` (default: dev).",
    )
    run_parser.add_argument(
        "--rebuild",
        action="store_true",
        help="Disable build cache for `molt build`.",
    )
    run_parser.add_argument(
        "--timing",
        action="store_true",
        help="Emit timing summary (compile + run).",
    )
    run_parser.add_argument(
        "--capabilities",
        help="Capability profiles/tokens or path to manifest (toml/json).",
    )
    run_parser.add_argument(
        "--capability-manifest",
        help="Path to a capability manifest file (toml/json/yaml) for runtime configuration.",
    )
    run_parser.add_argument(
        "--require-signed-manifest",
        action="store_true",
        default=False,
        help="Reject unsigned capability manifests. Requires --capability-manifest.",
    )
    run_parser.add_argument(
        "--audit-log",
        metavar="SINK:OUTPUT",
        help="Enable audit logging (e.g., 'jsonl:stderr', 'stderr:stderr')",
    )
    run_parser.add_argument(
        "--io-mode",
        choices=["real", "virtual", "callback"],
        default=None,
        help="IO mode: real (default), virtual (sandbox), callback (host-mediated)",
    )
    run_parser.add_argument(
        "--type-gate",
        action="store_true",
        default=False,
        help="Reject compilation if capability-touching code paths contain untyped variables",
    )
    run_parser.add_argument(
        "--trusted",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Disable capability checks for trusted deployments.",
    )
    run_parser.add_argument(
        "--backend",
        choices=["cranelift", "llvm", "auto"],
        default=None,
        help="Compilation backend passed to `molt build` (auto=cranelift; llvm is opt-in and requires an LLVM toolchain).",
    )
    run_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    run_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )
    run_parser.add_argument(
        "script_args",
        nargs=argparse.REMAINDER,
        help="Arguments passed to the script (use -- to separate).",
    )

    repl_parser = subparsers.add_parser(
        "repl",
        help="Start the guarded Molt REPL",
        description=(
            "Start an interactive Molt REPL. Each submitted snippet is compiled "
            "and executed through the shared adaptive memory guard."
        ),
    )
    repl_parser.add_argument(
        "--capabilities",
        help="Capability profiles/tokens or path to manifest (toml/json).",
    )
    repl_parser.add_argument(
        "--io-mode",
        choices=["real", "virtual", "callback"],
        default="real",
        help="IO mode: real (default), virtual (sandbox), callback (host-mediated)",
    )
    repl_parser.add_argument(
        "--molt-cmd",
        help=(
            "Override the Molt command used for snippet execution. Defaults to "
            "the current Python interpreter running `-m molt.cli`."
        ),
    )
    repl_parser.add_argument(
        "--timeout-sec",
        type=float,
        default=None,
        help="Per-snippet timeout in seconds (default: MOLT_REPL_TIMEOUT_SEC or 30).",
    )

    compare_parser = subparsers.add_parser(
        "compare",
        help="Compare CPython vs Molt output",
        description="Build and run a Python file with both CPython and Molt, then compare output.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "Examples:\n"
            "  molt compare app.py                    Compare output side by side\n"
            "  molt compare app.py --python 3.13      Compare against Python 3.13\n"
            "  molt compare app.py -- --flag           Pass args to both interpreters\n"
        ),
    )
    compare_parser.add_argument("file", nargs="?", help="Path to Python source")
    compare_parser.add_argument(
        "--module",
        help="Entry module name (uses pkg.__main__ when present).",
    )
    compare_parser.add_argument(
        "--python",
        help="Python interpreter (path) or version (e.g. 3.12).",
    )
    compare_parser.add_argument(
        "--python-version",
        help="Python version alias (e.g. 3.12).",
    )
    compare_parser.add_argument(
        "--build-arg",
        action="append",
        default=[],
        help="Extra args passed to `molt build` for the Molt side.",
    )
    compare_parser.add_argument(
        "--profile",
        "--build-profile",
        choices=["dev", "release"],
        default=None,
        help="Build profile passed to `molt build` (default: dev).",
    )
    compare_parser.add_argument(
        "--rebuild",
        action="store_true",
        help="Disable build cache for the Molt build.",
    )
    compare_parser.add_argument(
        "--capabilities",
        help="Capability profiles/tokens or path to manifest (toml/json).",
    )
    compare_parser.add_argument(
        "--trusted",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Disable capability checks for trusted deployments.",
    )
    compare_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    compare_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )
    compare_parser.add_argument(
        "script_args",
        nargs=argparse.REMAINDER,
        help="Arguments passed to the script (use -- to separate).",
    )

    parity_run_parser = subparsers.add_parser(
        "parity-run", help="Run the entrypoint with CPython (no Molt compilation)"
    )
    parity_run_parser.add_argument("file", nargs="?", help="Path to Python source")
    parity_run_parser.add_argument(
        "--module",
        help="Entry module name (uses pkg.__main__ when present).",
    )
    parity_run_parser.add_argument(
        "--python",
        help="Python interpreter (path) or version (e.g. 3.12).",
    )
    parity_run_parser.add_argument(
        "--python-version",
        help="Python version alias (e.g. 3.12).",
    )
    parity_run_parser.add_argument(
        "--timing",
        action="store_true",
        help="Emit timing summary for the CPython run.",
    )
    parity_run_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    parity_run_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )
    parity_run_parser.add_argument(
        "script_args",
        nargs=argparse.REMAINDER,
        help="Arguments passed to the script (use -- to separate).",
    )

    test_parser = subparsers.add_parser(
        "test",
        help="Discover and run tests",
        description=(
            "Discover and run test suites.\n"
            "Supports Molt's built-in dev suite, CPython differential tests, and pytest."
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "Examples:\n"
            "  molt test                             Run the default dev test suite\n"
            "  molt test --suite diff                Run differential tests against CPython\n"
            "  molt test --suite pytest              Run tests with pytest\n"
            "  molt test tests/test_math.py          Run a specific test file\n"
            "  molt test --suite diff --profile release\n"
            "                                        Diff tests with release builds\n"
        ),
    )
    test_parser.add_argument(
        "--suite",
        choices=["dev", "diff", "pytest"],
        default="dev",
        help="Test suite to run.",
    )
    test_parser.add_argument(
        "--python-version",
        help="Python version for diff suite (e.g. 3.13).",
    )
    test_parser.add_argument(
        "--profile",
        "--build-profile",
        choices=["dev", "release"],
        default=None,
        help="Build profile for Molt builds in suite=diff (default: dev).",
    )
    test_parser.add_argument(
        "--trusted",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Disable capability checks for trusted deployments.",
    )
    test_parser.add_argument("path", nargs="?", help="Optional test path.")
    test_parser.add_argument(
        "pytest_args",
        nargs=argparse.REMAINDER,
        help="Extra pytest args when --suite pytest (use -- to separate).",
    )
    test_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    test_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    diff_parser = subparsers.add_parser(
        "diff",
        help="Run differential tests against CPython",
        description="Run differential tests that compare Molt output against CPython.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "Examples:\n"
            "  molt diff                              Run all diff tests\n"
            "  molt diff tests/parity/               Run diff tests in a directory\n"
            "  molt diff --python-version 3.13        Test against Python 3.13\n"
        ),
    )
    diff_parser.add_argument("path", nargs="?", help="File or directory to test.")
    diff_parser.add_argument(
        "--python-version", help="Python version to test against (e.g. 3.13)."
    )
    diff_parser.add_argument(
        "--profile",
        "--build-profile",
        choices=["dev", "release"],
        default=None,
        help="Build profile for Molt builds in the diff harness (default: dev).",
    )
    diff_parser.add_argument(
        "--trusted",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Disable capability checks for trusted deployments.",
    )
    diff_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    diff_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    bench_parser = subparsers.add_parser(
        "bench",
        help="Run benchmarks",
        description=(
            "Run performance benchmarks.\n"
            "Uses the native bench harness by default, or the WASM harness with --wasm."
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "Examples:\n"
            "  molt bench                             Run all benchmarks\n"
            "  molt bench --wasm                      Run WASM benchmarks\n"
            "  molt bench --script bench/fib.py       Benchmark a custom script\n"
            "  molt bench -- --filter sort             Pass args to bench tool\n"
        ),
    )
    bench_parser.add_argument(
        "--wasm", action="store_true", help="Use the WASM bench harness."
    )
    bench_parser.add_argument(
        "--script",
        action="append",
        dest="bench_script",
        default=[],
        help="Benchmark a custom script path (repeatable).",
    )
    bench_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    bench_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )
    bench_parser.add_argument(
        "bench_args",
        nargs=argparse.REMAINDER,
        help="Arguments passed to the bench tool (use -- to separate).",
    )

    profile_parser = subparsers.add_parser(
        "profile",
        help="Profile benchmarks",
        description="Profile Molt benchmarks with detailed performance instrumentation.",
    )
    profile_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    profile_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )
    profile_parser.add_argument(
        "profile_args",
        nargs=argparse.REMAINDER,
        help="Arguments passed to the profile tool (use -- to separate).",
    )

    lint_parser = subparsers.add_parser(
        "lint",
        help="Run linting checks",
        description="Run Molt-specific linting checks on the project.",
    )
    lint_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    lint_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    setup_parser = subparsers.add_parser(
        "setup",
        help="Prepare the host toolchain and canonical Molt environment",
        description=(
            "Report and remediate the toolchains, environment variables, and\n"
            "backend readiness required for Molt development and release work."
        ),
    )
    setup_parser.add_argument(
        "--strict",
        action="store_true",
        help="Return non-zero exit on missing required setup items.",
    )
    setup_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    setup_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    doctor_parser = subparsers.add_parser(
        "doctor",
        help="Check toolchain setup",
        description=(
            "Verify that the Molt toolchain is installed and configured correctly.\n"
            "Checks for Rust/Cargo, wasm-opt, wasmtime, and other dependencies."
        ),
    )
    doctor_parser.add_argument(
        "--strict",
        action="store_true",
        help="Return non-zero exit on missing requirements.",
    )
    doctor_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    doctor_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    update_parser = subparsers.add_parser(
        "update",
        help="Refresh toolchains and dependency state",
        description=(
            "Refresh repo-level toolchains and dependency state.\n"
            "By default this updates rustup-managed toolchains plus Cargo/uv lockfiles.\n"
            "Use --all to also upgrade Rust dependency requirements in Cargo.toml."
        ),
    )
    update_parser.add_argument(
        "--all",
        action="store_true",
        help="Include manifest requirement upgrades (may be breaking).",
    )
    update_parser.add_argument(
        "--toolchains",
        action=argparse.BooleanOptionalAction,
        default=True,
        help="Refresh rustup-managed toolchains and wasm targets (default: enabled).",
    )
    update_parser.add_argument(
        "--locks",
        action=argparse.BooleanOptionalAction,
        default=True,
        help="Refresh Cargo.lock and uv.lock (default: enabled).",
    )
    update_parser.add_argument(
        "--manifests",
        action=argparse.BooleanOptionalAction,
        default=False,
        help="Upgrade Rust dependency requirements in Cargo.toml files.",
    )
    update_parser.add_argument(
        "--check",
        action="store_true",
        help="Print the planned update steps without executing them.",
    )
    update_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    update_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    validate_parser = subparsers.add_parser(
        "validate",
        help="Run the canonical end-to-end local validation matrix",
        description=(
            "Run the release-readiness matrix across CLI smoke, backend parity,\n"
            "conformance, and benchmark lanes."
        ),
    )
    validate_parser.add_argument(
        "--suite",
        choices=_VALIDATE_SUITE_CHOICES,
        default="full",
        help="Validation scope (default: full).",
    )
    validate_parser.add_argument(
        "--backend",
        choices=["all", "native", "llvm", "wasm", "luau"],
        default="all",
        help="Restrict validation to one backend family.",
    )
    validate_parser.add_argument(
        "--profile",
        choices=["all", "dev", "release"],
        default="all",
        help="Restrict validation to one build profile where applicable.",
    )
    validate_parser.add_argument(
        "--check",
        action="store_true",
        help="Print the validation plan without executing it.",
    )
    validate_parser.add_argument(
        "--summary-out",
        help=(
            "Write the validation JSON summary to this path. Executed runs default "
            "to logs/validate-<suite>-<backend>-<profile>.json; check-only runs "
            "write only when this option is provided."
        ),
    )
    validate_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    validate_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    package_parser = subparsers.add_parser(
        "package", help="Bundle a distributable package"
    )
    package_parser.add_argument("artifact", help="Path to the package artifact.")
    package_parser.add_argument(
        "manifest",
        help=(
            "Path to manifest JSON (fields per "
            "docs/spec/areas/compat/contracts/package_abi_contract.md)."
        ),
    )
    package_parser.add_argument(
        "--output",
        help="Output .moltpkg path (default dist/<name>-<version>-<target>.moltpkg).",
    )
    package_parser.add_argument(
        "--deterministic",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require deterministic package metadata.",
    )
    package_parser.add_argument(
        "--deterministic-warn",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Warn instead of failing when deterministic lockfile checks fail.",
    )
    package_parser.add_argument(
        "--capabilities",
        help="Capability profiles/tokens or path to manifest (toml/json).",
    )
    package_parser.add_argument(
        "--sbom",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Emit a CycloneDX SBOM sidecar (default: enabled).",
    )
    package_parser.add_argument(
        "--sbom-output",
        help="Override the SBOM output path (defaults next to the package).",
    )
    package_parser.add_argument(
        "--sbom-format",
        choices=["cyclonedx", "spdx"],
        default="cyclonedx",
        help="SBOM format to emit (default: cyclonedx).",
    )
    package_parser.add_argument(
        "--signature",
        help="Path to a signature file to attach and record in metadata.",
    )
    package_parser.add_argument(
        "--signature-output",
        help="Override the signature sidecar output path (defaults next to the package).",
    )
    package_parser.add_argument(
        "--sign",
        action=argparse.BooleanOptionalAction,
        default=False,
        help="Sign the artifact with cosign or codesign.",
    )
    package_parser.add_argument(
        "--signer",
        choices=["auto", "cosign", "codesign"],
        default="auto",
        help="Select the signing tool (default: auto).",
    )
    package_parser.add_argument(
        "--signing-key",
        help="Signing key path for cosign (or set COSIGN_KEY).",
    )
    package_parser.add_argument(
        "--signing-identity",
        help="Signing identity for codesign (or set MOLT_CODESIGN_IDENTITY).",
    )
    package_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    package_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    publish_parser = subparsers.add_parser("publish", help="Publish to registry")
    publish_parser.add_argument("package", help="Path to the .moltpkg file.")
    publish_parser.add_argument(
        "--registry",
        default="dist/registry",
        help="Registry directory, file path, or HTTP(S) URL.",
    )
    publish_parser.add_argument(
        "--registry-token",
        help=(
            "Bearer token for remote registry auth (or MOLT_REGISTRY_TOKEN; "
            "prefix @ for file)."
        ),
    )
    publish_parser.add_argument(
        "--registry-user",
        help="Username for basic auth (or MOLT_REGISTRY_USER).",
    )
    publish_parser.add_argument(
        "--registry-password",
        help=(
            "Password for basic auth (or MOLT_REGISTRY_PASSWORD; prefix @ for file)."
        ),
    )
    publish_parser.add_argument(
        "--registry-timeout",
        type=float,
        help="Registry request timeout in seconds (or MOLT_REGISTRY_TIMEOUT).",
    )
    publish_parser.add_argument(
        "--dry-run", action="store_true", help="Print the publish plan only."
    )
    publish_parser.add_argument(
        "--deterministic",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Verify package determinism before publishing.",
    )
    publish_parser.add_argument(
        "--deterministic-warn",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Warn instead of failing when deterministic lockfile checks fail.",
    )
    publish_parser.add_argument(
        "--capabilities",
        help="Capability profiles/tokens or path to manifest (toml/json).",
    )
    publish_parser.add_argument(
        "--require-signature",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require a package signature when publishing.",
    )
    publish_parser.add_argument(
        "--verify-signature",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Verify package signatures when publishing.",
    )
    publish_parser.add_argument(
        "--trusted-signers",
        help="Path to a trust policy for allowed signers.",
    )
    publish_parser.add_argument(
        "--signer",
        choices=["auto", "cosign", "codesign"],
        default="auto",
        help="Select the verification tool (default: auto).",
    )
    publish_parser.add_argument(
        "--signing-key",
        help="Verification key path for cosign (or set COSIGN_KEY).",
    )
    publish_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    publish_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    verify_parser = subparsers.add_parser(
        "verify", help="Verify a package manifest and checksum"
    )
    verify_parser.add_argument(
        "--package",
        help="Path to the .moltpkg archive (alternative to --manifest/--artifact).",
    )
    verify_parser.add_argument("--manifest", help="Manifest JSON path.")
    verify_parser.add_argument("--artifact", help="Artifact path.")
    verify_parser.add_argument(
        "--require-checksum",
        action="store_true",
        help="Fail when checksum is missing.",
    )
    verify_parser.add_argument(
        "--extension-metadata",
        action=argparse.BooleanOptionalAction,
        default=None,
        help=(
            "Treat manifest as extension metadata and enforce extension ABI/wheel "
            "checks (default: auto-detect from manifest keys)."
        ),
    )
    verify_parser.add_argument(
        "--require-extension-capabilities",
        action="store_true",
        help="Fail when extension manifest capability list is empty.",
    )
    verify_parser.add_argument(
        "--require-extension-abi",
        help="Require an exact extension molt_c_api_version match.",
    )
    verify_parser.add_argument(
        "--require-deterministic",
        action="store_true",
        help="Fail when manifest is not deterministic.",
    )
    verify_parser.add_argument(
        "--require-signature",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require a package signature.",
    )
    verify_parser.add_argument(
        "--verify-signature",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Verify package signatures when present.",
    )
    verify_parser.add_argument(
        "--trusted-signers",
        help="Path to a trust policy for allowed signers.",
    )
    verify_parser.add_argument(
        "--signer",
        choices=["auto", "cosign", "codesign"],
        default="auto",
        help="Select the verification tool (default: auto).",
    )
    verify_parser.add_argument(
        "--signing-key",
        help="Verification key path for cosign (or set COSIGN_KEY).",
    )
    verify_parser.add_argument(
        "--capabilities",
        help="Capability profiles/tokens or path to manifest (toml/json).",
    )
    verify_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    verify_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    deps_parser = subparsers.add_parser("deps", help="Show dependency info")
    deps_parser.add_argument(
        "--include-dev", action="store_true", help="Include dev dependencies"
    )
    deps_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    deps_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )
    vendor_parser = subparsers.add_parser(
        "vendor", help="Vendor pure-Python dependencies"
    )
    vendor_parser.add_argument(
        "--include-dev", action="store_true", help="Include dev dependencies"
    )
    vendor_parser.add_argument(
        "--output",
        help="Output directory for vendored artifacts (default: vendor).",
    )
    vendor_parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Show vendoring plan without downloading artifacts.",
    )
    vendor_parser.add_argument(
        "--allow-non-tier-a",
        action="store_true",
        help="Proceed even if non-Tier A dependencies are present.",
    )
    vendor_parser.add_argument(
        "--extras",
        action="append",
        help="Extras to include from project optional-dependencies.",
    )
    vendor_parser.add_argument(
        "--deterministic",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require deterministic inputs (lockfiles).",
    )
    vendor_parser.add_argument(
        "--deterministic-warn",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Warn instead of failing when deterministic lockfile checks fail.",
    )
    vendor_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    vendor_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    install_parser = subparsers.add_parser(
        "install",
        help="Install packages into .molt-venv/ using UV",
        description=(
            "Manage third-party Python packages with UV.\n\n"
            "Without arguments, syncs dependencies from pyproject.toml and\n"
            "requirements.txt into the .molt-venv/ virtual environment.\n"
            "Installed packages are automatically available to `molt build`\n"
            "and `molt run`.\n\n"
            "Use `molt install add <pkg>` to install a package AND persist it\n"
            "to pyproject.toml."
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "Examples:\n"
            "  molt install                        Sync deps from pyproject.toml\n"
            "  molt install requests flask          Install specific packages\n"
            "  molt install -r requirements.txt     Install from requirements file\n"
            "  molt install add requests            Add and persist a dependency\n"
        ),
    )
    install_parser.add_argument(
        "packages",
        nargs="*",
        default=[],
        help=(
            "Package(s) to install (e.g. requests, 'flask>=2.0'), "
            "or 'add <pkg>...' to add and persist to pyproject.toml."
        ),
    )
    install_parser.add_argument(
        "-r",
        "--requirements",
        help="Path to a requirements.txt file.",
    )
    install_parser.add_argument(
        "--sync",
        action="store_true",
        help="Sync venv to match pyproject.toml + requirements.txt.",
    )
    install_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    install_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    clean_parser = subparsers.add_parser(
        "clean",
        help="Dry-run or apply canonical ignored artifact/cache cleanup",
    )
    clean_parser.add_argument(
        "--apply",
        action="store_true",
        help="Delete ignored artifacts. Default is a dry run.",
    )
    clean_parser.add_argument(
        "--kill-processes",
        action="store_true",
        help="Run the repo process sentinel before cleanup.",
    )
    clean_parser.add_argument(
        "--extra-path",
        action="append",
        default=[],
        help="Additional repo-relative git-clean pathspec. Still removes ignored files only.",
    )
    clean_parser.add_argument(
        "--list-paths",
        action="store_true",
        help="Print canonical cleanup pathspecs and exit.",
    )
    clean_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    clean_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    config_parser = subparsers.add_parser("config", help="Show/set configuration")
    config_parser.add_argument(
        "--file",
        help="Resolve project root from a source file path.",
    )
    config_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    config_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    completion_parser = subparsers.add_parser(
        "completion", help="Generate shell completions"
    )
    completion_parser.add_argument(
        "--shell",
        choices=["bash", "zsh", "fish"],
        default="bash",
        help="Shell type to emit.",
    )
    completion_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    completion_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    # --- deploy command ---
    deploy_parser = subparsers.add_parser(
        "deploy",
        help="Build and deploy to a platform",
        description=(
            "Build and deploy a Python program to a target platform.\n"
            "Automatically sets the correct build target and optimization defaults."
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "Examples:\n"
            "  molt deploy cloudflare src/app.py      Deploy to Cloudflare Workers\n"
            "  molt deploy roblox src/game.py          Deploy to Roblox Studio\n"
            "  molt deploy cloudflare app.py --release  Optimized production deploy\n"
            "  molt deploy roblox app.py --roblox-project ./my-game\n"
            "                                          Deploy and copy to Roblox project\n"
            "  molt deploy cloudflare app.py --dry-run  Build only, skip wrangler\n"
            "\n"
            "Platforms:\n"
            "  cloudflare    Build as WASM with --split-runtime, deploy via wrangler\n"
            "  roblox        Build as Luau, optionally copy to a Roblox project dir\n"
        ),
    )
    deploy_parser.add_argument(
        "platform",
        choices=["cloudflare", "roblox"],
        help="Deployment target: cloudflare (WASM Workers) or roblox (Luau).",
    )
    deploy_parser.add_argument("file", nargs="?", help="Path to Python source")
    deploy_parser.add_argument(
        "--module",
        help="Entry module name (uses pkg.__main__ when present).",
    )
    deploy_parser.add_argument(
        "--release",
        action="store_true",
        default=False,
        help="Optimized release build (alias for --build-profile release).",
    )
    deploy_parser.add_argument(
        "--build-profile",
        choices=["dev", "release"],
        default=None,
        help="Build profile for backend/runtime (default: release).",
    )
    deploy_parser.add_argument(
        "--output",
        help="Output path for the build artifact.",
    )
    deploy_parser.add_argument(
        "--out-dir",
        help="Output directory for build artifacts.",
    )
    deploy_parser.add_argument(
        "--roblox-project",
        help="Path to the Roblox project directory to copy Luau output into.",
    )
    deploy_parser.add_argument(
        "--wrangler-args",
        default="",
        help="Extra arguments passed to wrangler deploy (cloudflare only).",
    )
    deploy_parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Build only; do not run wrangler deploy or copy to project.",
    )
    deploy_parser.add_argument(
        "--build-arg",
        action="append",
        default=[],
        help="Extra args passed to `molt build`.",
    )
    deploy_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    deploy_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    # --- harness command ---
    harness_parser = subparsers.add_parser(
        "harness",
        help="Run the Molt quality harness",
        description="Run layered quality checks (compile, lint, test, fuzz, etc.).",
    )
    harness_parser.add_argument(
        "profile",
        nargs="?",
        default="standard",
        choices=["quick", "standard", "deep"],
        help="Test profile (default: standard).",
    )
    harness_parser.add_argument(
        "--no-fail-fast",
        action="store_true",
        help="Continue running layers after a failure.",
    )
    harness_parser.add_argument(
        "--json", action="store_true", help="Print JSON report to stdout."
    )
    harness_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    return parser
