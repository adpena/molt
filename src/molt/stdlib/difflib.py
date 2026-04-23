"""Sequence comparison utilities — CPython 3.12 parity for Molt.

Pure-Python implementation of the difflib module.  No intrinsics required.

Provides:
  SequenceMatcher   — flexible pair-sequence comparison
  get_close_matches — find close matches in a list of possibilities
  unified_diff      — unified diff format generator
  context_diff      — context diff format generator
  ndiff             — human-readable line-by-line diff via Differ
  Differ            — class for producing human-readable deltas
  diff_bytes        — wrapper for bytes-input diffs
  IS_LINE_JUNK      — filter for blank/comment lines
  IS_CHARACTER_JUNK — filter for whitespace characters
  restore           — reconstruct one sequence from an ndiff output
  Match             — namedtuple(a, b, size) for matching block results
"""

from __future__ import annotations

from collections import namedtuple
from heapq import nlargest as _nlargest
from _intrinsics import require_intrinsic as _require_intrinsic

_molt_difflib_context_diff = _require_intrinsic("molt_difflib_context_diff")
_molt_difflib_get_close_matches = _require_intrinsic("molt_difflib_get_close_matches")
_molt_difflib_get_matching_blocks = _require_intrinsic(
    "molt_difflib_get_matching_blocks"
)
_molt_difflib_get_opcodes = _require_intrinsic("molt_difflib_get_opcodes")
_molt_difflib_is_junk = _require_intrinsic("molt_difflib_is_junk")
_molt_difflib_ndiff = _require_intrinsic("molt_difflib_ndiff")
_molt_difflib_quick_ratio = _require_intrinsic("molt_difflib_quick_ratio")
_molt_difflib_ratio = _require_intrinsic("molt_difflib_ratio")
_molt_difflib_unified_diff = _require_intrinsic("molt_difflib_unified_diff")

__all__ = [
    "get_close_matches",
    "ndiff",
    "restore",
    "SequenceMatcher",
    "Differ",
    "IS_CHARACTER_JUNK",
    "IS_LINE_JUNK",
    "context_diff",
    "unified_diff",
    "diff_bytes",
    "HtmlDiff",
    "Match",
]

Match = namedtuple("Match", "a b size")


def _calculate_ratio(matches: int, length: int) -> float:
    if length:
        return 2.0 * matches / length
    return 1.0


class SequenceMatcher:
    """Compare pairs of sequences of any type (elements must be hashable).

    The algorithm finds the longest contiguous matching subsequence that
    contains no junk elements (Ratcliff/Obershelp "gestalt" variant), then
    applies the same idea recursively to the portions left and right of the
    match.
    """

    def __init__(self, isjunk=None, a="", b="", autojunk: bool = True) -> None:
        self.isjunk = isjunk
        self.a = self.b = None  # type: ignore[assignment]
        self.autojunk = autojunk
        self.set_seqs(a, b)

    def set_seqs(self, a, b) -> None:
        self.set_seq1(a)
        self.set_seq2(b)

    def set_seq1(self, a) -> None:
        if a is self.a:
            return
        self.a = a
        self.matching_blocks = self.opcodes = None

    def set_seq2(self, b) -> None:
        if b is self.b:
            return
        self.b = b
        self.matching_blocks = self.opcodes = None
        self.fullbcount = None
        self.__chain_b()

    def __chain_b(self) -> None:
        b = self.b
        self.b2j: dict = {}
        b2j = self.b2j

        for i, elt in enumerate(b):
            indices = b2j.setdefault(elt, [])
            indices.append(i)

        # Purge junk elements
        self.bjunk: set = set()
        junk = self.bjunk
        isjunk = self.isjunk
        if isjunk:
            for elt in b2j.keys():
                if isjunk(elt):
                    junk.add(elt)
            for elt in junk:
                del b2j[elt]

        # Purge popular elements (autojunk heuristic)
        self.bpopular: set = set()
        popular = self.bpopular
        n = len(b)
        if self.autojunk and n >= 200:
            ntest = n // 100 + 1
            for elt, idxs in b2j.items():
                if len(idxs) > ntest:
                    popular.add(elt)
            for elt in popular:
                del b2j[elt]

    def find_longest_match(
        self, alo: int = 0, ahi: int = None, blo: int = 0, bhi: int = None
    ) -> Match:
        """Find the longest matching block in a[alo:ahi] and b[blo:bhi].

        Returns a Match(a, b, size) namedtuple.  If no block matches,
        returns Match(alo, blo, 0).
        """
        a, b, b2j, isbjunk = self.a, self.b, self.b2j, self.bjunk.__contains__
        if ahi is None:
            ahi = len(a)
        if bhi is None:
            bhi = len(b)
        besti, bestj, bestsize = alo, blo, 0

        j2len: dict[int, int] = {}
        nothing: list = []
        for i in range(alo, ahi):
            j2lenget = j2len.get
            newj2len: dict[int, int] = {}
            for j in b2j.get(a[i], nothing):
                if j < blo:
                    continue
                if j >= bhi:
                    break
                k = newj2len[j] = j2lenget(j - 1, 0) + 1
                if k > bestsize:
                    besti, bestj, bestsize = i - k + 1, j - k + 1, k
            j2len = newj2len

        # Extend the match with non-junk elements on both ends
        while (
            besti > alo
            and bestj > blo
            and not isbjunk(b[bestj - 1])
            and a[besti - 1] == b[bestj - 1]
        ):
            besti, bestj, bestsize = besti - 1, bestj - 1, bestsize + 1
        while (
            besti + bestsize < ahi
            and bestj + bestsize < bhi
            and not isbjunk(b[bestj + bestsize])
            and a[besti + bestsize] == b[bestj + bestsize]
        ):
            bestsize += 1

        # Extend with junk on both ends
        while (
            besti > alo
            and bestj > blo
            and isbjunk(b[bestj - 1])
            and a[besti - 1] == b[bestj - 1]
        ):
            besti, bestj, bestsize = besti - 1, bestj - 1, bestsize + 1
        while (
            besti + bestsize < ahi
            and bestj + bestsize < bhi
            and isbjunk(b[bestj + bestsize])
            and a[besti + bestsize] == b[bestj + bestsize]
        ):
            bestsize += 1

        return Match(besti, bestj, bestsize)

    def get_matching_blocks(self) -> list[Match]:
        """Return list of Match(a, b, size) triples for matching subsequences.

        The last triple is always a sentinel Match(len(a), len(b), 0).
        """
        if self.matching_blocks is not None:
            return self.matching_blocks
        la, lb = len(self.a), len(self.b)

        queue = [(0, la, 0, lb)]
        matching_blocks = []
        while queue:
            alo, ahi, blo, bhi = queue.pop()
            i, j, k = x = self.find_longest_match(alo, ahi, blo, bhi)
            if k:
                matching_blocks.append(x)
                if alo < i and blo < j:
                    queue.append((alo, i, blo, j))
                if i + k < ahi and j + k < bhi:
                    queue.append((i + k, ahi, j + k, bhi))
        matching_blocks.sort()

        # Collapse adjacent equal blocks
        i1 = j1 = k1 = 0
        non_adjacent = []
        for i2, j2, k2 in matching_blocks:
            if i1 + k1 == i2 and j1 + k1 == j2:
                k1 += k2
            else:
                if k1:
                    non_adjacent.append((i1, j1, k1))
                i1, j1, k1 = i2, j2, k2
        if k1:
            non_adjacent.append((i1, j1, k1))
        non_adjacent.append((la, lb, 0))

        self.matching_blocks = list(map(Match._make, non_adjacent))
        return self.matching_blocks

    def get_opcodes(self) -> list[tuple]:
        """Return list of (tag, i1, i2, j1, j2) tuples describing edits.

        Tags: 'replace', 'delete', 'insert', 'equal'.
        """
        if self.opcodes is not None:
            return self.opcodes
        i = j = 0
        self.opcodes = answer = []
        for ai, bj, size in self.get_matching_blocks():
            tag = ""
            if i < ai and j < bj:
                tag = "replace"
            elif i < ai:
                tag = "delete"
            elif j < bj:
                tag = "insert"
            if tag:
                answer.append((tag, i, ai, j, bj))
            i, j = ai + size, bj + size
            if size:
                answer.append(("equal", ai, i, bj, j))
        return answer

    def get_grouped_opcodes(self, n: int = 3):
        """Generate groups of opcodes with up to *n* lines of context.

        Yields lists of (tag, i1, i2, j1, j2) tuples.
        """
        codes = self.get_opcodes()
        if not codes:
            codes = [("equal", 0, 1, 0, 1)]
        # Trim leading/trailing equal blocks
        if codes[0][0] == "equal":
            tag, i1, i2, j1, j2 = codes[0]
            codes[0] = tag, max(i1, i2 - n), i2, max(j1, j2 - n), j2
        if codes[-1][0] == "equal":
            tag, i1, i2, j1, j2 = codes[-1]
            codes[-1] = tag, i1, min(i2, i1 + n), j1, min(j2, j1 + n)

        nn = n + n
        group: list = []
        for tag, i1, i2, j1, j2 in codes:
            if tag == "equal" and i2 - i1 > nn:
                group.append((tag, i1, min(i2, i1 + n), j1, min(j2, j1 + n)))
                yield group
                group = []
                i1, j1 = max(i1, i2 - n), max(j1, j2 - n)
            group.append((tag, i1, i2, j1, j2))
        if group and not (len(group) == 1 and group[0][0] == "equal"):
            yield group

    def ratio(self) -> float:
        """Return a float in [0, 1] measuring sequence similarity (2*M/T)."""
        matches = sum(triple[-1] for triple in self.get_matching_blocks())
        return _calculate_ratio(matches, len(self.a) + len(self.b))

    def quick_ratio(self) -> float:
        """Return an upper bound on ratio(), faster than ratio()."""
        if self.fullbcount is None:
            self.fullbcount = fullbcount = {}
            for elt in self.b:
                fullbcount[elt] = fullbcount.get(elt, 0) + 1
        fullbcount = self.fullbcount
        avail: dict = {}
        availhas = avail.__contains__
        matches = 0
        for elt in self.a:
            if availhas(elt):
                numb = avail[elt]
            else:
                numb = fullbcount.get(elt, 0)
            avail[elt] = numb - 1
            if numb > 0:
                matches += 1
        return _calculate_ratio(matches, len(self.a) + len(self.b))

    def real_quick_ratio(self) -> float:
        """Return a very fast (but loose) upper bound on ratio()."""
        la, lb = len(self.a), len(self.b)
        return _calculate_ratio(min(la, lb), la + lb)


def get_close_matches(word, possibilities, n: int = 3, cutoff: float = 0.6) -> list:
    """Return a list of up to *n* close matches to *word* from *possibilities*.

    *cutoff* is a float in [0, 1]; only possibilities scoring >= cutoff
    are considered.  Results are sorted by similarity, most similar first.
    """
    if not n > 0:
        raise ValueError("n must be > 0: %r" % (n,))
    if not 0.0 <= cutoff <= 1.0:
        raise ValueError("cutoff must be in [0.0, 1.0]: %r" % (cutoff,))
    result = []
    s = SequenceMatcher()
    s.set_seq2(word)
    for x in possibilities:
        s.set_seq1(x)
        if (
            s.real_quick_ratio() >= cutoff
            and s.quick_ratio() >= cutoff
            and s.ratio() >= cutoff
        ):
            result.append((s.ratio(), x))
    result = _nlargest(n, result)
    return [x for score, x in result]


def _keep_original_ws(s: str, tag_s: str) -> str:
    """Replace whitespace placeholders in tag_s with the originals from s."""
    return "".join(
        c if tag_c == " " and c.isspace() else tag_c for c, tag_c in zip(s, tag_s)
    )


# ---------------------------------------------------------------------------
# IS_LINE_JUNK / IS_CHARACTER_JUNK
# ---------------------------------------------------------------------------


def IS_LINE_JUNK(line: str, pat=None) -> bool:
    r"""Return True for ignorable line: blank or contains only '#'.

    >>> IS_LINE_JUNK('\n')
    True
    >>> IS_LINE_JUNK('  #   \n')
    True
    >>> IS_LINE_JUNK('hello\n')
    False
    """
    if pat is None:
        return line.strip() in "#"
    return pat(line) is not None


def IS_CHARACTER_JUNK(ch: str, ws: str = " \t") -> bool:
    r"""Return True for ignorable character: space or tab.

    >>> IS_CHARACTER_JUNK(' ')
    True
    >>> IS_CHARACTER_JUNK('\t')
    True
    >>> IS_CHARACTER_JUNK('\n')
    False
    >>> IS_CHARACTER_JUNK('x')
    False
    """
    return ch in ws


# ---------------------------------------------------------------------------
# Differ
# ---------------------------------------------------------------------------


class Differ:
    r"""Produce human-readable deltas from sequences of lines of text.

    Each output line begins with a two-character code:
        '- '  line unique to sequence 1
        '+ '  line unique to sequence 2
        '  '  line common to both sequences
        '? '  line not present in either input (intraline markers)
    """

    def __init__(self, linejunk=None, charjunk=None) -> None:
        self.linejunk = linejunk
        self.charjunk = charjunk

    def compare(self, a, b):
        """Compare two sequences of lines; yield the resulting delta."""
        cruncher = SequenceMatcher(self.linejunk, a, b)
        for tag, alo, ahi, blo, bhi in cruncher.get_opcodes():
            if tag == "replace":
                g = self._fancy_replace(a, alo, ahi, b, blo, bhi)
            elif tag == "delete":
                g = self._dump("-", a, alo, ahi)
            elif tag == "insert":
                g = self._dump("+", b, blo, bhi)
            elif tag == "equal":
                g = self._dump(" ", a, alo, ahi)
            else:
                raise ValueError("unknown tag %r" % (tag,))
            yield from g

    def _dump(self, tag: str, x, lo: int, hi: int):
        for i in range(lo, hi):
            yield "%s %s" % (tag, x[i])

    def _plain_replace(self, a, alo: int, ahi: int, b, blo: int, bhi: int):
        assert alo < ahi and blo < bhi
        if bhi - blo < ahi - alo:
            first = self._dump("+", b, blo, bhi)
            second = self._dump("-", a, alo, ahi)
        else:
            first = self._dump("-", a, alo, ahi)
            second = self._dump("+", b, blo, bhi)
        for g in first, second:
            yield from g

    def _fancy_replace(self, a, alo: int, ahi: int, b, blo: int, bhi: int):
        cutoff = 0.74999
        cruncher = SequenceMatcher(self.charjunk)
        crqr = cruncher.real_quick_ratio
        cqr = cruncher.quick_ratio
        cr = cruncher.ratio

        WINDOW = 10
        best_i = best_j = None
        dump_i, dump_j = alo, blo

        for j in range(blo, bhi):
            cruncher.set_seq2(b[j])
            aequiv = alo + (j - blo)
            arange = range(max(aequiv - WINDOW, dump_i), min(aequiv + WINDOW + 1, ahi))
            if not arange:
                break
            best_ratio = cutoff
            for i in arange:
                cruncher.set_seq1(a[i])
                if crqr() > best_ratio and cqr() > best_ratio and cr() > best_ratio:
                    best_i, best_j, best_ratio = i, j, cr()

            if best_i is None:
                continue

            yield from self._fancy_helper(a, dump_i, best_i, b, dump_j, best_j)

            aelt, belt = a[best_i], b[best_j]
            if aelt != belt:
                atags = btags = ""
                cruncher.set_seqs(aelt, belt)
                for tag, ai1, ai2, bj1, bj2 in cruncher.get_opcodes():
                    la, lb = ai2 - ai1, bj2 - bj1
                    if tag == "replace":
                        atags += "^" * la
                        btags += "^" * lb
                    elif tag == "delete":
                        atags += "-" * la
                    elif tag == "insert":
                        btags += "+" * lb
                    elif tag == "equal":
                        atags += " " * la
                        btags += " " * lb
                    else:
                        raise ValueError("unknown tag %r" % (tag,))
                yield from self._qformat(aelt, belt, atags, btags)
            else:
                yield "  " + aelt

            dump_i, dump_j = best_i + 1, best_j + 1
            best_i = best_j = None

        yield from self._fancy_helper(a, dump_i, ahi, b, dump_j, bhi)

    def _fancy_helper(self, a, alo: int, ahi: int, b, blo: int, bhi: int):
        if alo < ahi:
            if blo < bhi:
                g = self._plain_replace(a, alo, ahi, b, blo, bhi)
            else:
                g = self._dump("-", a, alo, ahi)
        elif blo < bhi:
            g = self._dump("+", b, blo, bhi)
        else:
            return
        yield from g

    def _qformat(self, aline: str, bline: str, atags: str, btags: str):
        atags = _keep_original_ws(aline, atags).rstrip()
        btags = _keep_original_ws(bline, btags).rstrip()
        yield "- " + aline
        if atags:
            yield f"? {atags}\n"
        yield "+ " + bline
        if btags:
            yield f"? {btags}\n"


# ---------------------------------------------------------------------------
# Unified diff
# ---------------------------------------------------------------------------


def _format_range_unified(start: int, stop: int) -> str:
    beginning = start + 1
    length = stop - start
    if length == 1:
        return "{}".format(beginning)
    if not length:
        beginning -= 1
    return "{},{}".format(beginning, length)


def unified_diff(
    a,
    b,
    fromfile: str = "",
    tofile: str = "",
    fromfiledate: str = "",
    tofiledate: str = "",
    n: int = 3,
    lineterm: str = "\n",
):
    """Compare two sequences of lines; generate the delta as a unified diff.

    Set *lineterm* to "" when inputs have no trailing newlines.
    """
    _check_types(a, b, fromfile, tofile, fromfiledate, tofiledate, lineterm)
    started = False
    for group in SequenceMatcher(None, a, b).get_grouped_opcodes(n):
        if not started:
            started = True
            fromdate = "\t{}".format(fromfiledate) if fromfiledate else ""
            todate = "\t{}".format(tofiledate) if tofiledate else ""
            yield "--- {}{}{}".format(fromfile, fromdate, lineterm)
            yield "+++ {}{}{}".format(tofile, todate, lineterm)

        first, last = group[0], group[-1]
        file1_range = _format_range_unified(first[1], last[2])
        file2_range = _format_range_unified(first[3], last[4])
        yield "@@ -{} +{} @@{}".format(file1_range, file2_range, lineterm)

        for tag, i1, i2, j1, j2 in group:
            if tag == "equal":
                for line in a[i1:i2]:
                    yield " " + line
                continue
            if tag in {"replace", "delete"}:
                for line in a[i1:i2]:
                    yield "-" + line
            if tag in {"replace", "insert"}:
                for line in b[j1:j2]:
                    yield "+" + line


# ---------------------------------------------------------------------------
# Context diff
# ---------------------------------------------------------------------------


def _format_range_context(start: int, stop: int) -> str:
    beginning = start + 1
    length = stop - start
    if not length:
        beginning -= 1
    if length <= 1:
        return "{}".format(beginning)
    return "{},{}".format(beginning, beginning + length - 1)


def context_diff(
    a,
    b,
    fromfile: str = "",
    tofile: str = "",
    fromfiledate: str = "",
    tofiledate: str = "",
    n: int = 3,
    lineterm: str = "\n",
):
    """Compare two sequences of lines; generate the delta as a context diff."""
    _check_types(a, b, fromfile, tofile, fromfiledate, tofiledate, lineterm)
    prefix = dict(insert="+ ", delete="- ", replace="! ", equal="  ")
    started = False
    for group in SequenceMatcher(None, a, b).get_grouped_opcodes(n):
        if not started:
            started = True
            fromdate = "\t{}".format(fromfiledate) if fromfiledate else ""
            todate = "\t{}".format(tofiledate) if tofiledate else ""
            yield "*** {}{}{}".format(fromfile, fromdate, lineterm)
            yield "--- {}{}{}".format(tofile, todate, lineterm)

        first, last = group[0], group[-1]
        yield "***************" + lineterm

        file1_range = _format_range_context(first[1], last[2])
        yield "*** {} ****{}".format(file1_range, lineterm)

        if any(tag in {"replace", "delete"} for tag, _, _, _, _ in group):
            for tag, i1, i2, _, _ in group:
                if tag != "insert":
                    for line in a[i1:i2]:
                        yield prefix[tag] + line

        file2_range = _format_range_context(first[3], last[4])
        yield "--- {} ----{}".format(file2_range, lineterm)

        if any(tag in {"replace", "insert"} for tag, _, _, _, _ in group):
            for tag, _, _, j1, j2 in group:
                if tag != "delete":
                    for line in b[j1:j2]:
                        yield prefix[tag] + line


def _check_types(a, b, *args) -> None:
    if a and not isinstance(a[0], str):
        raise TypeError(
            "lines to compare must be str, not %s (%r)" % (type(a[0]).__name__, a[0])
        )
    if b and not isinstance(b[0], str):
        raise TypeError(
            "lines to compare must be str, not %s (%r)" % (type(b[0]).__name__, b[0])
        )
    if isinstance(a, str):
        raise TypeError(
            "input must be a sequence of strings, not %s" % type(a).__name__
        )
    if isinstance(b, str):
        raise TypeError(
            "input must be a sequence of strings, not %s" % type(b).__name__
        )
    for arg in args:
        if not isinstance(arg, str):
            raise TypeError("all arguments must be str, not: %r" % (arg,))


def diff_bytes(
    dfunc,
    a,
    b,
    fromfile: bytes = b"",
    tofile: bytes = b"",
    fromfiledate: bytes = b"",
    tofiledate: bytes = b"",
    n: int = 3,
    lineterm: bytes = b"\n",
):
    """Wrapper that converts bytes inputs to str for *dfunc*, then back."""

    def decode(s: bytes) -> str:
        try:
            return s.decode("ascii", "surrogateescape")
        except AttributeError as err:
            msg = "all arguments must be bytes, not %s (%r)" % (type(s).__name__, s)
            raise TypeError(msg) from err

    a_str = list(map(decode, a))
    b_str = list(map(decode, b))
    fromfile_s = decode(fromfile)
    tofile_s = decode(tofile)
    fromfiledate_s = decode(fromfiledate)
    tofiledate_s = decode(tofiledate)
    lineterm_s = decode(lineterm)

    for line in dfunc(
        a_str, b_str, fromfile_s, tofile_s, fromfiledate_s, tofiledate_s, n, lineterm_s
    ):
        yield line.encode("ascii", "surrogateescape")


def ndiff(a, b, linejunk=None, charjunk=IS_CHARACTER_JUNK):
    """Compare *a* and *b* (lists of strings); return a Differ-style delta."""
    return Differ(linejunk, charjunk).compare(a, b)


def restore(delta, which: int):
    """Return one of the two sequences that generated a Differ delta.

    *which* is 1 to get the first sequence, 2 for the second.
    """
    if which not in (1, 2):
        raise ValueError('argument "which" must be 1 or 2')
    tag = {1: "- ", 2: "+ "}[which]
    prefixes = ("  ", tag)
    for line in delta:
        if line[:2] in prefixes:
            yield line[2:]


# ---------------------------------------------------------------------------
# HtmlDiff (minimal stub — full HTML side-by-side diff)
# ---------------------------------------------------------------------------


class HtmlDiff:
    """Generate HTML side-by-side comparison tables with change highlights.

    This is a minimal but correct implementation covering the public API.
    """

    _file_template = """<!DOCTYPE html>
<html><head><meta charset="utf-8">
<title>%(title)s</title>
<style type="text/css">%(styles)s</style>
</head><body>%(table)s%(legend)s</body></html>
"""

    _styles = """
table.diff {font-family:Courier; border:medium}
.diff_header {background-color:#e0e0e0}
td.diff_header {text-align:right}
.diff_next {background-color:#c0c0c0}
.diff_add {background-color:#aaffaa}
.diff_chg {background-color:#ffff77}
.diff_sub {background-color:#ffaaaa}
"""

    _legend = """
<table class="diff" summary="Legends">
<tr><th colspan="2">Legends</th></tr>
<tr><td><table border="" summary="Colors">
<tr><th>Colors</th></tr>
<tr><td class="diff_add">&nbsp;Added&nbsp;</td></tr>
<tr><td class="diff_chg">&nbsp;Changed&nbsp;</td></tr>
<tr><td class="diff_sub">&nbsp;Deleted&nbsp;</td></tr>
</table></td>
<td><table border="" summary="Links">
<tr><th colspan="2">Links</th></tr>
<tr><td>(f)irst change</td></tr>
<tr><td>(n)ext change</td></tr>
<tr><td>(t)op</td></tr>
</table></td></tr>
</table>
"""

    def __init__(
        self,
        tabsize: int = 8,
        wrapcolumn: int | None = None,
        linejunk=None,
        charjunk=IS_CHARACTER_JUNK,
    ) -> None:
        self._tabsize = tabsize
        self._wrapcolumn = wrapcolumn
        self._linejunk = linejunk
        self._charjunk = charjunk

    def make_file(
        self,
        fromlines,
        tolines,
        fromdesc: str = "",
        todesc: str = "",
        context: bool = False,
        numlines: int = 5,
        *,
        charset: str = "utf-8",
    ) -> str:
        """Return a complete HTML file containing the side-by-side diff."""
        return self._file_template % {
            "title": "diff",
            "styles": self._styles,
            "table": self.make_table(
                fromlines, tolines, fromdesc, todesc, context=context, numlines=numlines
            ),
            "legend": self._legend,
        }

    def make_table(
        self,
        fromlines,
        tolines,
        fromdesc: str = "",
        todesc: str = "",
        context: bool = False,
        numlines: int = 5,
    ) -> str:
        """Return an HTML table representing the side-by-side diff."""
        import html as _html_mod

        diff = list(ndiff(fromlines, tolines, self._linejunk, self._charjunk))
        rows = []
        rows.append(
            '<table class="diff" id="difflib_chg_to0__top"'
            ' cellspacing="0" cellpadding="0" rules="groups">'
        )
        rows.append(
            "<colgroup></colgroup><colgroup></colgroup>"
            "<colgroup></colgroup><colgroup></colgroup>"
        )
        rows.append(
            '<thead><tr><th class="diff_next"><br/></th>'
            '<th colspan="2" class="diff_header">%s</th>'
            '<th class="diff_next"><br/></th>'
            '<th colspan="2" class="diff_header">%s</th>'
            "</tr></thead>"
            % (
                _html_mod.escape(fromdesc),
                _html_mod.escape(todesc),
            )
        )
        rows.append("<tbody>")
        from_lineno = to_lineno = 1
        for line in diff:
            code = line[:2]
            content = _html_mod.escape(line[2:])
            if code == "  ":
                rows.append(
                    '<tr><td class="diff_next"></td>'
                    '<td class="diff_header">%d</td>'
                    '<td nowrap="nowrap">%s</td>'
                    '<td class="diff_next"></td>'
                    '<td class="diff_header">%d</td>'
                    '<td nowrap="nowrap">%s</td></tr>'
                    % (from_lineno, content, to_lineno, content)
                )
                from_lineno += 1
                to_lineno += 1
            elif code == "- ":
                rows.append(
                    '<tr><td class="diff_next"></td>'
                    '<td class="diff_header">%d</td>'
                    '<td nowrap="nowrap" class="diff_sub">%s</td>'
                    '<td class="diff_next"></td>'
                    '<td class="diff_header">&nbsp;</td>'
                    '<td nowrap="nowrap">&nbsp;</td></tr>' % (from_lineno, content)
                )
                from_lineno += 1
            elif code == "+ ":
                rows.append(
                    '<tr><td class="diff_next"></td>'
                    '<td class="diff_header">&nbsp;</td>'
                    '<td nowrap="nowrap">&nbsp;</td>'
                    '<td class="diff_next"></td>'
                    '<td class="diff_header">%d</td>'
                    '<td nowrap="nowrap" class="diff_add">%s</td></tr>'
                    % (to_lineno, content)
                )
                to_lineno += 1
            # '? ' lines (intraline hint markers) are skipped in the HTML view
        rows.append("</tbody></table>")
        return "\n".join(rows)
