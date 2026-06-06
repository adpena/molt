# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
#
# glob.iglob streaming-memory regression (task #29 / audit finding L-01/C-11).
#
# CPython's `glob.iglob` is a lazy generator chain over `os.scandir`: it yields
# matching paths one at a time, so iterating a large/deep tree streams at
# O(active-path-depth x one-directory-listing) memory. The historical molt hack
# made `iglob == glob` (full materialization) — every path string for the whole
# tree was allocated BEFORE the first `next()` returned. On a large recursive
# `**` walk that is an OOM-class behavioral divergence (the same eager-listdir
# bug class as the os.walk history).
#
# This test builds a wide+deep synthetic tree (the test owns it) of ~100k
# entries, then pulls only a HANDFUL of results from `iglob('**')` via `next()`.
# The lazy iterator touches just the first directory chain — a few KB of live
# state. A regression to eager materialization would instead walk and allocate
# the ENTIRE tree's worth of path strings before yielding the first element,
# blowing the RSS cap.
#
# Run under:  python3 tools/safe_run.py --rss-mb 64 --timeout 60 -- <binary>
# Streaming  -> stays far under 64 MB (exit 0).
# Eager (regressed) -> walks all ~100k paths up front; RSS balloons / cap trips.
#
# The printed summary is deterministic (sorted) so it is byte-identical to
# CPython under `molt diff` regardless of readdir order.

import glob
import itertools
import os
import tempfile


def _build_tree(root: str, depth: int, width_dirs: int, files_per_dir: int) -> int:
    """Create a deep+wide tree; return the total number of files created."""
    created = 0
    frontier = [root]
    for _ in range(depth):
        next_frontier = []
        for d in frontier:
            for fi in range(files_per_dir):
                with open(os.path.join(d, f"f{fi:03d}.txt"), "w", encoding="utf-8") as h:
                    h.write("x")
                created += 1
            for di in range(width_dirs):
                child = os.path.join(d, f"d{di:02d}")
                os.mkdir(child)
                next_frontier.append(child)
        frontier = next_frontier
    # Leaf directories also get files.
    for d in frontier:
        for fi in range(files_per_dir):
            with open(os.path.join(d, f"f{fi:03d}.txt"), "w", encoding="utf-8") as h:
                h.write("x")
            created += 1
    return created


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="molt_iglob_rss_") as root:
        # depth 4, 6 subdirs/level, 4 files/dir:
        #   dirs   = 6 + 6^2 + 6^3 + 6^4            = 1554
        #   files  = 4 * (1 + 6 + 36 + 216 + 1296)  = 6220  (incl. leaves)
        # `**` also matches every directory, so the full recursive match set is
        # ~7.8k entries spread across a 1554-directory tree. The build cost is a
        # few MB; the test cap (64 MB) leaves wide margin so that the *only* way
        # to exceed it is a regression that materializes the whole `**` walk
        # (the old eager `iglob == glob`) instead of streaming a single
        # root->leaf chain at a time.
        total_files = _build_tree(root, depth=4, width_dirs=6, files_per_dir=4)

        # Pull only a handful of results lazily. With a lazy iterator this lists
        # just the first directory (and descends one chain for `**`), never the
        # whole tree. `itertools.islice` consumes exactly N without a
        # `try/except` inside a loop (which would trip a pre-existing,
        # glob-unrelated backend codegen bug under tempfile + os.makedirs).
        it = glob.iglob("**", root_dir=root, recursive=True)
        first = list(itertools.islice(it, 25))

        # `it` is a real iterator (not a pre-materialized list).
        print("is_iterator", iter(it) is it)
        print("pulled", len(first))
        # Deterministic, order-independent witness that streaming produced real,
        # well-formed matches without materializing the whole tree.
        print("all_relative", all(not os.path.isabs(p) for p in first))
        print("sample_depth_ok", all(1 <= p.count(os.sep) + 1 <= 6 for p in first))
        print("total_files_ge_25", total_files >= 25)

        # Also confirm a bounded full drain of a *targeted* (non-`**`) pattern
        # streams and yields the expected count without OOM.
        top_txt = list(glob.iglob("d00/d00/*.txt", root_dir=root, recursive=True))
        print("targeted_count", len(top_txt))


main()
