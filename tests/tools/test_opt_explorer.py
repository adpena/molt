"""The optimization explorer reads real objdump, nm and dyld_info shapes.

Each fixture is a small function whose loops and calls are known by
construction, written in the exact text form the tool parses.
"""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO_ROOT / "tools"))


def _load():
    spec = importlib.util.spec_from_file_location(
        "molt_opt_explorer", REPO_ROOT / "tools" / "opt_explorer.py"
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


opt = _load()

# An outer loop (0x10-0x34) holding an inner loop (0x18-0x24). Calls: one direct
# `bl`, one register call through `adrp`+`add`, one through a GOT slot, and one
# whose register the function never set.
ARM64 = """
/tmp/prog_molt:\tfile format mach-o arm64

Disassembly of section __TEXT,__text:

0000000100000000 <_prog__main>:
100000000:     \tstp\tx29, x30, [sp, #-0x10]!
100000004:     \tmov\tx19, #0x0                ; =0
100000008:     \tbl\t0x100009000 <_molt_setup>
10000000c:     \tmov\tx20, #0x0
100000010:     \tadrp\tx14, 0x100008000 <_molt_other+0x10>
100000014:     \tadd\tx14, x14, #0x40
100000018:     \tblr\tx14
10000001c:     \tadd\tx20, x20, #0x1
100000020:     \tcmp\tx20, #0xa
100000024:     \tb.lt\t0x100000018 <_prog__main+0x18>
100000028:     \tadrp\tx16, 0x100010000
10000002c:     \tldr\tx16, [x16, #0x8]
100000030:     \tblr\tx16
100000034:     \tcbnz\tx19, 0x100000010 <_prog__main+0x10>
100000038:     \tblr\tx9
10000003c:     \tret
"""

NM = """0000000100000000 T _prog__main
0000000100008040 t _molt_get_attr_object_ic
0000000100009000 T _molt_setup
000000010065b245 s _attr_data
"""

FIXUPS = """/tmp/prog_molt [arm64]:
    -fixups:
        segment         section          address             type   target
        __DATA_CONST    __got            0x100010008         rebase  0x100009000
        __DATA_CONST    __got            0x100010010           bind  libSystem/_malloc
"""

X86 = """
/tmp/prog_molt:\tfile format elf64-x86-64

Disassembly of section .text:

0000000000401000 <prog__main>:
  401000:\tpush\t%rbp
  401001:\tcall\t0x402000 <molt_add>
  401006:\tcmp\t$0x9,%eax
  401009:\tjle\t0x401001 <prog__main+0x1>
  40100b:\tcall\t*%rax
  40100d:\tret
"""


def _table():
    symbols = opt.parse_nm(NM)
    return opt.SymbolTable(symbols, opt.parse_fixups(FIXUPS, symbols))


def test_symbol_sources_parse_text_symbols_and_got_targets() -> None:
    symbols = opt.parse_nm(NM)
    assert symbols == {
        0x100000000: "prog__main",
        0x100008040: "molt_get_attr_object_ic",
        0x100009000: "molt_setup",
    }
    assert opt.parse_fixups(FIXUPS, symbols) == {
        0x100010008: "molt_setup",
        0x100010010: "malloc",
    }


def test_arm64_calls_resolve_through_registers_and_got_slots() -> None:
    arch, functions = opt.parse_disassembly(ARM64)
    assert arch == "arm64"
    anatomy = opt.analyze_function(
        arch, "prog__main", functions["prog__main"], _table()
    )

    assert anatomy.whole.instructions == 16
    assert dict(anatomy.whole.calls) == {
        "molt_setup": 2,  # direct `bl`, then through its GOT slot
        "molt_get_attr_object_ic": 1,  # adrp + add
        "<indirect>": 1,  # x9 was never materialized here
    }
    # b.lt and cbnz; `ret` ends flow but is not a branch.
    assert anatomy.whole.branches == 2


def test_natural_loops_nest_and_count_only_their_own_blocks() -> None:
    arch, functions = opt.parse_disassembly(ARM64)
    anatomy = opt.analyze_function(
        arch, "prog__main", functions["prog__main"], _table()
    )

    inner, outer = anatomy.loops  # sorted by size
    assert (inner.start, inner.end, inner.instructions) == (0x100000018, 0x100000024, 4)
    assert dict(inner.calls) == {"molt_get_attr_object_ic": 1}
    assert (outer.start, outer.end, outer.instructions) == (
        0x100000010,
        0x100000034,
        10,
    )
    assert dict(outer.calls) == {"molt_get_attr_object_ic": 1, "molt_setup": 1}


def test_x86_calls_and_backward_branches() -> None:
    arch, functions = opt.parse_disassembly(X86)
    assert arch == "x86_64"
    anatomy = opt.analyze_function(arch, "prog__main", functions["prog__main"])

    assert dict(anatomy.whole.calls) == {"molt_add": 1, "<indirect>": 1}
    (loop,) = anatomy.loops
    assert (loop.start, loop.instructions) == (0x401001, 3)
    assert dict(loop.calls) == {"molt_add": 1}


def test_cost_buckets_put_runtime_entry_before_library_ownership() -> None:
    def bucket(symbol: str, lib: str = "prog_molt") -> str:
        return opt.classify_symbol(symbol, lib, "prog_molt", "prog")

    assert bucket("prog__main") == "compiled code"
    assert bucket("_tlv_get_addr", "libdyld.dylib") == "runtime entry"
    assert bucket(
        "_RNvMs4_NtNtCs760eLi01iLW_12molt_runtime11concurrency9execution"
    ) == ("runtime entry")
    assert bucket("molt_dec_ref_obj") == "reference counting"
    assert bucket("_platform_memmove", "libsystem_platform.dylib") == "system libraries"
    assert bucket("molt_hash_string") == "other runtime"

    split = opt.cost_split(
        [
            {"symbol": "prog__main", "self_samples": 30, "lib": "prog_molt"},
            {"symbol": "molt_inc_ref_obj", "self_samples": 50, "lib": "prog_molt"},
            {"symbol": "molt_dec_ref_obj", "self_samples": 20, "lib": "prog_molt"},
        ],
        "prog_molt",
        "prog",
    )
    assert split == [("reference counting", 70, 0.7), ("compiled code", 30, 0.3)]
