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

from molt.compat import CompatibilityError
from molt.frontend import SimpleTIRGenerator
from molt.type_facts import (
    collect_type_facts_from_paths,
    load_type_facts,
    write_type_facts,
)

Target = Literal["native", "wasm"]
ParseCodec = Literal["msgpack", "cbor", "json"]
TypeHintPolicy = Literal["ignore", "trust", "check"]
FallbackPolicy = Literal["error", "bridge"]
STUB_MODULES = {"molt_buffer", "molt_cbor", "molt_json", "molt_msgpack"}
STUB_PARENT_MODULES = {"molt"}


def _module_name_from_path(path: Path, roots: list[Path], stdlib_root: Path) -> str:
    resolved = path.resolve()
    rel = None
    try:
        rel = resolved.relative_to(stdlib_root.resolve())
    except ValueError:
        rel = None
    if rel is not None:
        if rel.name == "__init__.py":
            rel = rel.parent
        else:
            rel = rel.with_suffix("")
        if rel.parts:
            return ".".join(rel.parts)
        rel = None
    if rel is None:
        for root in roots:
            try:
                rel = resolved.relative_to(root.resolve())
                break
            except ValueError:
                continue
    if rel is None:
        rel = resolved.with_suffix("")
    if rel.name == "__init__.py":
        rel = rel.parent
    else:
        rel = rel.with_suffix("")
    return ".".join(rel.parts)


def _expand_module_chain(name: str) -> list[str]:
    parts = name.split(".")
    return [".".join(parts[:idx]) for idx in range(1, len(parts) + 1)]


def _find_project_root(start: Path) -> Path:
    for parent in [start] + list(start.parents):
        if (parent / "pyproject.toml").exists() or (parent / ".git").exists():
            return parent
    return start.parent


def _resolve_module_path(module_name: str, roots: list[Path]) -> Path | None:
    parts = module_name.split(".")
    rel = Path(*parts)
    for root in roots:
        mod_path = root / f"{rel}.py"
        if mod_path.exists():
            return mod_path
        pkg_path = root / rel / "__init__.py"
        if pkg_path.exists():
            return pkg_path
    return None


def _collect_imports(tree: ast.AST) -> list[str]:
    imports: list[str] = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                imports.append(alias.name)
        elif isinstance(node, ast.ImportFrom):
            if node.level:
                continue
            if node.module:
                imports.append(node.module)
                for alias in node.names:
                    if alias.name != "*":
                        imports.append(f"{node.module}.{alias.name}")
    return imports


def _module_dependencies(
    tree: ast.AST, module_name: str, module_graph: dict[str, Path]
) -> set[str]:
    deps: set[str] = set()
    for name in _collect_imports(tree):
        for candidate in _expand_module_chain(name):
            if candidate == "molt" and module_name.startswith("molt."):
                continue
            if candidate in module_graph and candidate != module_name:
                deps.add(candidate)
            if candidate.startswith("molt.stdlib."):
                stdlib_candidate = candidate[len("molt.stdlib.") :]
                if stdlib_candidate in module_graph and stdlib_candidate != module_name:
                    deps.add(stdlib_candidate)
    return deps


def _collect_func_defaults(tree: ast.AST) -> dict[str, dict[str, Any]]:
    defaults: dict[str, dict[str, Any]] = {}
    if not isinstance(tree, ast.Module):
        return defaults
    for stmt in tree.body:
        if not isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        if stmt.args.vararg or stmt.args.kwarg or stmt.args.kwonlyargs:
            continue
        params = [arg.arg for arg in stmt.args.args]
        default_specs: list[dict[str, Any]] = []
        for expr in stmt.args.defaults:
            if isinstance(expr, ast.Constant):
                default_specs.append({"const": True, "value": expr.value})
            else:
                default_specs.append({"const": False})
        defaults[stmt.name] = {"params": len(params), "defaults": default_specs}
    return defaults


def _topo_sort_modules(
    module_graph: dict[str, Path], module_deps: dict[str, set[str]]
) -> list[str]:
    in_degree = {name: 0 for name in module_graph}
    dependents: dict[str, set[str]] = {name: set() for name in module_graph}
    for name, deps in module_deps.items():
        for dep in deps:
            dependents[dep].add(name)
            in_degree[name] += 1
    ready = sorted(name for name, degree in in_degree.items() if degree == 0)
    order: list[str] = []
    while ready:
        name = ready.pop(0)
        order.append(name)
        for child in sorted(dependents[name]):
            in_degree[child] -= 1
            if in_degree[child] == 0:
                ready.append(child)
    if len(order) != len(module_graph):
        remaining = sorted(name for name in module_graph if name not in order)
        order.extend(remaining)
    return order


def _stdlib_allowlist() -> set[str]:
    allowlist: set[str] = set()
    spec_path = Path("docs/spec/0015_STDLIB_COMPATIBILITY_MATRIX.md")
    if not spec_path.exists():
        return allowlist
    in_table = False
    for line in spec_path.read_text().splitlines():
        if line.startswith("| Module |"):
            in_table = True
            continue
        if in_table and not line.startswith("|"):
            break
        if not in_table or line.startswith("| ---"):
            continue
        parts = [part.strip() for part in line.strip().strip("|").split("|")]
        if not parts:
            continue
        module_name = parts[0]
        if not module_name or module_name == "Module":
            continue
        for entry in module_name.split("/"):
            entry = entry.strip()
            if entry:
                allowlist.add(entry)
    return allowlist


def _discover_module_graph(
    entry_path: Path,
    roots: list[Path],
    module_roots: list[Path],
    skip_modules: set[str] | None = None,
    stub_parents: set[str] | None = None,
) -> tuple[dict[str, Path], set[str]]:
    stdlib_root = Path("src/molt/stdlib")
    graph: dict[str, Path] = {}
    skip_modules = skip_modules or set()
    stub_parents = stub_parents or set()
    explicit_imports: set[str] = set()
    queue = [entry_path]
    while queue:
        path = queue.pop()
        module_name = _module_name_from_path(path, module_roots, stdlib_root)
        if module_name in graph:
            continue
        graph[module_name] = path
        try:
            source = path.read_text()
        except OSError:
            continue
        try:
            tree = ast.parse(source)
        except SyntaxError:
            continue
        for name in _collect_imports(tree):
            explicit_imports.add(name)
            for candidate in _expand_module_chain(name):
                if candidate in stub_parents:
                    continue
                if candidate.split(".", 1)[0] in skip_modules:
                    continue
                resolved = None
                if candidate.startswith("molt.stdlib."):
                    stdlib_candidate = candidate[len("molt.stdlib.") :]
                    resolved = _resolve_module_path(stdlib_candidate, [stdlib_root])
                if resolved is None:
                    resolved = _resolve_module_path(candidate, roots)
                if resolved is not None:
                    queue.append(resolved)
    return graph, explicit_imports


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


def _detect_macos_deployment_target() -> str | None:
    env_target = os.environ.get("MOLT_MACOSX_DEPLOYMENT_TARGET")
    if env_target:
        return env_target
    env_target = os.environ.get("MACOSX_DEPLOYMENT_TARGET")
    if env_target:
        return env_target
    try:
        result = subprocess.run(
            ["xcrun", "--show-sdk-version"],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return None
    if result.returncode != 0:
        return None
    version = result.stdout.strip()
    return version or None


def build(
    file_path: str,
    target: Target = "native",
    parse_codec: ParseCodec = "msgpack",
    type_hint_policy: TypeHintPolicy = "ignore",
    fallback_policy: FallbackPolicy = "error",
    type_facts_path: str | None = None,
) -> int:
    source_path = Path(file_path)
    if not source_path.exists():
        print(f"File not found: {source_path}", file=sys.stderr)
        return 2

    stdlib_root = Path("src/molt/stdlib")
    try:
        entry_source = source_path.read_text()
    except OSError as exc:
        print(f"Failed to read entry module {source_path}: {exc}", file=sys.stderr)
        return 2
    try:
        entry_tree = ast.parse(entry_source)
    except SyntaxError as exc:
        print(f"Syntax error in {source_path}: {exc}", file=sys.stderr)
        return 2
    entry_imports = set(_collect_imports(entry_tree))
    stub_parents = STUB_PARENT_MODULES - entry_imports
    project_root = _find_project_root(source_path.resolve())
    cwd_root = _find_project_root(Path.cwd())
    module_roots: list[Path] = []
    for root in (project_root, cwd_root):
        src_root = root / "src"
        if src_root.exists():
            module_roots.append(src_root)
    module_roots.append(source_path.parent)
    module_roots = list(dict.fromkeys(root.resolve() for root in module_roots))
    roots = module_roots + [stdlib_root]
    module_graph, _explicit_imports = _discover_module_graph(
        source_path,
        roots,
        module_roots,
        skip_modules=STUB_MODULES,
        stub_parents=stub_parents,
    )
    entry_module = _module_name_from_path(source_path, module_roots, stdlib_root)
    for stub in stub_parents:
        if stub != entry_module:
            module_graph.pop(stub, None)
    known_modules = set(module_graph.keys())
    stdlib_allowlist = _stdlib_allowlist()
    stdlib_allowlist.update(STUB_MODULES)
    stdlib_allowlist.update(stub_parents)
    stdlib_allowlist.add("molt.stdlib")
    module_deps: dict[str, set[str]] = {}
    known_func_defaults: dict[str, dict[str, dict[str, Any]]] = {}
    for module_name, module_path in module_graph.items():
        try:
            source = module_path.read_text()
        except OSError as exc:
            print(f"Failed to read module {module_path}: {exc}", file=sys.stderr)
            return 2
        try:
            tree = ast.parse(source)
        except SyntaxError as exc:
            print(f"Syntax error in {module_path}: {exc}", file=sys.stderr)
            return 2
        module_deps[module_name] = _module_dependencies(tree, module_name, module_graph)
        known_func_defaults[module_name] = _collect_func_defaults(tree)
    module_order = _topo_sort_modules(module_graph, module_deps)
    type_facts = None
    if type_facts_path is None and type_hint_policy in {"trust", "check"}:
        type_facts, ty_ok = _collect_type_facts_for_build(
            list(module_graph.values()), type_hint_policy, source_path
        )
        if type_facts is None and type_hint_policy == "trust":
            print("Type facts unavailable; refusing trusted build.", file=sys.stderr)
            return 2
        if type_hint_policy == "trust" and not ty_ok:
            print("ty check failed; refusing trusted build.", file=sys.stderr)
            return 2
        if type_hint_policy == "check" and not ty_ok:
            print(
                "ty check failed; continuing with guarded hints only.",
                file=sys.stderr,
            )
    if type_facts_path is not None:
        facts_path = Path(type_facts_path)
        if not facts_path.exists():
            print(f"Type facts not found: {facts_path}", file=sys.stderr)
            return 2
        try:
            type_facts = load_type_facts(facts_path)
        except (OSError, json.JSONDecodeError, ValueError) as exc:
            print(f"Failed to load type facts: {exc}", file=sys.stderr)
            return 2

    functions: list[dict[str, Any]] = []
    enable_phi = target != "wasm"
    known_classes: dict[str, Any] = {}
    for module_name in module_order:
        module_path = module_graph[module_name]
        try:
            source = module_path.read_text()
        except OSError as exc:
            print(f"Failed to read module {module_path}: {exc}", file=sys.stderr)
            return 2
        try:
            tree = ast.parse(source)
        except SyntaxError as exc:
            print(f"Syntax error in {module_path}: {exc}", file=sys.stderr)
            return 2
        gen = SimpleTIRGenerator(
            parse_codec=parse_codec,
            type_hint_policy=type_hint_policy,
            fallback_policy=fallback_policy,
            source_path=str(module_path),
            type_facts=type_facts,
            module_name=module_name,
            entry_module=entry_module,
            enable_phi=enable_phi,
            known_modules=known_modules,
            known_classes=known_classes,
            stdlib_allowlist=stdlib_allowlist,
            known_func_defaults=known_func_defaults,
        )
        try:
            gen.visit(tree)
        except CompatibilityError as exc:
            print(exc, file=sys.stderr)
            return 2
        ir = gen.to_json()
        init_symbol = SimpleTIRGenerator.module_init_symbol(module_name)
        for func in ir["functions"]:
            if func["name"] == "molt_main":
                func["name"] = init_symbol
        functions.extend(ir["functions"])
        for class_name in gen.local_class_names:
            known_classes[class_name] = gen.classes[class_name]

    entry_init = SimpleTIRGenerator.module_init_symbol(entry_module)
    functions.append(
        {
            "name": "molt_main",
            "params": [],
            "ops": [
                {"kind": "call", "s_value": entry_init, "args": [], "out": "v0"},
                {"kind": "ret_void"},
            ],
        }
    )
    ir = {"functions": functions}

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
#include <string.h>
extern void molt_main();
extern int molt_json_parse_scalar(const char* ptr, long len, unsigned long long* out);
extern int molt_msgpack_parse_scalar(const char* ptr, long len, unsigned long long* out);
extern int molt_cbor_parse_scalar(const char* ptr, long len, unsigned long long* out);
extern long molt_get_attr_generic(void* obj, const char* attr, long len);
extern unsigned long long molt_alloc(long size);
extern long molt_block_on(void* task);
extern long molt_async_sleep(void* obj);
extern void molt_spawn(void* task);
extern void* molt_chan_new(unsigned long long capacity);
extern long molt_chan_send(void* chan, long val);
extern long molt_chan_recv(void* chan);
extern void molt_print_obj(unsigned long long val);
extern void molt_profile_dump();
int main() {
    molt_main();
    const char* profile = getenv("MOLT_PROFILE");
    if (profile != NULL && profile[0] != '\\0' && strcmp(profile, "0") != 0) {
        molt_profile_dump();
    }
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
        deployment_target = _detect_macos_deployment_target()
        if deployment_target:
            link_cmd.append(f"-mmacosx-version-min={deployment_target}")
    link_cmd.extend(["main_stub.c", "output.o", str(runtime_lib), "-o", output_binary])
    if sys.platform == "darwin":
        link_cmd.append("-lc++")
    elif sys.platform.startswith("linux"):
        link_cmd.append("-lstdc++")
        link_cmd.append("-lm")

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
    build_parser.add_argument(
        "--fallback",
        choices=["error", "bridge"],
        default="error",
        help="Fallback policy for unsupported constructs.",
    )
    build_parser.add_argument(
        "--type-facts",
        help="Path to type facts JSON from `molt check`.",
    )

    check_parser = subparsers.add_parser(
        "check", help="Generate a type facts artifact (ty-backed when available)"
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
        return build(
            args.file,
            args.target,
            args.codec,
            args.type_hints,
            args.fallback,
            args.type_facts,
        )
    if args.command == "check":
        return check(args.path, args.output, args.strict)
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


def _collect_py_files(target: Path) -> list[Path]:
    if target.is_file():
        return [target]
    return sorted(path for path in target.rglob("*.py") if path.is_file())


def _run_ty_check(path: Path) -> tuple[bool, str]:
    commands = [
        ["uv", "run", "ty", "check", str(path), "--output-format", "concise"],
        ["ty", "check", str(path), "--output-format", "concise"],
    ]
    for cmd in commands:
        try:
            result = subprocess.run(cmd, capture_output=True, text=True, check=False)
        except FileNotFoundError:
            continue
        if result.returncode == 0:
            return True, result.stdout.strip()
        combined = (result.stdout + result.stderr).strip()
        return False, combined
    return False, "ty is not available; install it with `uv add ty`."


def _collect_type_facts_for_build(
    paths: list[Path], type_hint_policy: TypeHintPolicy, ty_target: Path
) -> tuple[Any | None, bool]:
    trust = "trusted" if type_hint_policy == "trust" else "guarded"
    ty_ok, _ = _run_ty_check(ty_target)
    facts = collect_type_facts_from_paths(paths, trust, infer=ty_ok)
    if ty_ok:
        facts.tool = "molt-check+ty+infer"
    return facts, ty_ok


def check(path: str, output: str, strict: bool) -> int:
    target = Path(path)
    if not target.exists():
        print(f"Path not found: {target}", file=sys.stderr)
        return 2
    files = _collect_py_files(target)
    if not files:
        print(f"No Python files found under: {target}", file=sys.stderr)
        return 2
    trust = "trusted" if strict else "guarded"
    ty_ok, ty_output = _run_ty_check(target)
    if ty_ok:
        facts = collect_type_facts_from_paths(files, trust, infer=True)
        facts.tool = "molt-check+ty+infer"
    elif ty_output:
        print(ty_output, file=sys.stderr)
        if strict:
            print("ty check failed; refusing strict type facts.", file=sys.stderr)
            return 2
        facts = collect_type_facts_from_paths(files, trust, infer=False)
    else:
        facts = collect_type_facts_from_paths(files, trust, infer=False)
    output_path = Path(output)
    write_type_facts(output_path, facts)
    print(f"Wrote type facts to {output_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
