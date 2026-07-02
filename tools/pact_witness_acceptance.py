#!/usr/bin/env python3
from __future__ import annotations

import argparse
from collections.abc import Mapping, Sequence
from datetime import UTC, datetime
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
KERNEL_ROOT = ROOT / "collab" / "pact" / "pact_witness_kernel"
DEFAULT_OUT_DIR = ROOT / "tmp" / "pact_witness_acceptance_queue"
_STATIC_LINK_EXEC_FAILURE_RE = re.compile(
    r"ImportError:\s+"
    r"(?P<module>[A-Za-z_][A-Za-z0-9_.]*):\s+"
    r"(?P<reason>static-link PyModuleDef Py_mod_exec slot returned non-zero[^\r\n]*)"
)


def _run(args: list[str], *, cwd: Path, env: dict[str, str] | None = None) -> None:
    print(f"+ {' '.join(args)}", flush=True)
    subprocess.run(args, cwd=cwd, env=env, check=True)


def _run_capture(
    args: list[str],
    *,
    cwd: Path,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    print(f"+ {' '.join(args)}", flush=True)
    result = subprocess.run(
        args,
        cwd=cwd,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    if result.stdout:
        print(result.stdout, end="" if result.stdout.endswith("\n") else "\n")
    return result


def _node_bin() -> str:
    requested = os.environ.get("MOLT_NODE_BIN", "").strip()
    if requested:
        return requested
    found = shutil.which("node")
    if found:
        return found
    raise SystemExit("node is required to execute the Pact witness WASM artifact")


def _assert_owned_tmp(path: Path) -> Path:
    resolved = path.resolve()
    tmp_root = (ROOT / "tmp").resolve()
    try:
        resolved.relative_to(tmp_root)
    except ValueError as exc:
        raise SystemExit(
            f"Pact witness acceptance out-dir must stay under {tmp_root}: {resolved}"
        ) from exc
    return resolved


def _safe_attempt_slug(raw: str) -> str:
    cleaned = re.sub(r"[^0-9A-Za-z_.-]+", "_", raw.strip()).strip("._-")
    return cleaned or "manual"


def _attempt_slug() -> str:
    run_id = os.environ.get("MOLT_PROOF_QUEUE_RUN_ID", "").strip()
    if run_id:
        return _safe_attempt_slug(run_id)
    stamp = datetime.now(UTC).strftime("%Y%m%dT%H%M%S.%fZ")
    return _safe_attempt_slug(f"manual-{stamp}-{os.getpid()}")


def _prepare_attempt_dirs(out_dir: Path) -> tuple[Path, Path]:
    owned = _assert_owned_tmp(out_dir)
    owned.mkdir(parents=True, exist_ok=True)
    attempts_root = owned / "runs"
    attempts_root.mkdir(parents=True, exist_ok=True)
    base = _attempt_slug()
    attempt_dir = attempts_root / base
    counter = 2
    while attempt_dir.exists():
        attempt_dir = attempts_root / f"{base}-{counter}"
        counter += 1
    attempt_dir.mkdir(parents=True)
    build_dir = attempt_dir / "build"
    run_dir = attempt_dir / "run"
    build_dir.mkdir()
    run_dir.mkdir()
    (owned / "latest_attempt.txt").write_text(str(attempt_dir) + "\n", encoding="utf-8")
    return build_dir, run_dir


def _build_env() -> dict[str, str]:
    env = os.environ.copy()
    src_path = str(ROOT / "src")
    current = env.get("PYTHONPATH", "")
    env["PYTHONPATH"] = src_path if not current else src_path + os.pathsep + current
    return env


def _select_wasm_entry(build_dir: Path) -> Path:
    app_wasm = build_dir / "app.wasm"
    runtime_wasm = build_dir / "molt_runtime.wasm"
    if app_wasm.is_file():
        if not runtime_wasm.is_file():
            raise SystemExit(f"missing split runtime artifact: {runtime_wasm}")
        return app_wasm
    output_wasm = build_dir / "output.wasm"
    if not output_wasm.is_file():
        raise SystemExit(f"missing build artifact: {output_wasm}")
    return output_wasm


def _wasm_run_env(wasm_entry: Path) -> dict[str, str]:
    env = os.environ.copy()
    if wasm_entry.name == "app.wasm":
        env["MOLT_WASM_DIRECT_LINK"] = "1"
        env["MOLT_WASM_PREFER_LINKED"] = "0"
        env["MOLT_RUNTIME_WASM"] = str(wasm_entry.with_name("molt_runtime.wasm"))
    return env


def _build_wasm(build_dir: Path) -> Path:
    env = _build_env()
    _run(
        [
            sys.executable,
            "-m",
            "molt",
            "build",
            "collab/pact/pact_witness_kernel/field_solve.py",
            "--target",
            "wasm",
            "--profile",
            "browser",
            "--wasm-profile",
            "auto",
            "--split-runtime",
            "--out-dir",
            str(build_dir),
        ],
        cwd=ROOT,
        env=env,
    )
    return _select_wasm_entry(build_dir)


def _module_roots_from_env(env: Mapping[str, str]) -> tuple[Path, ...]:
    roots: list[Path] = []
    for raw in env.get("MOLT_MODULE_ROOTS", "").split(os.pathsep):
        stripped = raw.strip()
        if stripped:
            roots.append(Path(stripped))
    return tuple(roots)


def _find_extension_manifests(
    module_name: str,
    module_roots: Sequence[Path],
) -> tuple[Path, ...]:
    leaf = module_name.rsplit(".", 1)[-1]
    sidecar_name = f"{leaf}.molt.wasm.extension_manifest.json"
    direct_rel: Path | None = None
    if "." in module_name:
        module_rel = Path(*module_name.split("."))
        direct_rel = module_rel.with_name(sidecar_name)

    seen: set[Path] = set()
    matches: list[Path] = []
    for root in module_roots:
        if not root.is_dir():
            continue
        candidates: list[Path] = []
        if direct_rel is not None:
            candidates.append(root / direct_rel)
        candidates.extend(root.rglob(sidecar_name))
        for candidate in candidates:
            if not candidate.is_file():
                continue
            resolved = candidate.resolve()
            if resolved in seen:
                continue
            seen.add(resolved)
            matches.append(resolved)
    return tuple(matches)


def _load_json_object(path: Path) -> Mapping[str, Any] | None:
    try:
        loaded = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError):
        return None
    return loaded if isinstance(loaded, Mapping) else None


def _string_list(value: object) -> tuple[str, ...]:
    if not isinstance(value, list):
        return ()
    return tuple(str(item) for item in value if isinstance(item, str))


def _manifest_required_capsules(manifest: Mapping[str, Any]) -> tuple[str, ...]:
    capsules: set[str] = set()
    object_closure = manifest.get("object_closure")
    if not isinstance(object_closure, Mapping):
        return ()
    capsules.update(_string_list(object_closure.get("required_capsules")))
    objects = object_closure.get("objects")
    if isinstance(objects, list):
        for item in objects:
            if isinstance(item, Mapping):
                capsules.update(_string_list(item.get("required_capsules")))
    return tuple(sorted(capsules))


def _object_closure_summary(manifest: Mapping[str, Any]) -> dict[str, Any]:
    object_closure = manifest.get("object_closure")
    if not isinstance(object_closure, Mapping):
        return {"present": False}
    return {
        "present": True,
        "keys": sorted(str(key) for key in object_closure),
        "object_count": len(object_closure.get("objects") or [])
        if isinstance(object_closure.get("objects"), list)
        else 0,
        "runtime_symbol_count": len(
            _string_list(object_closure.get("runtime_symbols"))
        ),
        "undefined_symbol_count": len(
            _string_list(object_closure.get("undefined_symbols"))
        ),
        "defined_symbol_count": len(
            _string_list(object_closure.get("defined_symbols"))
        ),
        "required_capsule_count": len(_manifest_required_capsules(manifest)),
        "required_c_api_symbol_count": len(
            _string_list(object_closure.get("required_c_api_symbols"))
        ),
    }


def _source_capsule_line_hits(
    source_text: str,
    tokens: Sequence[str],
) -> list[dict[str, Any]]:
    hits: list[dict[str, Any]] = []
    for line_number, line in enumerate(source_text.splitlines(), start=1):
        for token in tokens:
            if re.search(rf"\b{re.escape(token)}\b", line):
                hits.append(
                    {
                        "line": line_number,
                        "token": token,
                        "text": line.strip()[:160],
                    }
                )
                break
        if len(hits) >= 12:
            break
    return hits


def _source_required_capsules(
    source_paths: Sequence[object],
) -> tuple[tuple[str, ...], list[dict[str, Any]]]:
    from molt.cli.source_extensions import source_extension_required_capsule_imports

    required: set[str] = set()
    reports: list[dict[str, Any]] = []
    for raw_path in source_paths:
        if not isinstance(raw_path, str):
            continue
        source_path = Path(raw_path)
        report: dict[str, Any] = {"path": str(source_path)}
        try:
            text = source_path.read_text(encoding="utf-8", errors="replace")
        except OSError as exc:
            report["error"] = str(exc)
            reports.append(report)
            continue
        imports_by_capsule = source_extension_required_capsule_imports(text)
        required.update(imports_by_capsule)
        report["required_capsules"] = sorted(imports_by_capsule)
        report["capsule_import_tokens"] = {
            capsule: list(tokens) for capsule, tokens in imports_by_capsule.items()
        }
        report["line_hits"] = [
            {
                "capsule": capsule,
                "hits": _source_capsule_line_hits(text, tokens),
            }
            for capsule, tokens in imports_by_capsule.items()
        ]
        reports.append(report)
    return tuple(sorted(required)), reports


def _static_extension_init_failure_report(
    *,
    output_text: str,
    env: Mapping[str, str],
) -> dict[str, Any] | None:
    match = _STATIC_LINK_EXEC_FAILURE_RE.search(output_text)
    if match is None:
        return None
    module_name = match.group("module")
    module_roots = _module_roots_from_env(env)
    manifest_matches = []
    for manifest_path in _find_extension_manifests(module_name, module_roots):
        manifest = _load_json_object(manifest_path)
        if manifest is None:
            manifest_matches.append(
                {"manifest_path": str(manifest_path), "error": "invalid manifest JSON"}
            )
            continue
        source_required, source_reports = _source_required_capsules(
            manifest.get("sources") if isinstance(manifest.get("sources"), list) else ()
        )
        manifest_required = _manifest_required_capsules(manifest)
        manifest_matches.append(
            {
                "manifest_path": str(manifest_path),
                "manifest_module": manifest.get("module"),
                "extension": manifest.get("extension"),
                "init_symbol": manifest.get("init_symbol"),
                "runtime_linkage": manifest.get("runtime_linkage"),
                "artifact_kind": manifest.get("artifact_kind"),
                "object_closure": _object_closure_summary(manifest),
                "manifest_required_capsules": list(manifest_required),
                "source_required_capsules": list(source_required),
                "missing_manifest_required_capsules": sorted(
                    set(source_required) - set(manifest_required)
                ),
                "sources": source_reports,
            }
        )
    return {
        "kind": "static_extension_init_failure",
        "failure": {
            "module": module_name,
            "reason": match.group("reason"),
        },
        "module_roots": [str(path) for path in module_roots],
        "manifest_matches": manifest_matches,
    }


def _emit_static_extension_init_failure_summary(
    report: Mapping[str, Any],
    report_path: Path,
) -> None:
    failure = (
        report.get("failure") if isinstance(report.get("failure"), Mapping) else {}
    )
    print("Pact witness static extension init diagnostic:", flush=True)
    print(
        f"  failure: {failure.get('module', '<unknown>')}: "
        f"{failure.get('reason', '<unknown>')}",
        flush=True,
    )
    matches = report.get("manifest_matches")
    if not isinstance(matches, list) or not matches:
        print("  manifest: no matching staged extension manifest found", flush=True)
        print(f"  diagnostic_json={report_path}", flush=True)
        return
    for item in matches[:3]:
        if not isinstance(item, Mapping):
            continue
        print(f"  manifest: {item.get('manifest_path')}", flush=True)
        print(
            "  extension: "
            f"{item.get('manifest_module')} init={item.get('init_symbol')} "
            f"linkage={item.get('runtime_linkage')}",
            flush=True,
        )
        missing = item.get("missing_manifest_required_capsules")
        if isinstance(missing, list) and missing:
            print(
                "  manifest/source drift: missing "
                f"object_closure.required_capsules {missing}",
                flush=True,
            )
        for source in (
            item.get("sources", []) if isinstance(item.get("sources"), list) else []
        ):
            if not isinstance(source, Mapping):
                continue
            line_hits = source.get("line_hits")
            if not isinstance(line_hits, list):
                continue
            for line_group in line_hits:
                if not isinstance(line_group, Mapping):
                    continue
                hits = line_group.get("hits")
                if not isinstance(hits, list) or not hits:
                    continue
                first = hits[0]
                if isinstance(first, Mapping):
                    print(
                        "  source capsule import: "
                        f"{source.get('path')}:{first.get('line')} "
                        f"{first.get('token')}",
                        flush=True,
                    )
    print(f"  diagnostic_json={report_path}", flush=True)


def _write_static_extension_init_failure_diagnostic(
    *,
    output_text: str,
    run_dir: Path,
    env: Mapping[str, str],
) -> Path | None:
    report = _static_extension_init_failure_report(output_text=output_text, env=env)
    if report is None:
        return None
    report_path = run_dir / "static_extension_init_failure.json"
    report_path.write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    _emit_static_extension_init_failure_summary(report, report_path)
    return report_path


def _run_candidate(output_wasm: Path, run_dir: Path) -> Path:
    fixture = KERNEL_ROOT / "lstar_sample.npz"
    if not fixture.is_file():
        raise SystemExit(f"missing Pact fixture: {fixture}")
    shutil.copy2(fixture, run_dir / "lstar_sample.npz")
    raw_output = run_dir / "reference_outputs.npz"
    candidate = run_dir / "candidate_outputs.npz"
    raw_output.unlink(missing_ok=True)
    candidate.unlink(missing_ok=True)
    node_args = [_node_bin(), str(ROOT / "wasm" / "run_wasm.js"), str(output_wasm)]
    env = _wasm_run_env(output_wasm)
    result = _run_capture(node_args, cwd=run_dir, env=env)
    if result.returncode != 0:
        _write_static_extension_init_failure_diagnostic(
            output_text=result.stdout,
            run_dir=run_dir,
            env=env,
        )
        raise subprocess.CalledProcessError(
            result.returncode,
            node_args,
            output=result.stdout,
        )
    if not raw_output.is_file():
        raise SystemExit(
            "Pact witness WASM execution did not produce reference_outputs.npz"
        )
    raw_output.replace(candidate)
    print(f"candidate_outputs={candidate}", flush=True)
    return candidate


def _check_parity(candidate: Path) -> None:
    reference = KERNEL_ROOT / "reference_outputs.npz"
    if not reference.is_file():
        raise SystemExit(f"missing Pact reference oracle: {reference}")
    _run(
        [
            sys.executable,
            str(KERNEL_ROOT / "check_parity.py"),
            str(candidate),
            str(reference),
        ],
        cwd=candidate.parent,
        env=_build_env(),
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Build, execute, and parity-check the Pact Kernel A WASM witness."
    )
    parser.add_argument(
        "--out-dir",
        type=Path,
        default=DEFAULT_OUT_DIR,
        help="Owned tmp artifact root for build/, run/, and candidate_outputs.npz.",
    )
    args = parser.parse_args(argv)

    build_dir, run_dir = _prepare_attempt_dirs(args.out_dir)

    output_wasm = _build_wasm(build_dir)
    candidate = _run_candidate(output_wasm, run_dir)
    _check_parity(candidate)
    print("pact witness acceptance PASS", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
