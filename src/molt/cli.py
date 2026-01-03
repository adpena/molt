import argparse
import ast
import json
import os
import platform
import re
import shlex
import subprocess
import sys
import tomllib
from pathlib import Path
from typing import Any, Literal

from molt.frontend import SimpleTIRGenerator

Target = Literal["native", "wasm"]
ParseCodec = Literal["msgpack", "cbor", "json"]
TypeHintPolicy = Literal["ignore", "trust", "check"]


def _latest_mtime(paths: list[Path]) -> float:
    latest = 0.0
    for path in paths:
        if path.is_dir():
            for item in path.rglob("*"):
                if item.is_file():
                    latest = max(latest, item.stat().st_mtime)
        elif path.exists():
            latest = max(latest, path.stat().st_mtime)
    return latest


def _ensure_runtime_lib(runtime_lib: Path) -> bool:
    sources = [
        Path("runtime/molt-runtime/src"),
        Path("runtime/molt-runtime/Cargo.toml"),
        Path("runtime/molt-obj-model/src"),
        Path("runtime/molt-obj-model/Cargo.toml"),
    ]
    latest_src = _latest_mtime(sources)
    lib_mtime = runtime_lib.stat().st_mtime if runtime_lib.exists() else 0.0
    if lib_mtime >= latest_src:
        return True
    build = subprocess.run(["cargo", "build", "-p", "molt-runtime", "--release"])
    if build.returncode != 0:
        print("Runtime build failed", file=sys.stderr)
        return False
    return True


def _strip_arch_flags(args: list[str]) -> list[str]:
    cleaned: list[str] = []
    skip_next = False
    for arg in args:
        if skip_next:
            skip_next = False
            continue
        if arg == "-arch":
            skip_next = True
            continue
        if arg.startswith("-arch="):
            continue
        cleaned.append(arg)
    return cleaned


def _detect_macos_arch(obj_path: Path) -> str | None:
    try:
        result = subprocess.run(
            ["lipo", "-archs", str(obj_path)],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return None
    if result.returncode != 0:
        return None
    archs = result.stdout.strip().split()
    return archs[0] if archs else None


def build(
    file_path: str,
    target: Target = "native",
    parse_codec: ParseCodec = "msgpack",
    type_hint_policy: TypeHintPolicy = "ignore",
) -> int:
    source_path = Path(file_path)
    if not source_path.exists():
        print(f"File not found: {source_path}", file=sys.stderr)
        return 2

    source = source_path.read_text()

    # 1. Frontend: Python -> JSON IR
    tree = ast.parse(source)
    gen = SimpleTIRGenerator(parse_codec=parse_codec, type_hint_policy=type_hint_policy)
    gen.visit(tree)
    ir = gen.to_json()

    # 2. Backend: JSON IR -> output.o / output.wasm
    cmd = ["cargo", "run", "--quiet", "--package", "molt-backend", "--"]
    if target == "wasm":
        cmd.extend(["--target", "wasm"])

    backend_process = subprocess.run(cmd, input=json.dumps(ir), text=True)
    if backend_process.returncode != 0:
        print("Backend compilation failed", file=sys.stderr)
        return backend_process.returncode or 1

    if target == "wasm":
        print("Successfully built output.wasm")
        return 0

    # 3. Linking: output.o + main.c -> binary
    main_c_content = """
#include <stdio.h>
#include <stdlib.h>
extern void molt_main();
extern int molt_json_parse_scalar(const char* ptr, long len, unsigned long long* out);
extern int molt_msgpack_parse_scalar(const char* ptr, long len, unsigned long long* out);
extern int molt_cbor_parse_scalar(const char* ptr, long len, unsigned long long* out);
extern long molt_get_attr_generic(void* obj, const char* attr, long len);
extern void* molt_alloc(long size);
extern long molt_block_on(void* task);
extern long molt_async_sleep(void* obj);
extern void molt_spawn(void* task);
extern void* molt_chan_new();
extern long molt_chan_send(void* chan, long val);
extern long molt_chan_recv(void* chan);
extern void molt_print_obj(unsigned long long val);
int main() {
    molt_main();
    return 0;
}
"""
    Path("main_stub.c").write_text(main_c_content)

    output_binary = "hello_molt"
    runtime_lib = Path("target/release/libmolt_runtime.a")
    if not _ensure_runtime_lib(runtime_lib):
        return 1

    cc = os.environ.get("CC", "clang")
    link_cmd = shlex.split(cc)
    cflags = os.environ.get("CFLAGS", "")
    if cflags:
        link_cmd.extend(shlex.split(cflags))
    if sys.platform == "darwin":
        link_cmd = _strip_arch_flags(link_cmd)
        arch = (
            os.environ.get("MOLT_ARCH")
            or _detect_macos_arch(Path("output.o"))
            or platform.machine()
        )
        link_cmd.extend(["-arch", arch])
    link_cmd.extend(["main_stub.c", "output.o", str(runtime_lib), "-o", output_binary])

    link_process = subprocess.run(link_cmd)

    if link_process.returncode == 0:
        print(f"Successfully built {output_binary}")
    else:
        print("Linking failed", file=sys.stderr)

    return link_process.returncode


def deps(include_dev: bool) -> int:
    pyproject = _load_toml(Path("pyproject.toml"))
    lock = _load_toml(Path("uv.lock"))
    deps = _collect_deps(pyproject, include_dev=include_dev)
    packages = _lock_packages(lock)
    allow = _dep_allowlists(pyproject)

    rows: list[tuple[str, str, str, str]] = []
    for dep in deps:
        key = _normalize_name(dep)
        pkg = packages.get(key)
        version = pkg.get("version", "unknown") if pkg else "missing"
        tier, reason = _classify_tier(dep, pkg, allow)
        rows.append((dep, version, tier, reason))

    for name, version, tier, reason in rows:
        print(f"{name} {version} {tier} {reason}")
    return 0


def vendor(include_dev: bool) -> int:
    pyproject = _load_toml(Path("pyproject.toml"))
    lock = _load_toml(Path("uv.lock"))
    deps = _collect_deps(pyproject, include_dev=include_dev)
    packages = _lock_packages(lock)
    allow = _dep_allowlists(pyproject)

    print("Vendoring plan (Tier A only, dry-run):")
    for dep in deps:
        key = _normalize_name(dep)
        pkg = packages.get(key)
        tier, _ = _classify_tier(dep, pkg, allow)
        if tier == "Tier A":
            version = pkg.get("version", "unknown") if pkg else "missing"
            print(f"- {dep} {version}")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(prog="molt")
    subparsers = parser.add_subparsers(dest="command", required=True)

    build_parser = subparsers.add_parser("build", help="Compile a Python file")
    build_parser.add_argument("file", help="Path to Python source")
    build_parser.add_argument("--target", choices=["native", "wasm"], default="native")
    build_parser.add_argument(
        "--codec",
        choices=["msgpack", "cbor", "json"],
        default="msgpack",
        help="Default structured codec for parse calls (JSON requires explicit flag).",
    )
    build_parser.add_argument(
        "--type-hints",
        choices=["ignore", "trust", "check"],
        default="ignore",
        help="Apply type annotations to guide lowering and specialization.",
    )

    deps_parser = subparsers.add_parser(
        "deps", help="Show dependency compatibility info"
    )
    deps_parser.add_argument(
        "--include-dev", action="store_true", help="Include dev dependencies"
    )
    vendor_parser = subparsers.add_parser(
        "vendor", help="Vendor pure Python dependencies"
    )
    vendor_parser.add_argument(
        "--include-dev", action="store_true", help="Include dev dependencies"
    )

    args = parser.parse_args()

    if args.command == "build":
        return build(args.file, args.target, args.codec, args.type_hints)
    if args.command == "deps":
        return deps(args.include_dev)
    if args.command == "vendor":
        return vendor(args.include_dev)

    return 2


def _load_toml(path: Path) -> dict[str, Any]:
    if not path.exists():
        return {}
    return tomllib.loads(path.read_text())


def _normalize_name(name: str) -> str:
    return re.sub(r"[-_.]+", "-", name).lower()


def _collect_deps(pyproject: dict[str, Any], include_dev: bool) -> list[str]:
    deps: list[str] = []
    deps.extend(pyproject.get("project", {}).get("dependencies", []))
    if include_dev:
        deps.extend(pyproject.get("dependency-groups", {}).get("dev", []))
    return [re.split(r"[<=>\\[\\s;]", dep, 1)[0] for dep in deps]


def _lock_packages(lock: dict[str, Any]) -> dict[str, dict[str, Any]]:
    packages: dict[str, dict[str, Any]] = {}
    for pkg in lock.get("package", []):
        name = _normalize_name(pkg.get("name", ""))
        if name:
            packages[name] = pkg
    return packages


def _classify_tier(
    name: str,
    pkg: dict[str, Any] | None,
    allow: dict[str, set[str]],
) -> tuple[str, str]:
    norm = _normalize_name(name)
    if norm in allow["tier_a"]:
        return "Tier A", _append_feature_notes("allowlisted", pkg)
    if norm in allow["tier_b"]:
        return "Tier B", _append_feature_notes("allowlisted", pkg)
    if norm in allow["tier_c"]:
        return "Tier C", _append_feature_notes("allowlisted", pkg)
    if norm in allow["native_wheels"]:
        return "Tier B", _append_feature_notes("allowlisted native wheels", pkg)

    molt_packages = {"molt_json", "molt_msgpack", "molt_cbor"}
    if norm in molt_packages:
        return "Tier B", _append_feature_notes("molt package", pkg)
    if pkg is None:
        return "Tier A", _append_feature_notes("unresolved (assumed pure python)", pkg)
    source = pkg.get("source", {})
    if source.get("git") or source.get("path"):
        return "Tier A", _append_feature_notes("local/git source", pkg)
    wheels = pkg.get("wheels", [])
    has_universal = any("py3-none-any" in wheel.get("url", "") for wheel in wheels)
    has_abi3 = any("abi3" in wheel.get("url", "") for wheel in wheels)
    if wheels and not has_universal and not has_abi3:
        return "Tier C", _append_feature_notes("platform wheels only", pkg)
    if has_abi3 and not has_universal:
        return "Tier B", _append_feature_notes("abi3 wheels", pkg)
    if wheels:
        return "Tier A", _append_feature_notes("universal wheels", pkg)
    if pkg.get("sdist"):
        return "Tier A", _append_feature_notes("sdist only", pkg)
    return "Tier A", _append_feature_notes("assumed pure python", pkg)


def _dep_allowlists(pyproject: dict[str, Any]) -> dict[str, set[str]]:
    tool_cfg = pyproject.get("tool", {}).get("molt", {}).get("deps", {})
    return {
        "tier_a": {_normalize_name(name) for name in tool_cfg.get("tier_a", [])},
        "tier_b": {_normalize_name(name) for name in tool_cfg.get("tier_b", [])},
        "tier_c": {_normalize_name(name) for name in tool_cfg.get("tier_c", [])},
        "native_wheels": {
            _normalize_name(name) for name in tool_cfg.get("native_wheels", [])
        },
    }


def _append_feature_notes(reason: str, pkg: dict[str, Any] | None) -> str:
    if not pkg:
        return reason
    metadata = pkg.get("metadata", {})
    requires = metadata.get("requires-dist", [])
    markers = any("marker" in dep for dep in requires)
    extras = any("extra" in dep for dep in requires)
    notes: list[str] = []
    if markers:
        notes.append("markers")
    if extras:
        notes.append("extras")
    if notes:
        return f"{reason}; {', '.join(notes)}"
    return reason


if __name__ == "__main__":
    raise SystemExit(main())
