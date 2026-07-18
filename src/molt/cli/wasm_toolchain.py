from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass
import functools
import os
import re
import shutil
from pathlib import Path
import tomllib

from molt.cli.command_runtime import _run_completed_command
from molt.wasi_sysroot import (
    WASI_TARGET_INCLUDE_DIRS as _WASI_TARGET_INCLUDE_DIRS,
    normalize_wasi_sysroot,
    wasi_sysroot_llvm_version,
)


_REQUIRED_WASM_RUST_TARGETS = ("wasm32-wasip1",)


class RustToolchainContractError(ValueError):
    pass


class WasmLinkerContractError(ValueError):
    pass


@dataclass(frozen=True)
class WasmLinkerIdentity:
    path: Path
    version: str
    wasi_sdk_llvm_version: str | None

    @property
    def diagnostic(self) -> str:
        expected = self.wasi_sdk_llvm_version or "unattested"
        return f"wasm-ld={self.path} version={self.version} wasi-sdk-llvm={expected}"


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


def _wasi_sdk_root_for_sysroot(sysroot: Path) -> Path | None:
    if sysroot.name == "wasi-sysroot" and sysroot.parent.name == "share":
        return sysroot.parent.parent
    if sysroot.name == "wasi-sysroot":
        return sysroot.parent
    return None


def _wasm_linker_version(path: Path) -> str:
    result = _run_completed_command(
        [str(path), "--version"],
        capture_output=True,
        env=None,
        cwd=None,
        memory_guard_prefix="MOLT_BUILD",
    )
    output = f"{result.stdout}\n{result.stderr}"
    match = re.search(r"\b(?:LLD\s+)?(\d+\.\d+(?:\.\d+)?)\b", output)
    if result.returncode != 0 or match is None:
        detail = output.strip() or f"exit code {result.returncode}"
        raise WasmLinkerContractError(
            f"unable to attest wasm-ld identity for {path}: {detail}"
        )
    return match.group(1)


def _llvm_release_line(version: str) -> tuple[int, int]:
    major, minor, *_ = version.split(".")
    return int(major), int(minor)


def resolve_wasm_linker() -> WasmLinkerIdentity | None:
    sysroot = resolve_wasi_sysroot()
    candidates: list[Path] = []
    override = os.environ.get("MOLT_WASM_LD", "").strip()
    if override:
        candidates.append(Path(override).expanduser())
    if sysroot is not None:
        sdk_root = _wasi_sdk_root_for_sysroot(sysroot)
        if sdk_root is not None:
            candidates.append(
                sdk_root / "bin" / ("wasm-ld.exe" if os.name == "nt" else "wasm-ld")
            )
    on_path = shutil.which("wasm-ld")
    if on_path:
        candidates.append(Path(on_path))
    linker = next(
        (path.resolve(strict=False) for path in candidates if path.is_file()), None
    )
    if linker is None:
        return None
    version = _wasm_linker_version(linker)
    expected = None
    if sysroot is not None:
        expected = wasi_sysroot_llvm_version(sysroot)
        if expected is not None and _llvm_release_line(version) != _llvm_release_line(
            expected
        ):
            raise WasmLinkerContractError(
                "wasm linker/toolchain mismatch: "
                f"{linker} reports {version}, but {sysroot / 'VERSION'} requires "
                f"LLVM {expected}; use the matching wasi-sdk bin/wasm-ld or set "
                "MOLT_WASM_LD"
            )
    return WasmLinkerIdentity(linker, version, expected)


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


def wasm_cxx_runtime_archives(
    target_triple: str = "wasm32-wasip1",
    *,
    exceptions: bool = True,
) -> tuple[Path, ...] | None:
    sysroot = resolve_wasi_sysroot()
    if sysroot is None:
        return None
    exception_mode = "eh" if exceptions else "noeh"
    target_names = [target_triple]
    if target_triple == "wasm32-wasip1":
        target_names.append("wasm32-wasi")
    for target_name in target_names:
        library_root = sysroot / "lib" / target_name / exception_mode
        libcxx = library_root / "libc++.a"
        libcxxabi = library_root / "libc++abi.a"
        archives = [libcxx, libcxxabi]
        if exceptions:
            archives.append(library_root / "libunwind.a")
        if all(archive.is_file() for archive in archives):
            return tuple(archive.resolve(strict=False) for archive in archives)
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

    Resolves from the active WASI sysroot's ``lib/wasm32-wasip1`` (preferred) or
    legacy ``lib/wasm32-wasi`` multilib, falling back to the durable committed
    ``vendor/wasm-builtins`` copy so a fresh/incomplete session sysroot cannot
    silently drop it (which masked the E1 witness long-double regression).
    """
    return _wasi_sysroot_lib_archive(
        "libc-printscan-long-double.a"
    ) or _vendored_wasm_lib_archive("libc-printscan-long-double.a")


def _wasi_sdk_compiler_rt_builtins_archive() -> Path | None:
    """``libclang_rt.builtins-wasm32.a`` from a full wasi-sdk's clang resource dir.

    In a complete wasi-sdk install the compiler-rt builtins live under
    ``<wasi-sdk>/lib/clang/<ver>/lib/{wasip1,wasi}/`` rather than inside the
    wasi-sysroot's ``lib`` multilib. When the active sysroot resolves to
    ``<wasi-sdk>/share/wasi-sysroot`` (or ``<wasi-sdk>/wasi-sysroot``) probe the
    sibling resource dir so a genuine wasi-sdk resolves the archive without the
    vendored fallback. Returns ``None`` when no such tree exists.
    """
    sysroot = resolve_wasi_sysroot()
    if sysroot is None:
        return None
    sdk_roots: list[Path] = [sysroot.parent]
    if sysroot.parent.name == "share":
        sdk_roots.append(sysroot.parent.parent)
    for sdk_root in sdk_roots:
        clang_lib = sdk_root / "lib" / "clang"
        if not clang_lib.is_dir():
            continue
        for subdir in ("wasip1", "wasi"):
            matches = sorted(
                clang_lib.glob(f"*/lib/{subdir}/libclang_rt.builtins-wasm32.a")
            )
            if matches:
                return matches[-1].resolve(strict=False)
    return None


def wasm_builtins_vendor_dir() -> Path:
    """Repo-vendored wasm long-double link archives (durable build inputs).

    A committed home for the wasm reloc-runtime long-double link inputs, resolved
    relative to this module so an editable checkout finds it without env hunting.
    """
    return Path(__file__).resolve().parents[3] / "vendor" / "wasm-builtins"


def _vendored_wasm_lib_archive(name: str) -> Path | None:
    """Resolve a named archive from the committed ``vendor/wasm-builtins`` copy.

    The provisioned toolchain is only the wasi-sysroot *subset*: a fresh / wiped
    / CI / other-machine session target dir can miss the long-double formatter
    (``libc-printscan-long-double.a``) and always misses compiler-rt
    (``libclang_rt.builtins-wasm32.a``, which lives in wasi-sdk's resource dir,
    not the sysroot). Both were otherwise placed by hand and raced provisioning,
    so the reloc link degraded and relinked the long-double ``unreachable`` stub.
    Byte-identical copies are committed under :func:`wasm_builtins_vendor_dir`
    (pinned to wasi-sdk-33 / LLVM 22.1.0; see its README) so the archives resolve
    with zero provisioning on every machine/session/CI.
    """
    candidate = wasm_builtins_vendor_dir() / name
    if candidate.exists():
        return candidate.resolve(strict=False)
    return None


def wasm_clang_rt_builtins_archive() -> Path | None:
    """LLVM compiler-rt builtins (incl. binary128 ``__addtf3``/``__multf3`` …).

    Rust's ``wasm32-wasip1`` sysroot ships only ``libc.a`` + a
    ``compiler_builtins`` rlib that *references* the TF-mode soft-float
    routines as undefined; the concrete definitions live in wasi-sdk's
    ``libclang_rt.builtins-wasm32.a``. Required so wasi-libc's long-double
    printf/scanf (and numpy's own longdouble arithmetic) resolve at link time
    instead of degrading to unresolved imports.

    Resolution order (first hit wins), from most- to least-specific to the
    active toolchain, ending in the durable committed vendored copy so the
    archive is present-by-construction on every machine/session/CI:

    1. the active WASI sysroot's ``lib/wasm32-wasip1`` (or legacy) multilib,
    2. a full wasi-sdk's clang compiler-rt resource dir, and
    3. the repo-vendored ``vendor/wasm-builtins`` copy.
    """
    return (
        _wasi_sysroot_lib_archive("libclang_rt.builtins-wasm32.a")
        or _wasi_sdk_compiler_rt_builtins_archive()
        or _vendored_wasm_lib_archive("libclang_rt.builtins-wasm32.a")
    )


# --- Single authority: wasi-libc long-double (%L) link policy ----------------
#
# ONE resolver + ordering policy that EVERY molt wasm link path consults so that
# no wasm module can link wasi-libc's ``libc.a`` without overriding its
# ``long_double_not_supported`` abort stub (raw ``unreachable`` trap at numpy
# ``_multiarray_umath`` import). Three link paths apply it, each via the
# mechanism appropriate to how it drives the linker:
#   * reloc runtime  — molt-driven ``wasm-ld -r`` (whole-archives the staticlib);
#   * split app.wasm — molt-driven ``wasm-ld``  (numpy + libc.a, no reloc rt);
#   * deploy cdylib  — rustc-driven link: the resolved archives are threaded to
#     molt-runtime's ``build.rs`` via env (``MOLT_WASM_LONGDOUBLE_ARCHIVE`` /
#     ``MOLT_WASM_BUILTINS_ARCHIVE``), which links them as build-script
#     ``rustc-link-lib`` entries — emitted AHEAD of the self-contained ``-lc``.
# The ``artifact_poison_gate`` attests the effect (stub string ABSENT) uniformly
# across all three built artifacts.

_LONG_DOUBLE_LINK_ARCHIVES: tuple[tuple[str, str], ...] = (
    ("libc-printscan-long-double.a", "MOLT_WASM_LONGDOUBLE_ARCHIVE"),
    ("libclang_rt.builtins-wasm32.a", "MOLT_WASM_BUILTINS_ARCHIVE"),
)


@dataclass(frozen=True)
class LongDoubleLinkPolicy:
    """Resolved wasi-libc long-double (%L) link inputs + fail-loud decision.

    ``printscan`` (``libc-printscan-long-double.a``) carries the real
    ``vfprintf``/``__floatscan``/``strtold`` that override ``libc.a``'s
    ``long_double_not_supported`` stub *when linked ahead of ``libc.a```;
    ``builtins`` (``libclang_rt.builtins-wasm32.a``) supplies the binary128
    soft-float (``__addtf3``/``__multf3``/…) the real formatters call.
    ``error`` is set (build MUST abort) when ``required`` and an archive is
    unresolvable — a runtime that relinks the abort stub is never acceptable.
    """

    printscan: Path | None
    builtins: Path | None
    error: str | None
    warnings: tuple[str, ...]


def long_double_archives_missing_message(missing: Sequence[str]) -> str:
    """Actionable hard-error diagnostic naming the missing archive(s) + the fix."""
    names = ", ".join(missing)
    return (
        "wasm long-double (%L) link (CPython-ABI/numpy tier) requires the "
        "wasi-libc long-double formatter archives, but these are not resolvable: "
        f"{names}. This module links numpy/scipy long double formatting; "
        "proceeding would relink wasi-libc's long_double_not_supported stub and "
        "abort() (raw `unreachable` trap) at _multiarray_umath import. Refusing "
        "to build a module that traps (no silent degrade). Provision the archives "
        "(both ship pinned in-repo at vendor/wasm-builtins/, resolved by "
        "molt.cli.wasm_toolchain automatically): libc-printscan-long-double.a "
        "ships in the wasi-sysroot-33.0+m tarball (lib/wasm32-wasip1/) — set "
        "MOLT_WASI_SYSROOT / MOLT_TARGET_ROOT to a complete sysroot; "
        "libclang_rt.builtins-wasm32.a is wasi-sdk-33 compiler-rt "
        "(lib/clang/*/lib/wasip1/). If they resolve None the committed "
        "vendor/wasm-builtins copy is missing — restore it (see its README)."
    )


def resolve_long_double_link_policy(*, required: bool) -> LongDoubleLinkPolicy:
    """Resolve the long-double link archives and decide fail-loud vs degrade.

    ``required`` (numpy/scipy or CPython-ABI tier): a missing archive returns an
    ``error`` the caller MUST honour (abort the build). Otherwise returns the
    archives plus any degrade ``warnings`` for a module that provably never hits
    ``%L`` (micro / no-numpy).
    """
    printscan = wasm_wasi_printscan_long_double_archive()
    builtins = wasm_clang_rt_builtins_archive()
    resolved = {
        "libc-printscan-long-double.a": printscan,
        "libclang_rt.builtins-wasm32.a": builtins,
    }
    missing = [name for name, _ in _LONG_DOUBLE_LINK_ARCHIVES if resolved[name] is None]
    if required and missing:
        return LongDoubleLinkPolicy(
            printscan, builtins, long_double_archives_missing_message(missing), ()
        )
    warnings: list[str] = []
    if printscan is None:
        warnings.append(
            "wasm long-double link warning: wasi-libc "
            "libc-printscan-long-double.a not found in the active WASI sysroot or "
            "vendor/wasm-builtins; long double %L formatting will abort() "
            "(unreachable) at runtime."
        )
    elif builtins is None:
        warnings.append(
            "wasm long-double link warning: long-double printf/scanf archive "
            "present but libclang_rt.builtins-wasm32.a is not resolvable (sysroot "
            "/ wasi-sdk resource dir / vendor/wasm-builtins) — binary128 "
            "soft-float (__addtf3/__multf3/…) will not resolve. Provision wasi-sdk "
            "compiler-rt builtins."
        )
    return LongDoubleLinkPolicy(printscan, builtins, None, tuple(warnings))


def long_double_whole_archive_link_argv(
    policy: LongDoubleLinkPolicy,
    *,
    whole_archive: Sequence[str],
    trailing: Sequence[str],
) -> list[str]:
    """The shared ``wasm-ld`` argv fragment applying the long-double policy.

    Emits ``--whole-archive <whole_archive...> [printscan] --no-whole-archive
    <trailing...> [builtins]`` so ``printscan``'s real formatters are force-
    loaded ahead of ``libc.a`` (which stays in the lazy ``trailing`` group and is
    skipped once the symbols are defined). ``builtins`` is appended lazily and
    de-duplicated. When ``printscan`` is unresolved the fragment degrades to the
    plain whole/no-whole split (callers gate the numpy tier on ``policy.error``).
    """
    wa = [str(entry) for entry in whole_archive]
    tr = [str(entry) for entry in trailing]
    if policy.printscan is not None:
        wa.append(str(policy.printscan.resolve(strict=False)))
        if policy.builtins is not None:
            builtins = str(policy.builtins.resolve(strict=False))
            if builtins not in tr:
                tr.append(builtins)
    return ["--whole-archive", *wa, "--no-whole-archive", *tr]
