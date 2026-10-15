"""The one parser of macOS `sample` reports: leaderboard, call graph, callers.

Fixtures copy the exact line forms `/usr/bin/sample` prints; the expected
numbers follow from the tree by hand.
"""

from __future__ import annotations

from pathlib import Path

from tools import sample_profile


REPORT = """Analysis of sampling bench (pid 123) every 1 millisecond
Call graph:
    100 Thread_1   DispatchQueue_1: com.apple.main-thread  (serial)
      100 start  (in dyld) + 6992  [0x18ff7fda4]
        100 main  (in bench) + 84  [0x1024ec9d4]
          70 hot_loop  (in bench) + 12  [0x1]
          + 40 molt_get_attr  (in bench) + 4  [0x2]
          + ! 25 _tlv_get_addr  (in libdyld.dylib) + 8  [0x3]
          + 20 molt_dec_ref  (in bench) + 4  [0x4]
          30 setup  (in bench) + 4  [0x5]
          + 10 _tlv_get_addr  (in libdyld.dylib) + 8  [0x6]

Total number in stack (recursive counted multiple times, when >=5):

Sort by top of stack, same collapsed (when >= 5):
        molt_dict_lookup  (in bench_x)        4200
        my_hot_symbol        512

Binary Images:
       0x1000 - 0x2000 +bench_x ... /tmp/bench_x
"""


def test_leaderboard_reads_trailing_counts_with_and_without_a_library() -> None:
    assert sample_profile.self_time(REPORT) == [
        {"symbol": "molt_dict_lookup", "self_samples": 4200, "lib": "bench_x"},
        {"symbol": "my_hot_symbol", "self_samples": 512, "lib": None},
    ]
    assert sample_profile.self_time(REPORT, top_n=1)[0]["self_samples"] == 4200
    assert sample_profile.self_time("no leaderboard\nBinary Images:\n") == []


def test_unreadable_report_file_has_no_leaders(tmp_path: Path) -> None:
    assert sample_profile.read_self_time(tmp_path / "missing.txt") == []


def test_call_graph_self_time_subtracts_direct_children_only() -> None:
    frames = sample_profile.call_frames(REPORT)
    # The thread line names no library, so it is not a frame.
    assert [(frame.symbol, frame.lib) for frame in frames[:3]] == [
        ("start", "dyld"),
        ("main", "bench"),
        ("hot_loop", "bench"),
    ]
    leaders = {
        (row["symbol"], row["lib"]): row["self_samples"]
        for row in sample_profile.self_time_from_frames(frames)
    }
    # hot_loop 70 - (molt_get_attr 40 + molt_dec_ref 20) = 10 self samples;
    # molt_get_attr 40 - 25 = 15; _tlv_get_addr sums its two leaf frames.
    assert leaders[("hot_loop", "bench")] == 10
    assert leaders[("molt_get_attr", "bench")] == 15
    assert leaders[("_tlv_get_addr", "libdyld.dylib")] == 35
    assert leaders[("setup", "bench")] == 20
    assert ("main", "bench") not in leaders  # all of main's time is in children
    assert sum(leaders.values()) == 100


def test_direct_callers_attribute_each_matching_frame_to_its_parent() -> None:
    frames = sample_profile.call_frames(REPORT)
    callers = sample_profile.direct_callers(frames, "_tlv_get_addr")
    assert dict(callers) == {"molt_get_attr": 25, "setup": 10}
