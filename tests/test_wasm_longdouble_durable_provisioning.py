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

Part B (fail loud): every runtime family requires its complete captured archive
closure. Missing or mutated archives fail before the linker executes; package
names do not select a weaker identity policy.

Also: missing mandatory archives reject runtime identity capture; changed archive
bytes invalidate both members of the exact runtime family. Vendored copies match
their pinned provenance.
"""

from __future__ import annotations

import hashlib
import subprocess
from pathlib import Path

import pytest

from molt.cli import wasm_link_inputs
from molt.cli import runtime_wasm_build_support as rb
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
    wasm_link_inputs._resolve_wasi_sysroot_cached.cache_clear()


def test_vendored_archives_match_pinned_provenance() -> None:
    vendor_dir = wasm_link_inputs.wasm_builtins_vendor_dir()
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
    assert wasm_link_inputs.resolve_wasi_sysroot() is None
    longdouble = wasm_link_inputs.wasm_wasi_printscan_long_double_archive()
    builtins = wasm_link_inputs.wasm_clang_rt_builtins_archive()
    assert longdouble is not None, "long-double archive did not resolve (no sysroot)"
    assert builtins is not None, "builtins archive did not resolve (no sysroot)"
    # Both came from the committed vendored copy.
    vendor_dir = wasm_link_inputs.wasm_builtins_vendor_dir()
    assert longdouble.parent == vendor_dir
    assert builtins.parent == vendor_dir


@pytest.fixture
def frozen_inputs(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    from tests.runtime_build_identity_helper import runtime_wasm_link_inputs

    inputs = runtime_wasm_link_inputs(tmp_path)
    monkeypatch.setattr(
        wasm_toolchain,
        "resolve_wasm_linker",
        lambda **_kwargs: wasm_toolchain.WasmLinkerIdentity(
            inputs.linker.entrypoint, "22.1.8", None, inputs.linker.identity.sha256
        ),
    )
    monkeypatch.setattr(
        wasm_link_inputs, "wasm_wasi_libc_archive", lambda **_kwargs: inputs.libc.path
    )
    monkeypatch.setattr(
        wasm_link_inputs,
        "wasm_compiler_builtins_archive",
        lambda **_kwargs: inputs.rust_builtins.path,
    )
    monkeypatch.setattr(
        wasm_link_inputs,
        "wasm_wasi_printscan_long_double_archive",
        lambda **_kwargs: inputs.long_double.path,
    )
    monkeypatch.setattr(
        wasm_link_inputs,
        "wasm_clang_rt_builtins_archive",
        lambda **_kwargs: inputs.clang_builtins.path,
    )
    return inputs


@pytest.mark.parametrize(
    "missing",
    ("wasm_wasi_printscan_long_double_archive", "wasm_clang_rt_builtins_archive"),
)
def test_every_runtime_family_requires_complete_archive_custody(
    frozen_inputs,
    monkeypatch: pytest.MonkeyPatch,
    missing: str,
) -> None:
    monkeypatch.setattr(wasm_link_inputs, missing, lambda **_kwargs: None)
    timings._reset_runtime_wasm_build_timings()
    with pytest.raises(ValueError, match="long_double_not_supported") as caught:
        rb.resolve_runtime_wasm_link_inputs(
            env={"WASI_SYSROOT": str(frozen_inputs.wasi_sysroot)},
            target_libdir=frozen_inputs.wasi_sysroot,
            project_root=frozen_inputs.wasi_sysroot,
        )
    message = str(caught.value)
    assert "libc-printscan-long-double.a" in message
    assert "libclang_rt.builtins-wasm32.a" in message
    assert "vendor/wasm-builtins" in message
    assert (
        timings._runtime_wasm_build_timings_snapshot()["longdouble_archives"]
        == "MISSING"
    )


def test_runtime_link_inputs_capture_complete_archive_set(frozen_inputs) -> None:
    assert (
        rb.resolve_runtime_wasm_link_inputs(
            env={"WASI_SYSROOT": str(frozen_inputs.wasi_sysroot)},
            target_libdir=frozen_inputs.wasi_sysroot,
            project_root=frozen_inputs.wasi_sysroot,
        )
        == frozen_inputs
    )


def test_runtime_link_capture_uses_effective_environment_and_selected_rust_root(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from tests.runtime_build_identity_helper import runtime_wasm_link_inputs

    inputs = runtime_wasm_link_inputs(tmp_path)
    selected = tmp_path / "selected-rust-root"
    (selected / "self-contained").mkdir(parents=True)
    libc = selected / "self-contained" / "libc.a"
    libc.write_bytes(b"!<arch>\nselected-libc")
    builtins = selected / "libcompiler_builtins-selected.rlib"
    builtins.write_bytes(b"!<arch>\nselected-builtins")
    environment = {
        "MOLT_WASI_SYSROOT": str(inputs.wasi_sysroot),
        "MOLT_WASM_LONGDOUBLE_ARCHIVE": str(inputs.long_double.path),
        "MOLT_WASM_BUILTINS_ARCHIVE": str(inputs.clang_builtins.path),
    }

    def linker(*, env, cwd):
        assert env == environment
        assert cwd == tmp_path
        return wasm_toolchain.WasmLinkerIdentity(
            inputs.linker.entrypoint, "22.1.8", None, inputs.linker.identity.sha256
        )

    monkeypatch.setattr(wasm_toolchain, "resolve_wasm_linker", linker)
    monkeypatch.setattr(
        wasm_link_inputs,
        "rust_target_libdir",
        lambda *_a, **_k: pytest.fail("captured target must not query ambient rustc"),
    )
    monkeypatch.setenv("MOLT_WASM_LONGDOUBLE_ARCHIVE", str(tmp_path / "ambient-poison"))
    result = rb.resolve_runtime_wasm_link_inputs(
        env=environment, target_libdir=selected, project_root=tmp_path
    )
    assert result.libc.path == libc
    assert result.rust_builtins.path == builtins
    assert result.long_double == inputs.long_double
    result.verify()


def test_runtime_link_custody_preserves_symlink_entrypoint(
    tmp_path: Path,
) -> None:
    from dataclasses import replace
    from molt.cli.runtime_cargo_plan import CargoExecutableCustody
    from tests.runtime_build_identity_helper import runtime_wasm_link_inputs

    inputs = runtime_wasm_link_inputs(tmp_path)
    alias = tmp_path / "wasm-ld"
    try:
        alias.symlink_to(inputs.linker.entrypoint)
    except OSError:
        pytest.skip("host cannot create executable symlinks")
    result = replace(
        inputs, linker=CargoExecutableCustody.capture("runtime WASM linker", alias)
    )
    result.verify()
    assert result.linker.entrypoint == alias
    assert result.linker.identity.path == inputs.linker.identity.path
    alias.unlink()
    alias.write_bytes(b"different-entrypoint")
    with pytest.raises(ValueError, match="changed"):
        result.verify()


def test_link_hard_errors_before_invoking_wasm_ld(
    frozen_inputs,
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    from tests.runtime_build_identity_helper import runtime_cargo_plan

    frozen_inputs.long_double.path.unlink()
    staticlib = tmp_path / "libmolt_runtime.a"
    staticlib.write_bytes(b"!<arch>\n")

    def forbidden(*_a, **_k):
        raise AssertionError("wasm-ld must not run after archive custody is lost")

    monkeypatch.setattr(rb, "_run_completed_command", forbidden)
    with pytest.raises(ValueError, match="unavailable|changed"):
        rb._link_runtime_staticlib_to_reloc_wasm(
            staticlib_path=staticlib,
            output_path=tmp_path / "runtime.wasm",
            json_output=True,
            link_timeout=1.0,
            link_inputs=frozen_inputs,
            cargo_plan=runtime_cargo_plan(tmp_path, env={}, cargo_command=("cargo",)),
        )


def test_runtime_archive_capture_rejects_missing_mandatory_content(
    tmp_path: Path,
) -> None:
    from molt.cli.runtime_build_identity import _archive_identity

    archive = tmp_path / "libc-printscan-long-double.a"
    archive.write_bytes(b"!<arch>\n")
    assert _archive_identity("wasi-long-double", archive)["sha256"]
    archive.unlink()
    with pytest.raises((ValueError, OSError)):
        _archive_identity("wasi-long-double", archive)
    with pytest.raises(ValueError, match="unresolved"):
        _archive_identity("wasi-long-double", None)


@pytest.mark.parametrize(
    "resource",
    (
        "linker",
        "libc",
        "rust_builtins",
        "long_double",
        "clang_builtins",
        "staticlib",
        "response",
    ),
)
def test_reloc_link_rejects_changed_inputs_without_replacing_output(
    frozen_inputs,
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    resource: str,
) -> None:
    from tests.runtime_build_identity_helper import runtime_cargo_plan

    staticlib = tmp_path / "libmolt_runtime.a"
    staticlib.write_bytes(b"!<arch>\n")
    output = tmp_path / "runtime.wasm"
    output.write_bytes(b"old-publication")

    def mutate(command, **kwargs):
        assert kwargs["env"]["CAPTURED_LINK_ENV"] == "original"
        Path(command[-1]).write_bytes(b"\0asm\x01\0\0\0")
        if resource == "response":
            selected = Path(next(arg[1:] for arg in command if arg.startswith("@")))
        elif resource == "staticlib":
            selected = staticlib
        else:
            selected = (
                frozen_inputs.linker.entrypoint
                if resource == "linker"
                else getattr(frozen_inputs, resource).path
            )
        selected.write_bytes(selected.read_bytes() + b"changed")
        return subprocess.CompletedProcess(command, 0, "link-stdout", "link-stderr")

    monkeypatch.setattr(rb, "_run_completed_command", mutate)
    with pytest.raises(rb.RuntimeWasmLinkError, match="changed") as caught:
        rb._link_runtime_staticlib_to_reloc_wasm(
            staticlib_path=staticlib,
            output_path=output,
            json_output=True,
            link_timeout=1.0,
            link_inputs=frozen_inputs,
            cargo_plan=runtime_cargo_plan(
                tmp_path,
                env={"CAPTURED_LINK_ENV": "original"},
                cargo_command=("cargo",),
            ),
            export_link_args="-C link-arg=--export=entry",
        )
    assert caught.value.stdout == "link-stdout"
    assert caught.value.stderr == "link-stderr"
    assert output.read_bytes() == b"old-publication"
    assert not list(tmp_path.glob(".runtime.wasm.*.tmp"))


@pytest.mark.parametrize("timeout", (False, True))
def test_reloc_link_failure_retains_child_evidence(
    frozen_inputs,
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    timeout: bool,
) -> None:
    from tests.runtime_build_identity_helper import runtime_cargo_plan

    staticlib = tmp_path / "libmolt_runtime.a"
    staticlib.write_bytes(b"!<arch>\n")
    output = tmp_path / "runtime.wasm"

    def fail(command, **_kwargs):
        if timeout:
            raise subprocess.TimeoutExpired(
                command, 1.0, output=b"partial stdout", stderr=b"precise cause"
            )
        return subprocess.CompletedProcess(
            command, 2, "partial stdout", "precise cause"
        )

    monkeypatch.setattr(rb, "_run_completed_command", fail)
    with pytest.raises(rb.RuntimeWasmLinkError) as caught:
        rb._link_runtime_staticlib_to_reloc_wasm(
            staticlib_path=staticlib,
            output_path=output,
            json_output=True,
            link_timeout=1.0,
            link_inputs=frozen_inputs,
            cargo_plan=runtime_cargo_plan(tmp_path, env={}, cargo_command=("cargo",)),
        )
    assert caught.value.command[0] == str(frozen_inputs.linker.entrypoint)
    assert caught.value.stdout == "partial stdout"
    assert caught.value.stderr == "precise cause"
    assert caught.value.timed_out is timeout
    assert not output.exists()


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
        wasm_link_inputs,
        "wasm_wasi_printscan_long_double_archive",
        lambda **_kwargs: None,
    )
    with pytest.raises(ValueError, match="long-double|unreachable"):
        wasm_link._split_app_native_link_args([Path("numpy.o"), Path("libc.a")])


# --- Single authority: every wasm link path routes through ONE policy --------
#
# The wasi-libc `long_double_not_supported` stub lives in `libc.a` and must be
# overridden in EVERY wasm module that links it. These lock in that the reloc
# runtime (wasm-ld), split app.wasm (wasm-ld), and deploy cdylib (rustc via
# build.rs env) all resolve the same archives + ordering through the ONE
# `wasm_link_inputs` policy — so a future 4th link path can't reintroduce the trap
# by re-implementing resolution.


def _fake_archives(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> tuple[Path, Path]:
    ld = tmp_path / "libc-printscan-long-double.a"
    ld.write_bytes(b"!<arch>\n")
    bi = tmp_path / "libclang_rt.builtins-wasm32.a"
    bi.write_bytes(b"!<arch>\n")
    monkeypatch.setattr(
        wasm_link_inputs,
        "wasm_wasi_printscan_long_double_archive",
        lambda **_kwargs: ld,
    )
    monkeypatch.setattr(
        wasm_link_inputs, "wasm_clang_rt_builtins_archive", lambda **_kwargs: bi
    )
    return ld, bi


def test_all_three_link_paths_share_the_one_authority(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    ld, bi = _fake_archives(monkeypatch, tmp_path)

    # (1) reloc arm — resolver delegates to the authority.
    reloc = wasm_link_inputs.resolve_long_double_link_policy(required=True)
    assert reloc.printscan == ld
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
    policy = wasm_link_inputs.resolve_long_double_link_policy(required=True)
    argv = wasm_link_inputs.long_double_whole_archive_link_argv(
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
        wasm_link_inputs,
        "wasm_wasi_printscan_long_double_archive",
        lambda **_kwargs: None,
    )
    monkeypatch.setattr(
        wasm_link_inputs, "wasm_clang_rt_builtins_archive", lambda **_kwargs: None
    )
    env: dict[str, str] = {}
    rb._configure_wasm_long_double_env(env)
    assert "MOLT_WASM_LONGDOUBLE_ARCHIVE" not in env
    assert "MOLT_WASM_BUILTINS_ARCHIVE" not in env


def test_shared_and_reloc_families_attest_exact_archive_content(tmp_path: Path) -> None:
    from molt.cli.runtime_build_identity import (
        RuntimeBuildIdentity,
        _archive_identity,
        _digest,
    )
    from tests.runtime_build_identity_helper import runtime_build_identity

    archive = tmp_path / "libc-printscan-long-double.a"
    archive.write_bytes(b"!<arch>\nfirst")
    before = runtime_build_identity("shared").to_dict()

    def with_archive(value):
        family = value["payload"]["family"]
        archives = family["compile"]["toolchain"]["archives"]
        archives[2] = _archive_identity("wasi-long-double", archive)
        compile_digest = _digest(family["compile"])
        family["compile_digest"] = compile_digest
        return tuple(
            RuntimeBuildIdentity(
                _digest({"family": family, "member_kind": kind}),
                compile_digest,
                _digest(family),
                {"family": family, "member_kind": kind},
            )
            for kind in ("shared", "reloc")
        )

    first = with_archive(before)
    archive.write_bytes(b"!<arch>\nother")
    second = with_archive(before)
    assert first[0].family_digest == first[1].family_digest
    assert second[0].family_digest == second[1].family_digest
    assert all(
        old.compile_digest != new.compile_digest and old.digest != new.digest
        for old, new in zip(first, second, strict=True)
    )
