#!/usr/bin/env python3
"""APPARATUS A10 -- make the memory system READABLE by machine, not just writable.

pact named the motivating bug class in its ``graph_memory`` docstring: the
GOLDFISH class -- "apparatus WRITES better than it READS." molt's own
``MEMORY.md`` header says the same thing a different way: "a stale line once cost
a 30-min detour." Both are the failure where a durable, cross-referenced memory
corpus exists on disk but nothing *traverses* it: the next session (or the same
session post-compaction, M22) re-reads everything from scratch, or worse, trusts
a stale hook line instead of the topic file.

This module parses the molt memory corpus into a TYPED, bidirectional graph and
exposes RECALL queries -- reconstructed subgraph traversal, not flat grep:

  nodes  : memory | finding | tool | gate | lane | decision | ledger
  edges  : links (from ``[[wikilinks]]`` + markdown links) | supersedes |
           produces | consumes | cites

Sources parsed (each independently guarded -- a broken source degrades to fewer
nodes, never a crash):

  * ``memory/MEMORY.md``   -- the M## hook index (node defs + cross-cites)
  * ``memory/POINTERS.md`` -- the M## -> topic-file resolver (aliases)
  * ``memory/*.md``        -- topic files (memory nodes; wikilinks; supersedes)
  * ``docs/agent/*.md``    -- ledger files (PROOF_QUEUE/CLAIMS/POISON/... ) +
                              CLAIMS lane rows (lane nodes) + decisions
  * findings registry      -- ``tools.findings_registry.query_findings`` findings
                              with producer/consumer edges (the A4 layer)
  * ``tools/molt_dev_gates.toml`` -- gate + gate_flip names (gate nodes)

Recall API (importable) + a CLI::

    python tools/memory_graph.py neighbors M12
    python tools/memory_graph.py supersedes M52
    python tools/memory_graph.py what-consumes probe_int_checkedmul_peel_v1
    python tools/memory_graph.py nearest "witness numpy wasm seal frontier"
    python tools/memory_graph.py stats
    python tools/memory_graph.py obsidian-export --out /tmp/mg

Consumers wired so it is READ, not just built (the whole point of A10):
  1. ``tools/hooks/session_digest.py`` -- surfaces the 3 nearest memories to the
     current lane/goal context in the SessionStart digest (fail-open, <5s).
  2. ``tools/findings_memo_lint.py``   -- when it flags a memo line missing a
     ``finding_id``, it SUGGESTS candidate link targets from this graph.

Pure stdlib (``tomllib`` is optional/guarded) so the hook spine can import it
under any Python 3.9+ without the molt venv. Never raises on load; the CLI is the
only place that prints.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from collections import defaultdict
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Optional

# --- constants -------------------------------------------------------------

NODE_TYPES = frozenset(
    {"memory", "finding", "tool", "gate", "lane", "decision", "ledger"}
)
EDGE_TYPES = frozenset({"links", "supersedes", "produces", "consumes", "cites"})

# ``[[wikilink]]`` -- inner text is a topic slug (optionally ``[[slug|alias]]``).
_WIKILINK_RE = re.compile(r"\[\[([^\]|#]+?)(?:[|#][^\]]*)?\]\]")
# Markdown link to a local ``.md`` file: ``[text](some/path/slug.md)``.
_MD_LINK_RE = re.compile(r"\]\(([^)]+?\.md)(?:#[^)]*)?\)")
# A directive id token (M## / M###) with word boundaries.
_MREF_RE = re.compile(r"\bM(\d{2,3})\b")
# A tool path token.
_TOOL_PATH_RE = re.compile(r"\btools/[A-Za-z0-9_./-]+\.py\b")
# A registered finding id (snake_case + trailing _vN).
_FINDING_ID_RE = re.compile(r"\b[a-z][a-z0-9_]*_v\d+\b")
# A ``See <slug>`` trailing cross-reference in a MEMORY.md hook line.
_SEE_REF_RE = re.compile(r"\bSee\s+([a-z0-9][a-z0-9-]{4,})\b")
# Supersession vocabulary on a line.
_SUPERSEDE_RE = re.compile(r"\bsupersed(?:e|es|ed|ing)\b", re.IGNORECASE)
# A landed-tool convention: "... LANDED ... tools/x.py ..." on one line.
_LANDED_RE = re.compile(r"\bLANDED\b", re.IGNORECASE)
# A decision / verdict marker.
_DECISION_RE = re.compile(
    r"^#{0,6}\s*(DECISION|VERDICT|RESOLVED|RULING)\b[:\s-]+(.+?)\s*$", re.IGNORECASE
)

_STOPWORDS = frozenset(
    """
    the a an and or of to in on for with without into onto from by at as is are
    was were be been being it its this that these those not no yes but so if then
    else than via per not any all each some more most less least very much many
    few can may must should will would shall do does did done has have had having
    you your our their his her they them we us he she who whom which what when
    where why how one two three new old use used using only also just now
    """.split()
)

_TOKEN_RE = re.compile(r"[A-Za-z][A-Za-z0-9_-]{2,}")


def _tokenize(text: str) -> set[str]:
    """Significant lowercased tokens (>=3 chars, non-stopword) for ranking."""
    out: set[str] = set()
    for m in _TOKEN_RE.finditer(text or ""):
        tok = m.group(0).lower()
        if len(tok) < 3 or tok in _STOPWORDS:
            continue
        out.add(tok)
    return out


def _slugify_target(ref: str) -> str:
    """Normalize a link target to a candidate node id (topic-file stem)."""
    ref = (ref or "").strip().strip("[]").strip()
    # Markdown path -> stem; wikilink -> as-is slug.
    if ref.endswith(".md"):
        ref = ref[:-3]
    ref = ref.replace("\\", "/")
    if "/" in ref:
        ref = ref.rsplit("/", 1)[-1]
    return ref.strip()


# --- data model ------------------------------------------------------------


@dataclass(frozen=True)
class Node:
    id: str
    type: str
    title: str = ""
    source: str = ""
    aliases: tuple[str, ...] = ()
    keywords: frozenset[str] = frozenset()

    def label(self) -> str:
        alias = f" ({', '.join(self.aliases)})" if self.aliases else ""
        return f"[{self.type}] {self.id}{alias}"


@dataclass(frozen=True)
class Edge:
    src: str
    dst: str  # canonical id if resolved, else the raw target ref
    type: str
    source: str = ""
    resolved: bool = True

    def as_dict(self) -> dict:
        return {
            "src": self.src,
            "dst": self.dst,
            "type": self.type,
            "source": self.source,
            "resolved": self.resolved,
        }


@dataclass
class _RawEdge:
    src: str
    dst_ref: str
    type: str
    source: str


class MemoryGraph:
    """A typed, bidirectional recall graph over the molt memory corpus."""

    def __init__(self) -> None:
        self._nodes: dict[str, Node] = {}
        self._alias: dict[str, str] = {}  # alias -> canonical id
        self._raw_edges: list[_RawEdge] = []
        self._edges: list[Edge] = []
        self._fwd: dict[str, list[int]] = defaultdict(list)  # id -> resolved-edge idx
        self._bwd: dict[str, list[int]] = defaultdict(list)
        self._dangling: list[Edge] = []
        # Provenance for the integrity check: which M## ids POINTERS.md resolves
        # vs which the MEMORY.md hook index references.
        self._pointers_mids: set[str] = set()
        self._memory_hook_mids: set[str] = set()
        self.memory_dir: Optional[Path] = None
        self.repo_root: Optional[Path] = None
        self.warnings: list[str] = []
        self._finalized = False

    # -- construction -------------------------------------------------------

    def add_node(
        self,
        node_id: str,
        node_type: str,
        *,
        title: str = "",
        source: str = "",
        aliases: Iterable[str] = (),
        keywords: Iterable[str] = (),
    ) -> None:
        node_id = node_id.strip()
        if not node_id:
            return
        aliases = tuple(a for a in aliases if a and a != node_id)
        kw = frozenset(keywords)
        existing = self._nodes.get(node_id)
        if existing is not None:
            # Merge: union aliases + keywords; keep first non-empty title/source;
            # never silently change the type once set (report a conflict).
            new_type = existing.type
            if existing.type != node_type:
                # memory/lane/ledger/etc. collisions: keep the more specific,
                # non-"tool" type; record the conflict for the integrity check.
                self.warnings.append(
                    f"node-type-conflict:{node_id}:{existing.type}!={node_type}"
                )
            merged = Node(
                id=node_id,
                type=new_type,
                title=existing.title or title,
                source=existing.source or source,
                aliases=tuple(sorted(set(existing.aliases) | set(aliases))),
                keywords=existing.keywords | kw,
            )
            self._nodes[node_id] = merged
        else:
            self._nodes[node_id] = Node(
                id=node_id,
                type=node_type,
                title=title,
                source=source,
                aliases=tuple(sorted(set(aliases))),
                keywords=kw,
            )
        for a in aliases:
            # First writer wins for an alias; conflicts are recorded.
            if a in self._alias and self._alias[a] != node_id:
                self.warnings.append(f"alias-conflict:{a}:{self._alias[a]}!={node_id}")
                continue
            self._alias[a] = node_id
        self._finalized = False

    def add_edge(self, src_id: str, dst_ref: str, edge_type: str, source: str) -> None:
        src_id = (src_id or "").strip()
        dst_ref = (dst_ref or "").strip()
        if not src_id or not dst_ref or edge_type not in EDGE_TYPES:
            return
        self._raw_edges.append(_RawEdge(src_id, dst_ref, edge_type, source))
        self._finalized = False

    # -- resolution ---------------------------------------------------------

    def resolve(self, ref: Optional[str]) -> Optional[str]:
        """Map a reference (canonical id, alias, wikilink, ``slug.md``) -> id."""
        if not ref:
            return None
        ref = ref.strip()
        if ref in self._nodes:
            return ref
        if ref in self._alias:
            return self._alias[ref]
        slug = _slugify_target(ref)
        if slug in self._nodes:
            return slug
        if slug in self._alias:
            return self._alias[slug]
        # M## with padding differences (M5 vs M05) -- normalize to two digits.
        m = re.fullmatch(r"[Mm](\d{1,3})", ref)
        if m:
            padded = f"M{int(m.group(1)):02d}"
            if padded in self._nodes:
                return padded
            if padded in self._alias:
                return self._alias[padded]
        return None

    def finalize(self) -> "MemoryGraph":
        """Resolve raw edges into forward/backward adjacency + dangling list."""
        self._edges = []
        self._fwd = defaultdict(list)
        self._bwd = defaultdict(list)
        self._dangling = []
        seen: set[tuple[str, str, str]] = set()
        for raw in self._raw_edges:
            src = self.resolve(raw.src) or raw.src
            dst_id = self.resolve(raw.dst_ref)
            if dst_id is None:
                # Dangling forward-reference: ALLOWED (a note "worth writing
                # later"); reported, never fatal.
                edge = Edge(src, raw.dst_ref, raw.type, raw.source, resolved=False)
                self._dangling.append(edge)
                continue
            key = (src, dst_id, raw.type)
            if key in seen:
                continue
            seen.add(key)
            idx = len(self._edges)
            edge = Edge(src, dst_id, raw.type, raw.source, resolved=True)
            self._edges.append(edge)
            self._fwd[src].append(idx)
            self._bwd[dst_id].append(idx)
        self._finalized = True
        return self

    def _ensure_final(self) -> None:
        if not self._finalized:
            self.finalize()

    # -- accessors ----------------------------------------------------------

    @property
    def nodes(self) -> dict[str, Node]:
        return dict(self._nodes)

    @property
    def edges(self) -> list[Edge]:
        self._ensure_final()
        return list(self._edges)

    def node(self, ref: str) -> Optional[Node]:
        cid = self.resolve(ref)
        return self._nodes.get(cid) if cid else None

    def nodes_of_type(self, node_type: str) -> list[Node]:
        return sorted(
            (n for n in self._nodes.values() if n.type == node_type),
            key=lambda n: n.id,
        )

    def counts(self) -> dict[str, int]:
        self._ensure_final()
        by_node: dict[str, int] = defaultdict(int)
        for n in self._nodes.values():
            by_node[n.type] += 1
        by_edge: dict[str, int] = defaultdict(int)
        for e in self._edges:
            by_edge[e.type] += 1
        return {
            "nodes_total": len(self._nodes),
            "edges_total": len(self._edges),
            "dangling_total": len(self._dangling),
            "nodes_by_type": dict(sorted(by_node.items())),
            "edges_by_type": dict(sorted(by_edge.items())),
        }

    # -- recall queries -----------------------------------------------------

    def neighbors(
        self, ref: str, *, edge_types: Optional[Iterable[str]] = None
    ) -> list[tuple[Node, Edge, str]]:
        """Adjacent nodes in BOTH directions. Returns (node, edge, direction)."""
        self._ensure_final()
        cid = self.resolve(ref)
        if cid is None:
            return []
        want = set(edge_types) if edge_types else None
        out: list[tuple[Node, Edge, str]] = []
        seen: set[tuple[str, str, str]] = set()
        for idx in self._fwd.get(cid, []):
            e = self._edges[idx]
            if want and e.type not in want:
                continue
            dst = self._nodes.get(e.dst)
            if dst and (key := (e.dst, e.type, "out")) not in seen:
                seen.add(key)
                out.append((dst, e, "out"))
        for idx in self._bwd.get(cid, []):
            e = self._edges[idx]
            if want and e.type not in want:
                continue
            src = self._nodes.get(e.src)
            if src and (key := (e.src, e.type, "in")) not in seen:
                seen.add(key)
                out.append((src, e, "in"))
        out.sort(key=lambda t: (t[2], t[1].type, t[0].id))
        return out

    def _typed_targets(self, ref: str, edge_type: str, direction: str) -> list[Node]:
        self._ensure_final()
        cid = self.resolve(ref)
        if cid is None:
            return []
        idxs = self._fwd.get(cid, []) if direction == "out" else self._bwd.get(cid, [])
        seen: set[str] = set()
        out: list[Node] = []
        for idx in idxs:
            e = self._edges[idx]
            if e.type != edge_type:
                continue
            other = e.dst if direction == "out" else e.src
            if other in seen:
                continue
            n = self._nodes.get(other)
            if n:
                seen.add(other)
                out.append(n)
        out.sort(key=lambda n: n.id)
        return out

    def supersedes(self, ref: str) -> list[Node]:
        """Nodes this one SUPERSEDES (forward supersedes edges)."""
        return self._typed_targets(ref, "supersedes", "out")

    def superseded_by(self, ref: str) -> list[Node]:
        """Nodes that supersede this one (backward supersedes edges)."""
        return self._typed_targets(ref, "supersedes", "in")

    def what_consumes(self, finding_ref: str) -> list[Node]:
        """Nodes that CONSUME the finding (backward consumes edges into it)."""
        return self._typed_targets(finding_ref, "consumes", "in")

    def what_produces(self, finding_ref: str) -> list[Node]:
        """Nodes that PRODUCE the finding (backward produces edges into it)."""
        return self._typed_targets(finding_ref, "produces", "in")

    def dangling_links(self) -> list[Edge]:
        """Forward-references whose target has no node (reported, not fatal)."""
        self._ensure_final()
        return list(self._dangling)

    def unresolved_mrefs(self) -> list[str]:
        """M## ids referenced by the MEMORY.md hook index that POINTERS.md does
        NOT resolve (no row at all). POINTERS listing an ``(inline note)`` with
        no file (e.g. M28) still COUNTS as resolved. Empty when the index is
        internally consistent."""
        return sorted(self._memory_hook_mids - self._pointers_mids)

    def nearest(
        self, context: str, k: int = 3, *, node_type: str = "memory"
    ) -> list[tuple[Node, float]]:
        """Rank nodes (default: memories) by relevance to a lane/goal context.

        Scoring blends: keyword overlap (Jaccard-ish), a boost when the context
        directly names the node (id / alias / slug), and a boost when the context
        shares a wikilink target with the node. Deterministic tie-break by id.
        """
        self._ensure_final()
        ctx = context or ""
        # If the context is a readable file path, use its contents.
        try:
            p = Path(ctx)
            if len(ctx) < 400 and p.is_file():
                ctx = ctx + "\n" + p.read_text(encoding="utf-8", errors="replace")
        except Exception:
            pass
        ctx_tokens = _tokenize(ctx)
        ctx_lower = ctx.lower()
        # Direct references named in the context (resolved to canonical ids).
        named: set[str] = set()
        for m in _WIKILINK_RE.finditer(ctx):
            r = self.resolve(m.group(1))
            if r:
                named.add(r)
        for m in _MREF_RE.finditer(ctx):
            r = self.resolve(f"M{int(m.group(1)):02d}")
            if r:
                named.add(r)
        # Context's own wikilink targets (for shared-link scoring).
        ctx_links = {self.resolve(m.group(1)) for m in _WIKILINK_RE.finditer(ctx)}
        ctx_links.discard(None)

        scored: list[tuple[float, str, Node]] = []
        for node in self._nodes.values():
            if node.type != node_type:
                continue
            score = 0.0
            if node.keywords and ctx_tokens:
                overlap = len(node.keywords & ctx_tokens)
                if overlap:
                    denom = float(len(ctx_tokens) + len(node.keywords) - overlap) or 1.0
                    score += overlap + 3.0 * (overlap / denom)
            # Direct naming is the strongest signal.
            if node.id in named:
                score += 12.0
            else:
                # Slug or alias appearing verbatim in the context text.
                if node.id.lower() in ctx_lower:
                    score += 6.0
                elif any(a.lower() in ctx_lower for a in node.aliases):
                    score += 4.0
            # Shared wikilink targets: the node links to something the context
            # also links to (reconstructed-subgraph affinity).
            if ctx_links:
                node_link_targets = {
                    self._edges[i].dst
                    for i in self._fwd.get(node.id, [])
                    if self._edges[i].type == "links"
                }
                shared = node_link_targets & ctx_links
                if shared:
                    score += 2.0 * len(shared)
            if score > 0:
                scored.append((score, node.id, node))
        scored.sort(key=lambda t: (-t[0], t[1]))
        return [(n, round(s, 3)) for s, _, n in scored[: max(0, k)]]

    # -- obsidian export ----------------------------------------------------

    def obsidian_export(self) -> dict[str, str]:
        """Re-emit each node as a markdown note whose SYNTHESIZED edges are real
        ``[[wikilinks]]`` -- so the graph is re-openable in Obsidian and the
        derived produces/consumes/supersedes/cites become first-class links.

        Returns ``{filename: content}``; the caller decides where to write.
        """
        self._ensure_final()
        by_src: dict[str, list[Edge]] = defaultdict(list)
        for e in self._edges:
            by_src[e.src].append(e)
        for e in self._dangling:
            by_src[e.src].append(e)
        out: dict[str, str] = {}
        for node in sorted(self._nodes.values(), key=lambda n: n.id):
            lines = [f"# {node.id}", ""]
            lines.append(f"- type:: {node.type}")
            if node.title:
                lines.append(f"- title:: {node.title}")
            if node.aliases:
                lines.append(f"- aliases:: {', '.join(node.aliases)}")
            if node.source:
                lines.append(f"- source:: {node.source}")
            lines.append("")
            grouped: dict[str, list[Edge]] = defaultdict(list)
            for e in by_src.get(node.id, []):
                grouped[e.type].append(e)
            for etype in sorted(grouped):
                targets = sorted({e.dst for e in grouped[etype]})
                rendered = " ".join(f"[[{t}]]" for t in targets)
                lines.append(f"- {etype}:: {rendered}")
            out[f"{_safe_filename(node.id)}.md"] = "\n".join(lines) + "\n"
        return out


def _safe_filename(node_id: str) -> str:
    return re.sub(r"[^A-Za-z0-9._-]+", "_", node_id)


# --- corpus discovery ------------------------------------------------------


def _default_repo_root() -> Path:
    return Path(__file__).resolve().parents[1]


def _claude_memory_key(path_like: str) -> str:
    """Mangle an absolute project path into the Claude auto-memory dir key.

    ``C:\\Users\\a\\OneDrive\\Documents\\molt`` -> ``C--Users-a-OneDrive-Documents-molt``.
    Drive-colon and every separator become a single dash, matching how Claude
    Code names ``~/.claude/projects/<key>/``.
    """
    s = str(path_like)
    s = s.replace(":", "-")
    s = s.replace("\\", "-").replace("/", "-")
    return s


def discover_memory_dir(
    *, repo_root: Optional[Path] = None, cwd: Optional[str] = None
) -> Optional[Path]:
    """Locate the live memory corpus. Order:

    1. ``$MOLT_MEMORY_DIR`` (explicit override).
    2. ``<repo>/memory/`` if it holds ``MEMORY.md``.
    3. The Claude auto-memory dir for this project (exact key from cwd, then a
       ``*molt*`` glob preferring the richest / freshest ``MEMORY.md``).

    Returns ``None`` (never raises) when no corpus is found -- callers degrade to
    an empty graph, they do not crash.
    """
    # 1. explicit override
    env = os.environ.get("MOLT_MEMORY_DIR")
    if env:
        p = Path(env)
        if (p / "MEMORY.md").is_file():
            return p
        if p.is_dir():
            return p
    # 2. in-repo memory/
    root = repo_root or _default_repo_root()
    cand = root / "memory"
    if (cand / "MEMORY.md").is_file():
        return cand
    # 3. Claude auto-memory
    try:
        projects = Path.home() / ".claude" / "projects"
        if projects.is_dir():
            # Exact key from cwd or the repo root.
            for base in filter(None, (cwd, str(root))):
                key = _claude_memory_key(base)
                exact = projects / key / "memory"
                if (exact / "MEMORY.md").is_file():
                    return exact
            # Glob fallback: any *molt* project with a MEMORY.md, richest first.
            best: Optional[tuple[int, float, Path]] = None
            for mem in projects.glob("*molt*/memory"):
                mpath = mem / "MEMORY.md"
                if not mpath.is_file():
                    continue
                try:
                    ntopics = sum(1 for _ in mem.glob("*.md"))
                    mtime = mpath.stat().st_mtime
                except Exception:
                    continue
                cand_key = (ntopics, mtime, mem)
                if best is None or cand_key[:2] > best[:2]:
                    best = cand_key
            if best is not None:
                return best[2]
    except Exception:
        pass
    return None


# --- source parsers (each guarded; failures append a warning) --------------


def _read(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8", errors="replace")
    except Exception:
        return ""


def _iter_code_lines(text: str) -> Iterable[tuple[int, str]]:
    """Yield (lineno, line) skipping fenced code blocks (examples, not asserts)."""
    in_fence = False
    for i, line in enumerate(text.splitlines(), start=1):
        s = line.lstrip()
        if s.startswith("```") or s.startswith("~~~"):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        yield i, line


def _parse_pointers(graph: MemoryGraph, memory_dir: Path) -> None:
    """POINTERS.md: the M## -> topic-file resolver. Registers M## aliases onto
    topic-slug nodes (or hook-only M## nodes when no file is mapped)."""
    text = _read(memory_dir / "POINTERS.md")
    if not text:
        return
    row_re = re.compile(r"^\|\s*(M\d{2,3})\s*\|\s*([^|]*?)\s*\|\s*([^|]*?)\s*\|")
    for line in text.splitlines():
        m = row_re.match(line.strip())
        if not m:
            continue
        mid, topic, filecell = m.group(1), m.group(2).strip(), m.group(3).strip()
        graph._pointers_mids.add(mid)
        link = _MD_LINK_RE.search(filecell + ")")
        slug = ""
        if link:
            slug = _slugify_target(link.group(1))
        if slug:
            # Topic-file-backed directive: the node id is the slug; M## is alias.
            graph.add_node(
                slug,
                "memory",
                title=topic,
                aliases=(mid,),
                keywords=_tokenize(topic),
            )
        else:
            # Inline / file-less directive (e.g. M28): node id IS the M##.
            graph.add_node(mid, "memory", title=topic, keywords=_tokenize(topic))


def _parse_memory_index(graph: MemoryGraph, memory_dir: Path) -> None:
    """MEMORY.md: one hook line per M##. Defines/annotates M## nodes and adds
    cross-cite / See-link / wikilink edges from the hook text."""
    text = _read(memory_dir / "MEMORY.md")
    if not text:
        return
    hook_re = re.compile(r"^[-*]\s*(M\d{2,3})\s*[—:-]\s*(.+)$")
    for _, line in _iter_code_lines(text):
        m = hook_re.match(line.strip())
        if not m:
            continue
        mid, body = m.group(1), m.group(2)
        graph._memory_hook_mids.add(mid)
        # Ensure the node exists (alias-only if POINTERS already made the slug).
        if graph.resolve(mid) is None:
            graph.add_node(mid, "memory", title=body[:120], keywords=_tokenize(body))
        # Cross-cites to OTHER directives named in the hook body.
        for mm in _MREF_RE.finditer(body):
            other = f"M{int(mm.group(1)):02d}"
            if other != mid:
                graph.add_edge(mid, other, "cites", "MEMORY.md")
        # ``See <slug>`` trailing references + inline wikilinks/markdown links.
        for sm in _SEE_REF_RE.finditer(body):
            graph.add_edge(mid, sm.group(1), "links", "MEMORY.md")
        for wm in _WIKILINK_RE.finditer(body):
            graph.add_edge(mid, wm.group(1), "links", "MEMORY.md")
        for md in _MD_LINK_RE.finditer(body):
            graph.add_edge(mid, _slugify_target(md.group(1)), "links", "MEMORY.md")
        if _SUPERSEDE_RE.search(body):
            for tgt in _supersede_targets(body):
                if graph.resolve(tgt) != mid:
                    graph.add_edge(mid, tgt, "supersedes", "MEMORY.md")


def _parse_topic_files(graph: MemoryGraph, memory_dir: Path) -> None:
    """Each memory/*.md topic file is a memory node; extract wikilinks (links),
    M## / finding_id / tool references (cites), supersedes, and produces."""
    for path in sorted(memory_dir.rglob("*.md")):
        stem = path.stem
        if stem in ("MEMORY", "POINTERS"):
            continue
        text = _read(path)
        title = ""
        for line in text.splitlines():
            if line.startswith("# "):
                title = line[2:].strip()
                break
        relpath = path.relative_to(memory_dir).as_posix()
        node_id = stem if path.parent == memory_dir else f"memory:{relpath[:-3]}"
        graph.add_node(
            node_id,
            "memory",
            title=title or stem,
            source=f"memory/{relpath}",
            keywords=_tokenize(title + "\n" + text),
        )
        for lineno, line in _iter_code_lines(text):
            src = f"memory/{relpath}:{lineno}"
            for wm in _WIKILINK_RE.finditer(line):
                tgt = wm.group(1).strip()
                if _slugify_target(tgt) != node_id:
                    graph.add_edge(node_id, tgt, "links", src)
            for md in _MD_LINK_RE.finditer(line):
                tgt = _slugify_target(md.group(1))
                if tgt and tgt != node_id:
                    graph.add_edge(node_id, tgt, "links", src)
            for mm in _MREF_RE.finditer(line):
                graph.add_edge(node_id, f"M{int(mm.group(1)):02d}", "cites", src)
            for fm in _FINDING_ID_RE.finditer(line):
                graph.add_edge(node_id, fm.group(0), "cites", src)
            for tm in _TOOL_PATH_RE.finditer(line):
                tool = tm.group(0)
                graph.add_node(tool, "tool", title=Path(tool).name, source="")
                if _LANDED_RE.search(line):
                    graph.add_edge(node_id, tool, "produces", src)
                else:
                    graph.add_edge(node_id, tool, "cites", src)
            if _SUPERSEDE_RE.search(line):
                for tgt in _supersede_targets(line):
                    if _slugify_target(tgt) != node_id:
                        graph.add_edge(node_id, tgt, "supersedes", src)
            dm = _DECISION_RE.match(line.strip())
            if dm:
                did = f"decision:{node_id}:{lineno}"
                graph.add_node(
                    did,
                    "decision",
                    title=dm.group(2)[:120],
                    source=src,
                    keywords=_tokenize(dm.group(2)),
                )
                graph.add_edge(node_id, did, "cites", src)


def _supersede_targets(line: str) -> list[str]:
    """Wikilink / markdown-link / M## targets named on a supersede line."""
    out: list[str] = []
    for wm in _WIKILINK_RE.finditer(line):
        out.append(wm.group(1).strip())
    for md in _MD_LINK_RE.finditer(line):
        out.append(_slugify_target(md.group(1)))
    for mm in _MREF_RE.finditer(line):
        out.append(f"M{int(mm.group(1)):02d}")
    return out


def _parse_ledgers(graph: MemoryGraph, docs_agent: Path) -> None:
    """docs/agent/*.md ledger files -> ledger nodes; CLAIMS.md rows -> lane
    nodes with cites/produces edges to tools named in their descriptions."""
    if not docs_agent.is_dir():
        return
    for path in sorted(docs_agent.glob("*.md")):
        stem = path.stem
        ledger_id = f"ledger:{stem}"
        text = _read(path)
        title = ""
        for line in text.splitlines():
            if line.startswith("# "):
                title = line[2:].strip()
                break
        graph.add_node(
            ledger_id,
            "ledger",
            title=title or stem,
            source=f"docs/agent/{path.name}",
            keywords=_tokenize(title),
        )
        if stem == "CLAIMS":
            _parse_claims_rows(graph, text)
        # Decisions + supersessions inside any ledger.
        for lineno, line in _iter_code_lines(text):
            dm = _DECISION_RE.match(line.strip())
            if dm:
                did = f"decision:{stem}:{lineno}"
                graph.add_node(
                    did,
                    "decision",
                    title=dm.group(2)[:120],
                    source=f"docs/agent/{path.name}:{lineno}",
                    keywords=_tokenize(dm.group(2)),
                )
                graph.add_edge(ledger_id, did, "cites", f"docs/agent/{path.name}")
            if _SUPERSEDE_RE.search(line):
                for tgt in _supersede_targets(line):
                    if graph.resolve(tgt) != ledger_id:
                        graph.add_edge(
                            ledger_id, tgt, "supersedes", f"docs/agent/{path.name}"
                        )


def _parse_claims_rows(graph: MemoryGraph, text: str) -> None:
    """CLAIMS.md pipe rows: ``| LANE | branch | ts | STATE | desc |``."""
    for line in text.splitlines():
        s = line.strip()
        if not s.startswith("|"):
            continue
        cells = [c.strip() for c in s.strip("|").split("|")]
        if len(cells) < 5:
            continue
        lane = cells[0]
        if not re.match(r"^[A-Z][A-Z0-9-]{2,}$", lane):
            continue  # header / separator / prose rows
        desc = cells[-1]
        lane_id = f"lane:{lane}"
        graph.add_node(
            lane_id,
            "lane",
            title=lane,
            aliases=(lane,),
            source="docs/agent/CLAIMS.md",
            keywords=_tokenize(desc),
        )
        for tm in _TOOL_PATH_RE.finditer(desc):
            tool = tm.group(0)
            graph.add_node(tool, "tool", title=Path(tool).name)
            edge = "produces" if _LANDED_RE.search(desc) else "cites"
            graph.add_edge(lane_id, tool, edge, "docs/agent/CLAIMS.md")
        for fm in _FINDING_ID_RE.finditer(desc):
            graph.add_edge(lane_id, fm.group(0), "cites", "docs/agent/CLAIMS.md")


def _parse_findings(graph: MemoryGraph, repo_root: Path) -> None:
    """The A4 findings registry: finding nodes + producer/consumer edges.

    A producer (tool/commit/gate that emits the anchors) --produces--> finding;
    a consumer (tool/gate/doc that reads it) --consumes--> finding. This is the
    structured backbone of ``what-consumes``."""
    try:
        if str(repo_root) not in sys.path:
            sys.path.insert(0, str(repo_root))
        from tools.findings_registry import query_findings  # type: ignore
    except Exception as exc:  # registry absent/broken must not sink the graph
        graph.warnings.append(f"findings-registry-unavailable:{type(exc).__name__}")
        return
    try:
        findings = query_findings()
    except Exception as exc:
        graph.warnings.append(f"findings-query-failed:{type(exc).__name__}")
        return
    for f in findings:
        try:
            kw = _tokenize(f.one_line_summary + " " + f.claim)
            graph.add_node(
                f.finding_id,
                "finding",
                title=f.one_line_summary,
                source=".molt/state/findings_registry.jsonl",
                keywords=kw,
            )
            for prod in getattr(f, "producers", ()):  # producer -> produces -> f
                node_id = _resolve_or_make_agent(graph, prod)
                graph.add_edge(node_id, f.finding_id, "produces", "findings_registry")
            for cons in getattr(f, "consumers", ()):  # consumer -> consumes -> f
                node_id = _resolve_or_make_agent(graph, cons)
                graph.add_edge(node_id, f.finding_id, "consumes", "findings_registry")
        except Exception:
            continue


_COMMIT_RE = re.compile(r"^commit[:\s]+([0-9a-f]{7,40})\b", re.IGNORECASE)


def _resolve_or_make_agent(graph: MemoryGraph, name: str) -> str:
    """Resolve a finding producer/consumer string to an EXISTING node where
    possible (so ``memory/M47`` becomes an edge on the real M47 memory node,
    not a synthetic duplicate), else create the best-typed node and return it."""
    name = (name or "").strip()
    # Already a node / alias / slug?
    r = graph.resolve(name)
    if r:
        return r
    # A commit reference -> a decision node (a landed change record).
    cm = _COMMIT_RE.match(name)
    if cm:
        cid = f"commit:{cm.group(1)}"
        graph.add_node(cid, "decision", title=name)
        return cid
    # memory/<slug-or-Mxx> -> resolve onto the memory node (make one if unknown).
    if name.startswith("memory/"):
        inner = name.split("/", 1)[1]
        if inner.endswith(".md"):
            inner = inner[:-3]
        r = graph.resolve(inner)
        if r:
            return r
        graph.add_node(inner, "memory", title=name)
        return inner
    # docs/... or any .md -> a ledger node keyed by stem.
    if name.startswith("docs/") or name.endswith(".md"):
        stem = Path(name).stem
        r = graph.resolve(f"ledger:{stem}") or graph.resolve(stem)
        if r:
            return r
        graph.add_node(f"ledger:{stem}", "ledger", title=name)
        return f"ledger:{stem}"
    # A tool path.
    if name.startswith("tools/") or (name.endswith(".py") and "/" in name):
        graph.add_node(name, "tool", title=Path(name).name)
        return name
    # A gate-ish bare identifier.
    if re.fullmatch(r"[a-z0-9][a-z0-9_-]+", name) and (
        "gate" in name or "check" in name
    ):
        graph.add_node(name, "gate", title=name)
        return name
    graph.add_node(name, "tool", title=name)
    return name


def _parse_gates(graph: MemoryGraph, repo_root: Path) -> None:
    """molt_dev_gates.toml: [[rule]] + [[gate_flip]] names -> gate nodes."""
    try:
        import tomllib  # 3.11+
    except Exception:
        graph.warnings.append("tomllib-unavailable:gate-nodes-skipped")
        return
    path = repo_root / "tools" / "molt_dev_gates.toml"
    if not path.is_file():
        return
    try:
        data = tomllib.loads(_read(path))
    except Exception as exc:
        graph.warnings.append(f"gates-toml-parse-failed:{type(exc).__name__}")
        return
    for rule in data.get("rule", []) or []:
        name = str(rule.get("name", "")).strip()
        if name:
            graph.add_node(
                name,
                "gate",
                title=str(rule.get("description", ""))[:120],
                source="tools/molt_dev_gates.toml",
                keywords=_tokenize(str(rule.get("description", ""))),
            )
    for flip in data.get("gate_flip", []) or []:
        name = str(flip.get("name", "")).strip()
        if name:
            graph.add_node(
                name,
                "gate",
                title=str(flip.get("rationale", ""))[:120],
                source="tools/molt_dev_gates.toml",
                keywords=_tokenize(str(flip.get("rationale", ""))),
            )


# --- top-level builders ----------------------------------------------------


def build_graph(
    *,
    memory_dir: Optional[Path] = None,
    repo_root: Optional[Path] = None,
    cwd: Optional[str] = None,
    include_memory: bool = True,
    include_ledgers: bool = True,
    include_findings: bool = True,
    include_gates: bool = True,
) -> MemoryGraph:
    """Build the typed recall graph from the live corpus. Never raises."""
    graph = MemoryGraph()
    root = repo_root or _default_repo_root()
    graph.repo_root = root
    mdir = memory_dir or discover_memory_dir(repo_root=root, cwd=cwd)
    graph.memory_dir = mdir
    if mdir is None:
        graph.warnings.append("memory-dir-not-found")
    if include_memory and mdir is not None:
        for parser in (_parse_pointers, _parse_memory_index, _parse_topic_files):
            try:
                parser(graph, mdir)
            except Exception as exc:
                graph.warnings.append(f"{parser.__name__}-failed:{type(exc).__name__}")
    if include_ledgers:
        try:
            _parse_ledgers(graph, root / "docs" / "agent")
        except Exception as exc:
            graph.warnings.append(f"ledgers-failed:{type(exc).__name__}")
    if include_findings:
        try:
            _parse_findings(graph, root)
        except Exception as exc:
            graph.warnings.append(f"findings-failed:{type(exc).__name__}")
    if include_gates:
        try:
            _parse_gates(graph, root)
        except Exception as exc:
            graph.warnings.append(f"gates-failed:{type(exc).__name__}")
    return graph.finalize()


def nearest_memories(
    context: str,
    k: int = 3,
    *,
    memory_dir: Optional[Path] = None,
    repo_root: Optional[Path] = None,
    cwd: Optional[str] = None,
) -> list[tuple[Node, float]]:
    """Convenience for the SessionStart digest: build a memory-only graph (fast,
    ~60 tiny files) and return the ``k`` nearest memories to ``context``.

    Never raises: any failure yields ``[]`` so the digest still renders."""
    try:
        graph = build_graph(
            memory_dir=memory_dir,
            repo_root=repo_root,
            cwd=cwd,
            include_memory=True,
            include_ledgers=False,
            include_findings=False,
            include_gates=False,
        )
        return graph.nearest(context, k=k)
    except Exception:
        return []


# --- CLI -------------------------------------------------------------------


def _force_utf8() -> None:
    try:
        from tools._io_utf8 import force_utf8_stdio

        force_utf8_stdio()
    except Exception:
        for stream in (sys.stdout, sys.stderr):
            rc = getattr(stream, "reconfigure", None)
            if rc:
                try:
                    rc(encoding="utf-8", errors="backslashreplace")
                except Exception:
                    pass


def _cmd_neighbors(graph: MemoryGraph, args: argparse.Namespace) -> int:
    node = graph.node(args.ref)
    if node is None:
        print(f"no node resolves for {args.ref!r}", file=sys.stderr)
        return 1
    print(f"{node.label()}  {node.title}".rstrip())
    nbrs = graph.neighbors(args.ref, edge_types=args.edge_type or None)
    if not nbrs:
        print("  (no resolved neighbors)")
        return 0
    for n, e, direction in nbrs:
        arrow = "->" if direction == "out" else "<-"
        print(f"  {arrow} [{e.type}] {n.label()}")
    return 0


def _cmd_supersedes(graph: MemoryGraph, args: argparse.Namespace) -> int:
    fwd = graph.supersedes(args.ref)
    bwd = graph.superseded_by(args.ref)
    for n in fwd:
        print(f"  supersedes -> {n.label()}")
    for n in bwd:
        print(f"  superseded-by <- {n.label()}")
    if not fwd and not bwd:
        print("  (no supersedes edges)")
    return 0


def _cmd_what_consumes(graph: MemoryGraph, args: argparse.Namespace) -> int:
    cons = graph.what_consumes(args.ref)
    prod = graph.what_produces(args.ref)
    for n in cons:
        print(f"  consumed-by <- {n.label()}")
    for n in prod:
        print(f"  produced-by <- {n.label()}")
    if not cons and not prod:
        print("  (no producer/consumer edges; unknown or orphan finding)")
    return 0


def _cmd_nearest(graph: MemoryGraph, args: argparse.Namespace) -> int:
    ranked = graph.nearest(args.context, k=args.k, node_type=args.node_type)
    if not ranked:
        print("  (no nearby memories)")
        return 0
    for n, score in ranked:
        title = n.title[:80]
        print(f"  {score:6.2f}  {n.label()}  {title}".rstrip())
    return 0


def _cmd_stats(graph: MemoryGraph, args: argparse.Namespace) -> int:
    c = graph.counts()
    print(json.dumps(c, indent=2))
    if graph.warnings:
        print(f"warnings: {len(graph.warnings)}", file=sys.stderr)
        for w in graph.warnings[:20]:
            print(f"  {w}", file=sys.stderr)
    return 0


def _cmd_dangling(graph: MemoryGraph, args: argparse.Namespace) -> int:
    dangling = graph.dangling_links()
    for e in dangling:
        print(f"  {e.src} -[{e.type}]-> {e.dst}  ({e.source})")
    print(f"dangling: {len(dangling)}")
    unresolved = graph.unresolved_mrefs()
    print(
        f"unresolved M##: {len(unresolved)}"
        + (f" ({', '.join(unresolved)})" if unresolved else "")
    )
    return 0


def _cmd_obsidian(graph: MemoryGraph, args: argparse.Namespace) -> int:
    files = graph.obsidian_export()
    out_dir = Path(args.out)
    out_dir.mkdir(parents=True, exist_ok=True)
    for name, content in files.items():
        (out_dir / name).write_text(content, encoding="utf-8")
    print(f"wrote {len(files)} note(s) to {out_dir}")
    return 0


def main(argv: Optional[list[str]] = None) -> int:
    _force_utf8()
    ap = argparse.ArgumentParser(prog="memory_graph", description=__doc__)
    ap.add_argument("--memory-dir", default=None, help="override corpus dir")
    ap.add_argument("--repo-root", default=None)
    sub = ap.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("neighbors", help="adjacent nodes (both directions)")
    p.add_argument("ref")
    p.add_argument("--edge-type", action="append", choices=sorted(EDGE_TYPES))
    p.set_defaults(func=_cmd_neighbors)

    p = sub.add_parser("supersedes", help="supersedes / superseded-by")
    p.add_argument("ref")
    p.set_defaults(func=_cmd_supersedes)

    p = sub.add_parser("what-consumes", help="producers/consumers of a finding")
    p.add_argument("ref")
    p.set_defaults(func=_cmd_what_consumes)

    p = sub.add_parser("nearest", help="rank memories nearest a lane context")
    p.add_argument("context")
    p.add_argument("-k", type=int, default=3)
    p.add_argument("--node-type", default="memory", choices=sorted(NODE_TYPES))
    p.set_defaults(func=_cmd_nearest)

    p = sub.add_parser("stats", help="node/edge counts")
    p.set_defaults(func=_cmd_stats)

    p = sub.add_parser("dangling", help="dangling links + unresolved M##")
    p.set_defaults(func=_cmd_dangling)

    p = sub.add_parser("obsidian-export", help="re-emit synthesized edges as [[..]]")
    p.add_argument("--out", required=True)
    p.set_defaults(func=_cmd_obsidian)

    args = ap.parse_args(argv)
    graph = build_graph(
        memory_dir=Path(args.memory_dir) if args.memory_dir else None,
        repo_root=Path(args.repo_root) if args.repo_root else None,
    )
    return args.func(graph, args)


if __name__ == "__main__":
    raise SystemExit(main())
