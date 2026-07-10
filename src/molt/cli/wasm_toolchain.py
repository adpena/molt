from __future__ import annotations

from dataclasses import dataclass
import functools
import os
import shutil
from pathlib import Path
import tomllib

from molt.cli.command_runtime import _run_completed_command


_WASI_TARGET_INCLUDE_DIRS = ("wasm32-wasip1", "wasm32-wasi")
_REQUIRED_WASM_RUST_TARGETS = ("wasm32-wasip1",)


class RustToolchainContractError(ValueError):
    pass


@dataclass(frozen=True)
class RustToolchainContract:
    channel: str | None
    components: tuple[str, ...]
    targets: tuple[str, ...]

    @property
    def rustup_toolchain_args(self) -> tuple[str, ...]:
        return () if self.channel is None else ("--toolchain", self.channel)

    @property
    def required_wasm_targets(self) -> tuple[str, ...]:
        targets: list[str] = []
        for target in (*_REQUIRED_WASM_RUST_TARGETS, *self.targets):
            if target.startswith("wasm32") and target not in targets:
                targets.append(target)
        return tuple(targets)


@functools.lru_cache(maxsize=32)
def rust_toolchain_contract(root: Path | str | None = None) -> RustToolchainContract:
    root_path = Path(root).resolve(strict=False) if root is not None else None
    toolchain_path = (
        root_path / "rust-toolchain.toml" if root_path is not None else None
    )
    if toolchain_path is None or not toolchain_path.exists():
        return RustToolchainContract(channel=None, components=(), targets=())
    try:
        data = tomllib.loads(toolchain_path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as exc:
        raise RustToolchainContractError(
            f"invalid Rust toolchain contract {toolchain_path}: {exc}"
        ) from exc
    toolchain = data.get("toolchain", {})
    if not isinstance(toolchain, dict):
        toolchain = {}
    channel_raw = toolchain.get("channel")
    channel = channel_raw.strip() if isinstance(channel_raw, str) else None
    if not channel:
        channel = None

    def string_tuple(key: str) -> tuple[str, ...]:
        value = toolchain.get(key, ())
        if not isinstance(value, list):
            return ()
        return tuple(
            item.strip() for item in value if isinstance(item, str) and item.strip()
        )

    return RustToolchainContract(
        channel=channel,
        components=string_tuple("components"),
        targets=string_tuple("targets"),
    )


def rustup_toolchain_install_cmd(root: Path) -> list[str]:
    contract = rust_toolchain_contract(root)
    cmd = ["rustup", "toolchain", "install"]
    if contract.channel is not None:
        cmd.append(contract.channel)
    else:
        cmd.append("stable")
    cmd.extend(["--profile", "minimal"])
    for component in contract.components:
        cmd.extend(["--component", component])
    for target in contract.required_wasm_targets:
        cmd.extend(["--target", target])
    return cmd


def rustup_target_add_cmd(target_triple: str, root: Path | None = None) -> list[str]:
    contract = rust_toolchain_contract(root)
    return [
        "rustup",
        "target",
        "add",
        target_triple,
        *contract.rustup_toolchain_args,
    ]


def rustup_installed_targets(root: Path | None = None) -> tuple[str, ...] | None:
    rustup = shutil.which("rustup")
    if rustup is None:
        return None
    contract = rust_toolchain_contract(root)
    try:
        result = _run_completed_command(
            [
                rustup,
                "target",
                "list",
                "--installed",
                *contract.rustup_toolchain_args,
            ],
            capture_output=True,
            env=None,
            cwd=root,
            memory_guard_prefix="MOLT_BUILD",
        )
    except OSError:
        return None
    if result.returncode != 0:
        return None
    return tuple(result.stdout.split())


def _rustlib_target_dir_installed(target_triple: str, root: Path | None) -> bool:
    """Lock-free ground truth for an installed rustup target.

    `rustup target list` contends on the rustup lock and has returned empty
    output under concurrent cargo/rustup lanes, producing false "target
    missing" build failures for targets that are installed. The installed
    standard library lives at
    ``$RUSTUP_HOME/toolchains/<channel>-*/lib/rustlib/<triple>`` — a plain
    directory probe that no lock can lie about.
    """
    rustup_home = os.environ.get("RUSTUP_HOME", "").strip()
    home = Path(rustup_home).expanduser() if rustup_home else Path.home() / ".rustup"
    toolchains = home / "toolchains"
    if not toolchains.is_dir():
        return False
    contract = rust_toolchain_contract(root)
    pattern = f"{contract.channel}-*" if contract.channel else "*"
    for toolchain_dir in toolchains.glob(pattern):
        if (toolchain_dir / "lib" / "rustlib" / target_triple).is_dir():
            return True
    return False


def ensure_rustup_target(
    target_triple: str, warnings: list[str], *, root: Path | None = None
) -> bool:
    rustup_path = shutil.which("rustup")
    if not rustup_path:
        warnings.append(f"rustup not found; cannot ensure target {target_triple}")
        return False
    # Filesystem ground truth first: the rustup CLI query contends on the
    # rustup lock under concurrent lanes and has returned empty output for
    # installed targets, failing witness builds with a false "target
    # missing". The rustlib directory probe cannot be starved by a lock.
    if _rustlib_target_dir_installed(target_triple, root):
        return True
    try:
        installed = rustup_installed_targets(root)
    except RustToolchainContractError as exc:
        warnings.append(str(exc))
        return False
    if installed is None:
        warnings.append(f"Failed to query rustup targets for {target_triple}")
        return False
    if target_triple in installed:
        return True
    add_command = rustup_target_add_cmd(target_triple, root)
    add_command[0] = rustup_path
    try:
        add = _run_completed_command(
            add_command,
            capture_output=True,
            env=None,
            cwd=root,
            memory_guard_prefix="MOLT_BUILD",
        )
    except OSError as exc:
        warnings.append(f"Failed to install rustup target {target_triple}: {exc}")
        return False
    if add.returncode != 0:
        detail = (add.stderr or add.stdout).strip() or "unknown error"
        warnings.append(f"rustup target add failed for {target_triple}: {detail}")
        return False
    rust_target_libdir.cache_clear()
    return True


def rust_target_missing_message(
    target_triple: str, *, root: Path | None = None, context: str = "WASM build"
) -> str:
    try:
        cmd = rustup_target_add_cmd(target_triple, root)
    except RustToolchainContractError as exc:
        return f"{context} cannot resolve Rust target setup: {exc}"
    return (
        f"{context} requires Rust target {target_triple}, but the active Rust "
        f"toolchain does not provide it. Run: {' '.join(cmd)}"
    )


def _normalize_target_include_path(candidate: Path) -> Path | None:
    if candidate.name not in _WASI_TARGET_INCLUDE_DIRS:
        return None
    if candidate.parent.name != "include":
        return None
    if not (candidate / "errno.h").exists():
        return None
    return candidate.parent.parent.resolve(strict=False)


def wasi_libcxx_include_dir(
    sysroot: str | Path | None,
    *,
    target_triple: str | None = None,
    exceptions: bool = True,
) -> Path | None:
    """Resolve the C++ standard library (libc++) include dir inside a sysroot.

    WASI SDK sysroots that ship multiple ABI variants (the ``+m``/multilib
    layout) place libc++ headers under a per-target, per-exception-mode subtree
    ``include/<target>/{eh,noeh}/c++/v1`` and leave the flat ``include/c++/v1``
    empty, so ``clang++ --target wasm32-wasip1`` does NOT auto-discover
    ``<atomic>``/``<vector>`` etc. Return the variant that matches how molt
    compiles wasm C++ (``-mexception-handling`` on -> the ``eh`` subtree). Fall
    back to the flat ``include/c++/v1`` for single-variant sysroots. Returns
    ``None`` when no populated libc++ tree exists.
    """
    if sysroot is None:
        return None
    root = Path(sysroot).expanduser()
    inc = root / "include"
    eh_order = ("eh", "noeh") if exceptions else ("noeh", "eh")
    targets: list[str] = []
    if target_triple:
        targets.append(target_triple)
    targets.extend(t for t in _WASI_TARGET_INCLUDE_DIRS if t not in targets)
    for target in targets:
        for eh in eh_order:
            cand = inc / target / eh / "c++" / "v1"
            if (cand / "atomic").exists():
                return cand.resolve(strict=False)
    flat = inc / "c++" / "v1"
    if (flat / "atomic").exists():
        return flat.resolve(strict=False)
    return None


def normalize_wasi_sysroot(path: str | Path | None) -> Path | None:
    if path is None:
        return None
    candidate = Path(path).expanduser()
    target_include_root = _normalize_target_include_path(candidate)
    if target_include_root is not None:
        return target_include_root
    roots = [candidate]
    if candidate.name == "include":
        roots.append(candidate.parent)
    for root in roots:
        for target in _WASI_TARGET_INCLUDE_DIRS:
            if (root / "include" / target / "errno.h").exists():
                return root.resolve(strict=False)
        if (root / "include" / "errno.h").exists():
            return root.resolve(strict=False)
    return None


def _wasi_sdk_sysroot_candidates(raw: str | None) -> list[Path]:
    if not raw:
        return []
    sdk_root = Path(raw).expanduser()
    return [
        sdk_root,
        sdk_root / "share" / "wasi-sysroot",
        sdk_root / "wasi-sysroot",
    ]


@functools.lru_cache(maxsize=64)
def _resolve_wasi_sysroot_cached(
    molt_wasi_sysroot: str | None,
    wasi_sysroot: str | None,
    wasi_sdk_path: str | None,
    wasi_sdk_prefix: str | None,
    molt_target_root: str | None,
) -> Path | None:
    candidates: list[Path] = []
    for raw in (molt_wasi_sysroot, wasi_sysroot):
        if raw:
            candidates.append(Path(raw).expanduser())
    candidates.extend(_wasi_sdk_sysroot_candidates(wasi_sdk_path))
    candidates.extend(_wasi_sdk_sysroot_candidates(wasi_sdk_prefix))
    if molt_target_root:
        target_root = Path(molt_target_root).expanduser()
        target_toolchains = target_root / "toolchains"
        candidates.extend(
            [
                target_root / "toolchains" / "wasi-sysroot",
                target_root / "toolchains" / "wasi-sdk" / "share" / "wasi-sysroot",
                target_root / "toolchains" / "wasi-sdk" / "wasi-sysroot",
                target_root / "wasi-sysroot",
                target_root / "wasi-sdk" / "share" / "wasi-sysroot",
                target_root / "wasi-sdk" / "wasi-sysroot",
            ]
        )
        if target_toolchains.exists():
            candidates.extend(sorted(target_toolchains.glob("wasi-sysroot-*")))
    if os.name == "nt":
        program_files = os.environ.get("ProgramFiles")
        local_app_data = os.environ.get("LOCALAPPDATA")
        for root in (program_files, local_app_data):
            if root:
                candidates.extend(
                    _wasi_sdk_sysroot_candidates(str(Path(root) / "wasi-sdk"))
                )
    else:
        candidates.extend(
            [
                Path("/opt/homebrew/opt/wasi-libc/share/wasi-sysroot"),
                Path("/usr/local/opt/wasi-libc/share/wasi-sysroot"),
                Path("/opt/wasi-sdk/share/wasi-sysroot"),
                Path("/opt/wasi-sdk/wasi-sysroot"),
                Path("/usr/share/wasi-sysroot"),
                Path("/usr/include/wasm32-wasi"),
                Path("/usr/local/share/wasi-sysroot"),
                Path("/usr/local/include/wasm32-wasi"),
            ]
        )
    seen: set[Path] = set()
    for candidate in candidates:
        normalized = candidate.resolve(strict=False)
        if normalized in seen:
            continue
        seen.add(normalized)
        resolved = normalize_wasi_sysroot(normalized)
        if resolved is not None:
            return resolved
    return None


def resolve_wasi_sysroot() -> Path | None:
    return _resolve_wasi_sysroot_cached(
        os.environ.get("MOLT_WASI_SYSROOT"),
        os.environ.get("WASI_SYSROOT"),
        os.environ.get("WASI_SDK_PATH"),
        os.environ.get("WASI_SDK_PREFIX"),
        os.environ.get("MOLT_TARGET_ROOT"),
    )


@functools.lru_cache(maxsize=8)
def rust_target_libdir(target_triple: str) -> Path | None:
    rustc = shutil.which("rustc")
    if rustc is None:
        return None
    try:
        result = _run_completed_command(
            [rustc, "--print", "target-libdir", "--target", target_triple],
            capture_output=True,
            timeout=30,
            env=None,
            cwd=None,
            memory_guard_prefix="MOLT_BUILD",
        )
    except OSError:
        return None
    if result.returncode != 0:
        return None
    path_text = result.stdout.strip()
    if not path_text:
        return None
    return Path(path_text)


def wasm_wasi_libc_archive(target_triple: str = "wasm32-wasip1") -> Path | None:
    target_libdir = rust_target_libdir(target_triple)
    if target_libdir is None:
        return None
    libc_archive = target_libdir / "self-contained" / "libc.a"
    if not libc_archive.exists():
        return None
    return libc_archive


def wasm_compiler_builtins_archive(target_triple: str = "wasm32-wasip1") -> Path | None:
    target_libdir = rust_target_libdir(target_triple)
    if target_libdir is None:
        return None
    candidates = sorted(target_libdir.glob("libcompiler_builtins-*.rlib"))
    if candidates:
        return candidates[0]
    unversioned = target_libdir / "libcompiler_builtins.rlib"
    if unversioned.exists():
        return unversioned
    return None


# WASI sysroots use the wasm32-wasip1 (and legacy wasm32-wasi) multilib layout.
_WASI_SYSROOT_LIB_SUBDIRS = ("wasm32-wasip1", "wasm32-wasi")


def _wasi_sysroot_lib_archive(name: str) -> Path | None:
    """Resolve a named archive under the active WASI sysroot's lib dir.

    Probes the ABI-variant lib subdirs (``lib/wasm32-wasip1`` then the legacy
    ``lib/wasm32-wasi``) of :func:`resolve_wasi_sysroot`. Returns ``None`` when
    no sysroot resolves or the archive is absent from every candidate dir.
    """
    sysroot = resolve_wasi_sysroot()
    if sysroot is None:
        return None
    for subdir in _WASI_SYSROOT_LIB_SUBDIRS:
        candidate = sysroot / "lib" / subdir / name
        if candidate.exists():
            return candidate.resolve(strict=False)
    return None


def wasm_wasi_printscan_long_double_archive() -> Path | None:
    """wasi-libc's long-double-capable printf/scanf archive.

    The default ``libc.a`` links a ``long_double_not_supported`` stub for the
    ``%L`` float conversions that ``abort()``s (raw ``unreachable`` trap) — the
    E1 witness frontier where numpy's longdouble repr/parse hit it during
    ``_multiarray_umath`` import. wasi-libc ships the real formatters in this
    companion archive; whole-archiving it ahead of ``libc.a`` overrides the
    stub. Its binary128 arithmetic needs the TF-mode soft-float builtins from
    :func:`wasm_clang_rt_builtins_archive`.
    """
    return _wasi_sysroot_lib_archive("libc-printscan-long-double.a")


def wasm_clang_rt_builtins_archive() -> Path | None:
    """LLVM compiler-rt builtins (incl. binary128 ``__addtf3``/``__multf3`` …).

    Rust's ``wasm32-wasip1`` sysroot ships only ``libc.a`` + a
    ``compiler_builtins`` rlib that *references* the TF-mode soft-float
    routines as undefined; the concrete definitions live in wasi-sdk's
    ``libclang_rt.builtins-wasm32.a``. Required so wasi-libc's long-double
    printf/scanf (and numpy's own longdouble arithmetic) resolve at link time
    instead of degrading to unresolved imports.
    """
    return _wasi_sysroot_lib_archive("libclang_rt.builtins-wasm32.a")
