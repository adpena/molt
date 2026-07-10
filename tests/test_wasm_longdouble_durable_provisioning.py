"""Durability + fail-loud gate for the wasm reloc-runtime long-double archives.

Companion to ``test_wasm_longdouble_printf_link.py`` (which proves the *link*
overrides the stub end-to-end). This module is hermetic — it needs no clang /
wasm-ld / nm — and locks in the two structural repairs that stopped a
graceful-degrade from silently reintroducing the long-double ``unreachable``
trap (witness RUN 20260710T164604):

Part A (durable provisioning): both link archives
(``libc-printscan-long-double.a`` + ``libclang_rt.builtins-wasm32.a``) resolve on
a machine with NO usable WASI sysroot, from the committed ``vendor/wasm-builtins``
copy, so a fresh/wiped/CI/other-machine session cannot silently miss them.

Part B (fail loud): a reloc runtime that links numpy/scipy long double
(CPython-ABI tier) HARD-ERRORS with an actionable message when an archive is
unresolvable — it does not warn-but-proceed. A non-numpy / micro build still
degrades gracefully.

Also: the reloc fingerprint token changes when archive presence flips (so a
degraded cached runtime cannot be served once the archives arrive), and the
vendored copies match their pinned provenance.
"""

from __future__ import annotations

import hashlib
from pathlib import Path

import pytest

from molt.cli import runtime_build as rb
from molt.cli import runtime_wasm_build_timings as timings
from molt.cli import wasm_toolchain

# Pinned provenance (wasi-sdk-33 / LLVM 22.1.0); see vendor/wasm-builtins/README.
_VENDORED = {
    "libc-printscan-long-double.a": (
        111146,
        "744a4c150a0352732923c167ba284f435947f5836205d9470827bb84256148b9",
    ),
    "libclang_rt.builtins-wasm32.a": (
        456060,
        "b1e23c0376609e09052ff225f290d971b0f8eabd3ffd0737e5d0ebb10f1880d1",
    ),
}


def _clear_sysroot_env(monkeypatch: pytest.MonkeyPatch, empty_root: Path) -> None:
    for key in (
        "MOLT_WASI_SYSROOT",
        "WASI_SYSROOT",
        "WASI_SDK_PATH",
        "WASI_SDK_PREFIX",
    ):
        monkeypatch.delenv(key, raising=False)
    # Point the target root at an empty dir so no sysroot resolves from it, and
    # bust the lru_cache that memoised any earlier resolution.
    monkeypatch.setenv("MOLT_TARGET_ROOT", str(empty_root))
    wasm_toolchain._resolve_wasi_sysroot_cached.cache_clear()


def test_vendored_archives_match_pinned_provenance() -> None:
    vendor_dir = wasm_toolchain.wasm_builtins_vendor_dir()
    for name, (size, sha) in _VENDORED.items():
        archive = vendor_dir / name
        assert archive.exists(), f"vendored {name} missing from {vendor_dir}"
        blob = archive.read_bytes()
        assert len(blob) == size, f"{name} size drift"
        assert hashlib.sha256(blob).hexdigest() == sha, f"{name} sha256 drift"


def test_archives_resolve_in_fresh_session_without_sysroot(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    """Part A: a fresh session with no resolvable sysroot still gets both archives."""
    _clear_sysroot_env(monkeypatch, tmp_path)
    assert wasm_toolchain.resolve_wasi_sysroot() is None
    longdouble = wasm_toolchain.wasm_wasi_printscan_long_double_archive()
    builtins = wasm_toolchain.wasm_clang_rt_builtins_archive()
    assert longdouble is not None, "long-double archive did not resolve (no sysroot)"
    assert builtins is not None, "builtins archive did not resolve (no sysroot)"
    # Both came from the committed vendored copy.
    vendor_dir = wasm_toolchain.wasm_builtins_vendor_dir()
    assert longdouble.parent == vendor_dir
    assert builtins.parent == vendor_dir


def test_requires_long_double_gate() -> None:
    req = rb._reloc_runtime_requires_long_double
    # CPython-ABI tier via resolved numpy/scipy modules.
    assert req(resolved_modules={"numpy.core.multiarray"}, required_exports=None)
    assert req(resolved_modules={"scipy.ndimage"}, required_exports=None)
    # Micro / stdlib-only build does not hit %L.
    assert not req(resolved_modules={"os", "sys", "json"}, required_exports=None)
    assert not req(resolved_modules=None, required_exports=None)


def test_witness_tier_fails_loud_when_archive_absent(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """Part B: required + missing => hard error (no degrade) + MISSING attestation."""
    monkeypatch.setattr(
        wasm_toolchain, "wasm_wasi_printscan_long_double_archive", lambda: None
    )
    monkeypatch.setattr(
        wasm_toolchain, "wasm_clang_rt_builtins_archive", lambda: None
    )
    timings._reset_runtime_wasm_build_timings()
    result = rb._resolve_reloc_long_double_archives(long_double_required=True)
    assert result.error is not None, "numpy tier must fail loud, not degrade"
    # Actionable: names the trap, the archives, and how to provision.
    assert "long_double_not_supported" in result.error
    assert "libc-printscan-long-double.a" in result.error
    assert "libclang_rt.builtins-wasm32.a" in result.error
    assert "vendor/wasm-builtins" in result.error
    snapshot = timings._runtime_wasm_build_timings_snapshot()
    assert snapshot is not None
    assert snapshot["longdouble_archives"] == "MISSING"


def test_micro_build_degrades_when_archive_absent(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """A build that provably does not link long double keeps the graceful degrade."""
    monkeypatch.setattr(
        wasm_toolchain, "wasm_wasi_printscan_long_double_archive", lambda: None
    )
    monkeypatch.setattr(
        wasm_toolchain, "wasm_clang_rt_builtins_archive", lambda: None
    )
    timings._reset_runtime_wasm_build_timings()
    result = rb._resolve_reloc_long_double_archives(long_double_required=False)
    assert result.error is None, "micro build must not hard-error"
    assert result.warnings, "micro build with absent archive should warn"
    snapshot = timings._runtime_wasm_build_timings_snapshot()
    assert snapshot is not None
    assert snapshot["longdouble_archives"] == "not_required"


def test_link_hard_errors_before_invoking_wasm_ld(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    """The reloc link chokepoint refuses the numpy tier build with archives absent.

    Proves the hard error fires (returns False) BEFORE ever spawning wasm-ld, so
    a trapping runtime is never produced. wasm-ld / libc are stubbed present so
    the archive check — not a missing toolchain — is what stops the link.
    """
    fake_wasm_ld = tmp_path / "wasm-ld"
    fake_wasm_ld.write_text("", encoding="utf-8")
    fake_libc = tmp_path / "libc.a"
    fake_libc.write_bytes(b"")
    staticlib = tmp_path / "libmolt_runtime.a"
    staticlib.write_bytes(b"")

    monkeypatch.setattr(rb.shutil, "which", lambda _name: str(fake_wasm_ld))
    monkeypatch.setattr(
        wasm_toolchain, "wasm_wasi_libc_archive", lambda *a, **k: fake_libc
    )
    monkeypatch.setattr(
        wasm_toolchain, "wasm_wasi_printscan_long_double_archive", lambda: None
    )
    monkeypatch.setattr(
        wasm_toolchain, "wasm_clang_rt_builtins_archive", lambda: None
    )

    def _boom(*_a, **_k):  # pragma: no cover - must never run
        raise AssertionError("wasm-ld was invoked despite a required archive absent")

    monkeypatch.setattr(rb, "_run_completed_command", _boom)

    ok = rb._link_runtime_staticlib_to_reloc_wasm(
        staticlib_path=staticlib,
        output_path=tmp_path / "molt_runtime.wasm",
        json_output=True,
        link_timeout=None,
        long_double_required=True,
    )
    assert ok is False


def test_fingerprint_token_flips_when_presence_flips(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """A degraded (archives-absent) reloc runtime is keyed apart from a good one.

    So once the archives arrive, the folded fingerprint changes and the cached
    degraded runtime is not served (forces a rebuild).
    """
    token_present = rb._reloc_link_archive_fingerprint_token()
    monkeypatch.setattr(
        wasm_toolchain, "wasm_wasi_printscan_long_double_archive", lambda: None
    )
    monkeypatch.setattr(
        wasm_toolchain, "wasm_clang_rt_builtins_archive", lambda: None
    )
    token_absent = rb._reloc_link_archive_fingerprint_token()
    assert token_present != token_absent


# --- Split app.wasm link: numpy (no reloc runtime here) needs its own formatters ---
import wasm_link  # noqa: E402  (tools/ is on sys.path via conftest)


def test_split_app_wholearchives_longdouble_when_libc_present() -> None:
    args = wasm_link._split_app_native_link_args(
        [Path("numpy_multiarray.o"), Path("libc.a")]
    )
    assert args[0] == "--whole-archive"
    assert args[1].endswith("libc-printscan-long-double.a")
    assert args[2] == "--no-whole-archive"
    assert any(a.endswith("libc.a") for a in args)
    assert any(a.endswith("libclang_rt.builtins-wasm32.a") for a in args)


def test_split_app_plain_passthrough_without_libc() -> None:
    inputs = [Path("extmod.o"), Path("data_alias.o")]
    assert wasm_link._split_app_native_link_args(inputs) == [str(p) for p in inputs]


def test_split_app_fails_loud_when_longdouble_absent(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        wasm_toolchain, "wasm_wasi_printscan_long_double_archive", lambda: None
    )
    with pytest.raises(ValueError, match="long-double|unreachable"):
        wasm_link._split_app_native_link_args([Path("numpy.o"), Path("libc.a")])


# --- Single authority: every wasm link path routes through ONE policy --------
#
# The wasi-libc `long_double_not_supported` stub lives in `libc.a` and must be
# overridden in EVERY wasm module that links it. These lock in that the reloc
# runtime (wasm-ld), split app.wasm (wasm-ld), and deploy cdylib (rustc via
# build.rs env) all resolve the same archives + ordering through the ONE
# `wasm_toolchain` policy — so a future 4th link path can't reintroduce the trap
# by re-implementing resolution.


def _fake_archives(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> tuple[Path, Path]:
    ld = tmp_path / "libc-printscan-long-double.a"
    ld.write_bytes(b"!<arch>\n")
    bi = tmp_path / "libclang_rt.builtins-wasm32.a"
    bi.write_bytes(b"!<arch>\n")
    monkeypatch.setattr(
        wasm_toolchain, "wasm_wasi_printscan_long_double_archive", lambda: ld
    )
    monkeypatch.setattr(wasm_toolchain, "wasm_clang_rt_builtins_archive", lambda: bi)
    return ld, bi


def test_all_three_link_paths_share_the_one_authority(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    ld, bi = _fake_archives(monkeypatch, tmp_path)

    # (1) reloc arm — resolver delegates to the authority.
    reloc = rb._resolve_reloc_long_double_archives(long_double_required=True)
    assert reloc.longdouble == ld
    assert reloc.builtins == bi
    assert reloc.error is None

    # (2) split app.wasm arm — argv whole-archives printscan ahead of libc.a.
    args = wasm_link._split_app_native_link_args([Path("numpy.o"), Path("libc.a")])
    ld_in_args = [a for a in args if Path(a).name == ld.name]
    bi_in_args = [a for a in args if Path(a).name == bi.name]
    assert ld_in_args and Path(ld_in_args[0]).parent == tmp_path.resolve()
    assert bi_in_args and Path(bi_in_args[0]).parent == tmp_path.resolve()
    assert args.index("--whole-archive") < args.index(ld_in_args[0])
    assert args.index(ld_in_args[0]) < args.index("--no-whole-archive")

    # (3) deploy cdylib arm — archives threaded to build.rs by env.
    env: dict[str, str] = {}
    rb._configure_wasm_long_double_env(env)
    assert Path(env["MOLT_WASM_LONGDOUBLE_ARCHIVE"]).parent == tmp_path.resolve()
    assert Path(env["MOLT_WASM_LONGDOUBLE_ARCHIVE"]).name == ld.name
    assert Path(env["MOLT_WASM_BUILTINS_ARCHIVE"]).name == bi.name


def test_shared_argv_order_matches_reloc_policy(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    """The shared argv builder emits printscan in the whole-archive group ahead
    of the (lazy) libc, with builtins trailing — the proven override order."""
    ld, bi = _fake_archives(monkeypatch, tmp_path)
    policy = wasm_toolchain.resolve_long_double_link_policy(required=True)
    argv = wasm_toolchain.long_double_whole_archive_link_argv(
        policy, whole_archive=["staticlib.a"], trailing=["libc.a"]
    )
    assert argv == [
        "--whole-archive",
        "staticlib.a",
        str(ld.resolve(strict=False)),
        "--no-whole-archive",
        "libc.a",
        str(bi.resolve(strict=False)),
    ]


def test_deploy_cdylib_env_absent_when_archive_unresolved(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """No archive -> no env keys: build.rs emits nothing and the artifact poison
    gate (plus the reloc/split-app numpy-tier fail-loud) is the effect backstop.
    """
    monkeypatch.setattr(
        wasm_toolchain, "wasm_wasi_printscan_long_double_archive", lambda: None
    )
    monkeypatch.setattr(wasm_toolchain, "wasm_clang_rt_builtins_archive", lambda: None)
    env: dict[str, str] = {}
    rb._configure_wasm_long_double_env(env)
    assert "MOLT_WASM_LONGDOUBLE_ARCHIVE" not in env
    assert "MOLT_WASM_BUILTINS_ARCHIVE" not in env


def test_shared_and_reloc_fingerprints_fold_archive_identity(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    """Both runtime fingerprints re-key when the archive set changes, so a stale
    long-double-stubbed cached runtime (reloc OR shared cdylib) is never served.
    """
    _fake_archives(monkeypatch, tmp_path)
    token_present = rb._reloc_link_archive_fingerprint_token()
    monkeypatch.setattr(
        wasm_toolchain, "wasm_wasi_printscan_long_double_archive", lambda: None
    )
    token_absent = rb._reloc_link_archive_fingerprint_token()
    assert token_present != token_absent
