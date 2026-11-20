"""Shared namespace premises for closed Python source graphs (HF-226).

The syntax authority is ``python_namespace_writes``. This module projects those
summaries onto a caller-owned module topology; it never discovers modules or
reads source paths. Tooling discovery and fixed source inventories share this
aggregation, while each graph owner supplies package-child bindings and decides
when its premise has converged.
"""

from __future__ import annotations

from collections.abc import Callable, Collection, Iterable, Mapping
import ast
import re
import symtable
from molt.typed_cache import typed_cache
from dataclasses import dataclass, field, replace
from pathlib import Path
from types import MappingProxyType
from typing import Final

from molt.compiler_analysis.python_identity_catalog_generated import (
    MemberMask,
    NO_INVALID_MEMBERS,
    ALL_INVALID_MEMBERS,
)
from molt.compiler_analysis.python_binding_flow import (
    PythonBindingFlowPolicy,
    python_canonical_member_writes,
)
from molt.compiler_analysis.python_identity_catalog_generated import PythonMember
from molt.compiler_analysis.python_import_redirects import (
    IMPORT_SPEC_ATTRIBUTES,
    NO_IMPORT_REDIRECTS,
    UNKNOWN_IMPORT_REDIRECTS,
    ImportRedirects,
    ImportWriter,
    import_redirects,
)
from molt.compiler_analysis.python_namespace_writes import PythonNamespaceWrite
from molt.compiler_analysis.python_source_keys import PythonSourceKey
from molt.compiler_analysis.python_value_identity import (
    canonical_import_binding_write_mask,
)


# The frame-locals writer as a scan view states it: present, site elided.
_SCAN_FRAME_LOCALS_WRITER: Final = "a frame-locals writer in the graph"


@dataclass(frozen=True, slots=True)
class PythonNamespacePremise:
    """One module's namespace premise, verified over a closed graph (HF-226).

    ``foreign_writes`` lists, sorted, the names graph code outside the module
    writes into its namespace: named writes and submodule bindings; ``None``
    when a written name is computed. ``member_writes`` lists the canonical
    members code anywhere in the graph writes. The binding flow keeps every
    other global and member fact across callbacks. ``namespace_reflected``
    holds when activated graph code may reach the module's namespace mapping
    by identity (a ``reflection`` row naming it or any module), so the mapping
    must hold every global. ``code_rebinding`` holds when activated graph code
    reads a code object (a ``code`` row) or a native extension can: then
    ``FunctionType`` may run any function over another globals mapping, so no
    function's global read is its module's. ``escape_writes`` pairs, sorted,
    each name some code stores as an attribute while the module's value has
    escaped (call_anatomy.md 4.5a) with its first writer site; those names are
    also in ``foreign_writes``.
    """

    foreign_writes: tuple[str, ...] | None
    member_writes: MemberMask
    entry_member_writes: MemberMask
    noncanonical_imports: tuple[str, ...]
    opaque_activation_sources: tuple[PythonSourceKey, ...] | None = None
    namespace_reflected: bool = False
    code_rebinding: bool = False
    escape_writes: tuple[tuple[str, str], ...] = ()
    # The first site that may assign ``__class__`` on an instance anywhere in
    # the graph: a named ``__class__`` store on a receiver that is not a
    # module, a computed-name attribute store, or a native extension. None:
    # no code can change an instance's class, so a constructor's result is
    # exactly its class for its whole lifetime (call_anatomy.md 4.5,
    # "Implementation (field slots)").
    instance_class_writer: str | None = None
    # The first site that may write a running function's locals through a
    # PEP 667 frame proxy: an ``f_locals`` use anywhere in the graph that may
    # write through it or hand it to code that may (a store, a delete, a
    # writing method, a return or an escape; a read is no writer), or a
    # native extension. None: no code can rebind a fast local behind its
    # function's back, so on a 3.13+ target a callback leaves a function's
    # locals in SSA as it does before 3.13 (HF-901).
    frame_locals_writer: str | None = None
    # Pairs (method source, class statement source), sorted: every entry of
    # the method binds its first parameter to an instance that class
    # statement built (`PythonSourceDemands.exact_receivers`), so the method
    # compiles with an exact `self` (design 78, 3.8).
    exact_receivers: tuple[tuple[PythonSourceKey, PythonSourceKey], ...] = ()
    # Set only on a scan view: the names its source can spell. ``foreign_writes``
    # then lists only those, and the binding analysis treats any other name a
    # computed key reaches as foreign-written (HF-926).
    foreign_universe: frozenset[str] | None = None

    def scan_view(
        self, universe: frozenset[str] | None = None
    ) -> PythonNamespacePremise:
        """The premise a source scan reads, for a source naming ``universe``.

        A scan reads the premise only through its binding policy
        (``python_namespace_binding_policy``, not lowering) and the ``__all__``
        export check. Lowering-only facts (``exact_receivers``, escape writer
        sites, namespace reflection, code rebinding, the instance-class writer)
        never revise a scan. With ``universe`` (``python_namespace_name_universe``)
        the foreign names shrink to those the source can spell: the binding
        analysis reaches any other name only through a computed key, and then
        treats it as foreign-written. A write of a name the module never spells
        therefore neither revises nor re-keys its scan (HF-926).
        """
        if self.foreign_universe is not None:
            raise ValueError("a scan view is not a whole premise")
        foreign = self.foreign_writes
        if foreign is None or universe is None:
            universe = None
        else:
            foreign = tuple(name for name in foreign if name in universe)
        return replace(
            self,
            foreign_writes=foreign,
            foreign_universe=universe,
            namespace_reflected=False,
            code_rebinding=False,
            escape_writes=(),
            instance_class_writer=None,
            exact_receivers=(),
            # A scan reads only whether a frame-locals writer exists, never
            # its site, so a site change re-keys no scan.
            frame_locals_writer=None
            if self.frame_locals_writer is None
            else _SCAN_FRAME_LOCALS_WRITER,
        )

    def escape_writer(self, name: str) -> str | None:
        """The first site that may write ``name`` through an escaped value."""
        return dict(self.escape_writes).get(name)


# Module names the binding analysis and export check consult without a
# spelling in the source: loader metadata, builtins and export controls.
_IMPLICIT_MODULE_NAMES = frozenset(
    {
        "__name__",
        "__package__",
        "__spec__",
        "__path__",
        "__file__",
        "__cached__",
        "__loader__",
        "__doc__",
        "__builtins__",
        "__all__",
        "__annotations__",
        "__annotate__",
        "__conditional_annotations__",
        "__dict__",
        "__getattr__",
        "__dir__",
        "__class__",
        "__debug__",
    }
)
_IDENTIFIER = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")


def _module_scope_names(table: symtable.SymbolTable) -> set[str]:
    """Names that resolve in the module namespace from any scope: every
    module-level symbol, and each nested scope's global references."""
    names = {symbol.get_name() for symbol in table.get_symbols()}
    pending = list(table.get_children())
    while pending:
        child = pending.pop()
        names.update(
            symbol.get_name()
            for symbol in child.get_symbols()
            if symbol.is_global() or symbol.is_declared_global()
        )
        pending.extend(child.get_children())
    return names


@typed_cache(maxsize=512)
def _name_universe(text: str) -> frozenset[str] | None:
    try:
        table = symtable.symtable(text, "<module>", "exec")
        tree = ast.parse(text)
    except (SyntaxError, ValueError):
        return None
    names = _module_scope_names(table) | _IMPLICIT_MODULE_NAMES
    for node in ast.walk(tree):
        if isinstance(node, ast.Constant) and isinstance(node.value, str):
            # Literal keys, __all__ rows and string annotations name globals
            # by value; escapes are already decoded here.
            names.update(_IDENTIFIER.findall(node.value))
        annotation = (
            node.annotation
            if isinstance(node, (ast.arg, ast.AnnAssign))
            else node.returns
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
            else None
        )
        if annotation is not None:
            # PEP 563 annotations are never compiled, so the symbol table
            # omits their names; annotation introspection still reads them.
            names.update(
                name.id for name in ast.walk(annotation) if isinstance(name, ast.Name)
            )
    return frozenset(names)


def python_namespace_name_universe(text: str | None) -> frozenset[str] | None:
    """Every name a module's own code can reach in its namespace by spelling.

    The module's symbols and the global references of its nested scopes
    (function locals and parameters excluded), identifiers in its string
    literals, its annotation names, and the module dunders the analysis
    consults unspelled. A name outside it reaches the namespace only through a
    computed key, which the binding analysis treats as foreign-written under
    a scan view. None (no projection) when the text is unavailable or does not
    compile.
    """
    if text is None:
        return None
    return _name_universe(text)


def python_namespace_binding_policy[P: PythonBindingFlowPolicy](
    policy: P, premise: PythonNamespacePremise | None, *, lowering: bool = False
) -> P:
    """Apply a closed graph's initial and future uncertainty together.

    A reached writer may already have run before the first acquisition. Its
    entry mask must travel with callback writes through every graph consumer.
    Standalone callers keep their explicit policy when no graph was proved.

    Exposure lets unknown code activate a definition; only a code rebinder
    (``code_rebinding``) can make that activation read another globals
    mapping. Lowering therefore sees the exposed set only when the graph holds
    a rebinder, so a method's global and builtin reads keep their proofs.
    Graph discovery keeps the exposed set: its demand worklist activates the
    bodies a value with a known origin can reach, and a precise origin for a
    whole module's escape would activate every function of that module (the
    runtime closure's escape of ``types`` pulls in 17 modules).
    """
    if premise is None:
        return policy
    opaque = premise.opaque_activation_sources
    if lowering and opaque is not None and not premise.code_rebinding:
        opaque = ()
    return replace(
        policy,
        foreign_namespace_writes=premise.foreign_writes,
        foreign_namespace_universe=premise.foreign_universe,
        callback_member_writes=premise.member_writes,
        entry_invalid_members=premise.entry_member_writes,
        noncanonical_imports=premise.noncanonical_imports,
        opaque_activation_sources=opaque,
        frame_locals_writer=premise.frame_locals_writer,
    )


@dataclass(frozen=True, slots=True)
class PythonGraphNamespaceWrites:
    """Named namespace writes found in one walk of a closed graph."""

    # Target source -> names written into it; None when a name is computed.
    by_source: Mapping[Path, frozenset[str] | None]
    # Names written into a namespace reached by a computed ``sys.modules`` key.
    any_module: frozenset[str] | None
    members: MemberMask
    entry_members: MemberMask
    # Sources whose namespace mapping a reflection row reaches; None when one
    # reaches any module's (a computed key, ``__globals__``, ``f_globals``).
    reflected: frozenset[Path] | None = frozenset()
    # Whether some activated code reads a code object (a ``code`` row).
    code_rebinding: bool = False
    # Named attribute stores on receivers that are not module references,
    # each with its first writer site (``member_store`` rows).
    member_stores: Mapping[str, str] = field(default_factory=dict)
    # Sources whose module value escapes; None when a computed key's does.
    escaped: frozenset[Path] | None = frozenset()
    # The first computed-name attribute store on a receiver that is not a
    # module (``computed_member_store`` rows).
    computed_member_store: str | None = None
    # Who can change what an import yields (bedrock §13, D1): one graph fact.
    import_redirects: ImportRedirects = NO_IMPORT_REDIRECTS
    # The first ``frame_locals`` row's site (a PEP 667 frame proxy use that
    # may write).
    frame_locals_writer: str | None = None
    # Named attribute stores on a local proven to hold a function or class
    # object (``local_definition_store`` rows): module writes only while a
    # frame-locals writer could rebind that local (``_module_member_stores``).
    local_definition_stores: Mapping[str, str] = field(default_factory=dict)

    def premise(
        self,
        source: Path,
        children: frozenset[str],
        *,
        noncanonical_imports: tuple[str, ...] = (),
        opaque_activation_sources: tuple[PythonSourceKey, ...] | None = None,
        native_code: bool = False,
    ) -> PythonNamespacePremise:
        return _namespace_premise(
            self.by_source.get(source, frozenset()),
            self.any_module,
            self.members,
            self.entry_members,
            children,
            _redirected_canonical(noncanonical_imports, self.import_redirects),
            opaque_activation_sources,
            self.reflected is None or source in self.reflected,
            self.code_rebinding or native_code,
            _escape_writes(
                source,
                self.escaped,
                _module_member_stores(
                    self.member_stores,
                    self.local_definition_stores,
                    _frame_locals_writer(self.frame_locals_writer, native_code),
                ),
            ),
            _instance_class_writer(
                self.member_stores, self.computed_member_store, native_code
            ),
            _frame_locals_writer(self.frame_locals_writer, native_code),
        )


def _redirected_canonical(
    noncanonical_imports: tuple[str, ...], redirects: ImportRedirects
) -> tuple[str, ...]:
    """A canonical provider some writer redirects is not canonical (M7)."""
    redirected = redirects.canonical_redirected()
    if not redirected:
        return noncanonical_imports
    return tuple(sorted({*noncanonical_imports, *redirected}))


def _instance_class_writer(
    stores: Mapping[str, str], computed: str | None, native_code: bool
) -> str | None:
    if "__class__" in stores:
        return stores["__class__"]
    if computed is not None:
        return computed
    return "a native extension module" if native_code else None


def _frame_locals_writer(site: str | None, native_code: bool) -> str | None:
    if site is not None:
        return site
    return "a native extension module" if native_code else None


def _site_order(site: str) -> tuple[str, int]:
    label, _, line = site.rpartition(":")
    return (label, int(line)) if line.isdigit() else (site, 0)


def _earlier_site(known: str | None, site: str | None) -> bool:
    """Whether ``site`` is a first writer site to keep over ``known``."""
    return site is not None and (
        known is None or _site_order(site) < _site_order(known)
    )


def _earlier_sites(
    known: Mapping[str, str], batch: Mapping[str, str]
) -> dict[str, str]:
    """Each store name's first writer site by (module, line), whatever order
    discovery admits the writers in: a new name, or an earlier site."""
    return {
        name: site
        for name, site in batch.items()
        if _earlier_site(known.get(name), site)
    }


def _module_member_stores(
    stores: Mapping[str, str],
    local_definitions: Mapping[str, str],
    frame_locals_writer: str | None,
) -> Mapping[str, str]:
    """The named stores that may write an escaped module's namespace.

    A store on a local that holds only function or class objects writes no
    module unless a frame-locals writer can rebind that local to one (HF-926).
    """
    if frame_locals_writer is None or not local_definitions:
        return stores
    return {**local_definitions, **stores}


def _escape_writes(
    source: Path, escaped: Collection[Path] | None, stores: Mapping[str, str]
) -> tuple[tuple[str, str], ...]:
    if escaped is not None and source not in escaped:
        return ()
    return tuple(sorted(stores.items()))


NO_GRAPH_NAMESPACE_WRITES = PythonGraphNamespaceWrites(
    MappingProxyType({}), frozenset(), NO_INVALID_MEMBERS, NO_INVALID_MEMBERS
)
UNKNOWN_GRAPH_NAMESPACE_WRITES = PythonGraphNamespaceWrites(
    MappingProxyType({}),
    None,
    ALL_INVALID_MEMBERS,
    ALL_INVALID_MEMBERS,
    None,
    True,
    computed_member_store="an unknown graph",
    import_redirects=UNKNOWN_IMPORT_REDIRECTS,
    frame_locals_writer="an unknown graph",
)


def _namespace_premise(
    named: frozenset[str] | set[str] | None,
    any_module: frozenset[str] | set[str] | None,
    members: MemberMask,
    entry_members: MemberMask,
    children: frozenset[str],
    noncanonical_imports: tuple[str, ...],
    opaque_activation_sources: tuple[PythonSourceKey, ...] | None,
    reflected: bool = False,
    code_rebinding: bool = False,
    escape_writes: tuple[tuple[str, str], ...] = (),
    instance_class_writer: str | None = None,
    frame_locals_writer: str | None = None,
) -> PythonNamespacePremise:
    return PythonNamespacePremise(
        None
        if named is None or any_module is None
        else tuple(
            sorted(children | named | any_module | {name for name, _ in escape_writes})
        ),
        members,
        entry_members,
        noncanonical_imports,
        opaque_activation_sources,
        reflected,
        code_rebinding,
        escape_writes,
        instance_class_writer,
        frame_locals_writer,
    )


def join_python_namespace_premises(
    left: PythonNamespacePremise | None, right: PythonNamespacePremise | None
) -> PythonNamespacePremise | None:
    """Join scan authorities; an unverified namespace cannot grant precision."""
    if left is None or right is None:
        return None
    if left.foreign_universe is not None or right.foreign_universe is not None:
        raise ValueError("scan views do not join; join whole premises")
    names = (
        None
        if left.foreign_writes is None or right.foreign_writes is None
        else tuple(sorted(set(left.foreign_writes) | set(right.foreign_writes)))
    )
    return PythonNamespacePremise(
        names,
        left.member_writes | right.member_writes,
        left.entry_member_writes | right.entry_member_writes,
        tuple(sorted(set(left.noncanonical_imports) | set(right.noncanonical_imports))),
        None
        if left.opaque_activation_sources is None
        or right.opaque_activation_sources is None
        else tuple(
            sorted(
                set(left.opaque_activation_sources)
                | set(right.opaque_activation_sources)
            )
        ),
        left.namespace_reflected or right.namespace_reflected,
        left.code_rebinding or right.code_rebinding,
        tuple(
            sorted(
                {
                    **dict(right.escape_writes),
                    **dict(left.escape_writes),
                }.items()
            )
        ),
        left.instance_class_writer or right.instance_class_writer,
        left.frame_locals_writer or right.frame_locals_writer,
        tuple(sorted(set(left.exact_receivers) & set(right.exact_receivers))),
    )


class PythonGraphNamespaceIndex:
    """One mutable writer index for an operation's monotone discovery queue.

    Merging a source visits only that source's writes. A global delta is
    reported separately so graph owners can wake affected scans without copying
    or visiting every previously indexed target on each module admission.
    """

    def __init__(self) -> None:
        self.by_source: dict[Path, set[str] | None] = {}
        self.any_module: set[str] | None = set()
        self.members = NO_INVALID_MEMBERS
        self.entry_members = NO_INVALID_MEMBERS
        self.reflected: set[Path] | None = set()
        self.code_rebinding = False
        self.member_stores: dict[str, str] = {}
        self.local_definition_stores: dict[str, str] = {}
        self.computed_member_store: str | None = None
        self.frame_locals_writer: str | None = None
        self.escaped: set[Path] | None = set()
        self.import_redirects = NO_IMPORT_REDIRECTS

    def merge(self, batch: PythonGraphNamespaceWrites) -> frozenset[Path] | None:
        changed: set[Path] = set()
        # The graph fact is stored once here; a premise sees only its
        # canonical projection (M7), so only that projection wakes scans.
        redirects = self.import_redirects.join(batch.import_redirects)
        canonical_change = (
            redirects.canonical_redirected()
            != self.import_redirects.canonical_redirected()
        )
        self.import_redirects = redirects
        new_stores = _earlier_sites(self.member_stores, batch.member_stores)
        self.member_stores.update(new_stores)
        new_local_stores = _earlier_sites(
            self.local_definition_stores, batch.local_definition_stores
        )
        self.local_definition_stores.update(new_local_stores)
        # The instance class writer is one fact of every premise.
        escape_global = "__class__" in new_stores
        if batch.computed_member_store is not None and (
            self.computed_member_store is None
        ):
            self.computed_member_store = batch.computed_member_store
            escape_global = True
        # The frame-locals writer is one fact of every premise.
        if batch.frame_locals_writer is not None and self.frame_locals_writer is None:
            self.frame_locals_writer = batch.frame_locals_writer
            escape_global = True
        if self.escaped is not None:
            if batch.escaped is None:
                self.escaped = None
                escape_global = (
                    escape_global
                    or bool(self.member_stores)
                    or bool(self.local_definition_stores)
                )
            else:
                added_sources = batch.escaped - self.escaped
                self.escaped.update(added_sources)
                if self.member_stores or self.local_definition_stores:
                    changed.update(added_sources)
        # A local-definition store reaches escaped modules only under a
        # frame-locals writer, which a native extension may supply per
        # premise; waking them is safe, as unchanged premises compare equal.
        if new_stores or new_local_stores:
            if self.escaped is None:
                escape_global = True
            else:
                changed.update(self.escaped)
        members = self.members | batch.members
        entry_members = self.entry_members | batch.entry_members
        global_change = (
            escape_global
            or canonical_change
            or members != self.members
            or entry_members != self.entry_members
        )
        self.members = members
        self.entry_members = entry_members
        if batch.code_rebinding and not self.code_rebinding:
            self.code_rebinding = True
            global_change = True
        if self.reflected is not None:
            if batch.reflected is None:
                self.reflected = None
                global_change = True
            else:
                added = batch.reflected - self.reflected
                self.reflected.update(added)
                changed.update(added)
        if self.any_module is not None:
            if batch.any_module is None:
                self.any_module = None
                global_change = True
            else:
                added = batch.any_module - self.any_module
                if added:
                    self.any_module.update(added)
                    global_change = True
        if self.any_module is None:
            # Unknown is absorbing for every target in this operation. Named
            # additions cannot change any premise, so retain no dead index.
            self.by_source.clear()
            return None if global_change else frozenset(changed)
        for source, names in batch.by_source.items():
            if source not in self.by_source:
                if names is None or names:
                    self.by_source[source] = None if names is None else set(names)
                    changed.add(source)
                continue
            previous = self.by_source[source]
            if previous is None:
                continue
            if names is None:
                self.by_source[source] = None
                changed.add(source)
            else:
                added = names - previous
                if added:
                    previous.update(added)
                    self.by_source[source] = previous
                    changed.add(source)
        return None if global_change else frozenset(changed)

    def premise(
        self,
        source: Path,
        children: frozenset[str],
        *,
        noncanonical_imports: tuple[str, ...] = (),
        opaque_activation_sources: tuple[PythonSourceKey, ...] | None = None,
        native_code: bool = False,
    ) -> PythonNamespacePremise:
        return _namespace_premise(
            self.by_source.get(source, frozenset()),
            self.any_module,
            self.members,
            self.entry_members,
            children,
            _redirected_canonical(noncanonical_imports, self.import_redirects),
            opaque_activation_sources,
            self.reflected is None or source in self.reflected,
            self.code_rebinding or native_code,
            _escape_writes(
                source,
                self.escaped,
                _module_member_stores(
                    self.member_stores,
                    self.local_definition_stores,
                    _frame_locals_writer(self.frame_locals_writer, native_code),
                ),
            ),
            _instance_class_writer(
                self.member_stores, self.computed_member_store, native_code
            ),
            _frame_locals_writer(self.frame_locals_writer, native_code),
        )


def bind_namespace_writer(
    write: PythonNamespaceWrite, module: str
) -> PythonNamespaceWrite:
    """Name the writing module of an own-module escape for a flattened graph walk.

    An ``own_module_escape`` row means the writer's own module value escaped;
    a caller that aggregates many modules' rows at once names it here.
    """
    if write.construct != "own_module_escape":
        return write
    return replace(write, module=module, construct="module_escape")


def graph_python_namespace_writes(
    writes: Iterable[PythonNamespaceWrite],
    resolve_module: Callable[[str], Path | None],
    *,
    entry: bool = True,
    writer: tuple[str, Path] | None = None,
) -> PythonGraphNamespaceWrites:
    """Aggregate named writes against the caller's admitted source topology.

    Source capture, package children, graph discovery and fixpoint scheduling
    remain with the graph owner; this function performs no filesystem work.
    """
    by_source: dict[Path, set[str] | None] = {}
    any_module: set[str] | None = set()
    reflected: set[Path] | None = set()
    code_rebinding = False
    members = NO_INVALID_MEMBERS
    member_stores: dict[str, str] = {}
    local_definition_stores: dict[str, str] = {}
    computed_member_store: tuple[int, str] | None = None
    frame_locals: tuple[int, str] | None = None
    escaped: set[Path] | None = set()
    label = writer[0] if writer is not None else "<module>"
    global_writers: list[ImportWriter] = []
    key_writers: list[tuple[str, ImportWriter]] = []
    bound_writers = []
    guarded = []
    # D2b spec facts (python_import_redirects.ImportRedirects.closed_writers).
    spec_writers: list[tuple[str | None, ImportWriter]] = []
    spec_member_stores: list[ImportWriter] = []
    escaped_names: set[str] | None = set()
    import_hook_writers: list[ImportWriter] = []
    for write in writes:
        site = f"{label}:{write.line}"
        if write.construct in {"attribute", "setattr", "namespace"} and (
            write.name is None or write.name in IMPORT_SPEC_ATTRIBUTES
        ):
            spec_writers.append(
                (
                    write.module,
                    ImportWriter(
                        site, f"{write.construct} store of {write.name or 'a name'}"
                    ),
                )
            )
        elif write.construct == "reflection":
            # A namespace mapping reached by identity may store any name; the
            # ``sys`` mapping can rebind ``sys.modules`` itself.
            spec_writers.append(
                (
                    None if write.module in (None, "sys") else write.module,
                    ImportWriter(site, "a namespace reflection"),
                )
            )
        elif write.construct == "member_store" and write.name == "_initializing":
            spec_writers.append((None, ImportWriter(site, "an _initializing store")))
        elif write.construct == "member_store" and write.name in IMPORT_SPEC_ATTRIBUTES:
            # Joined with escaped module values as the premise joins named
            # stores (``_escape_writes``); a computed store on a non-module
            # receiver writes no module namespace there either.
            spec_member_stores.append(ImportWriter(site, f"a store of {write.name}"))
        if write.construct == "module_escape" and escaped_names is not None:
            if write.module is None:
                escaped_names = None
            else:
                escaped_names.add(write.module)
        elif write.construct == "own_module_escape" and escaped_names is not None:
            if writer is None:
                escaped_names = None
            else:
                escaped_names.add(writer[0])
        if write.construct == "guarded_import":
            guarded.append(
                (
                    write.module if write.key_bound is None else write.key_bound,
                    ImportWriter(site, "an import under a swallowing handler"),
                )
            )
            continue
        if write.construct == "frame_locals":
            if frame_locals is None or write.line < frame_locals[0]:
                frame_locals = (write.line, f"{label}:{write.line}")
            continue
        if write.construct == "computed_member_store":
            if computed_member_store is None or write.line < computed_member_store[0]:
                computed_member_store = (write.line, f"{label}:{write.line}")
            continue
        if write.construct == "member_store":
            assert write.name is not None
            member_stores.setdefault(write.name, f"{label}:{write.line}")
            continue
        if write.construct == "local_definition_store":
            assert write.name is not None
            local_definition_stores.setdefault(write.name, f"{label}:{write.line}")
            continue
        if write.construct in {"module_escape", "own_module_escape"}:
            if escaped is None:
                continue
            spelling = write.module
            if write.construct == "own_module_escape":
                # Without its writer's source the value is any module's.
                if writer is None:
                    escaped = None
                else:
                    escaped.add(writer[1])
            elif spelling is None:
                escaped = None
            elif (target := resolve_module(spelling)) is not None:
                escaped.add(target)
            continue
        if write.construct == "code":
            code_rebinding = True
            continue
        if write.construct == "reflection":
            if write.module is None:
                reflected = None
            elif reflected is not None:
                target = resolve_module(write.module)
                if target is not None:
                    reflected.add(target)
            continue
        if write.construct == "import_binding":
            key = write.name if write.key_bound is None else write.key_bound
            members |= canonical_import_binding_write_mask(key)
            if write.name is not None:
                key_writers.append(
                    (
                        write.name,
                        ImportWriter(site, f"sys.modules[{write.name!r}] write"),
                    )
                )
            elif write.key_bound is not None:
                bound = write.key_bound
                bound_writers.append(
                    (
                        bound,
                        ImportWriter(
                            site,
                            f"sys.modules write of a key {bound.prefix!r}...{bound.suffix!r}",
                        ),
                    )
                )
            else:
                global_writers.append(
                    ImportWriter(site, "sys.modules write of an unbounded key")
                )
            continue
        written = python_canonical_member_writes(write.module, write.name)
        members |= written
        if written & int(
            PythonMember.SYS_MODULES
            | PythonMember.IMPORT_HOOKS
            | PythonMember.BUILTINS_IMPORT
        ):
            member_writer = ImportWriter(site, f"write of {write.module}.{write.name}")
            global_writers.append(member_writer)
            if written & int(PythonMember.BUILTINS_IMPORT):
                import_hook_writers.append(member_writer)
        if write.module is None:
            if write.name is None or any_module is None:
                any_module = None
            else:
                any_module.add(write.name)
            continue
        target = resolve_module(write.module)
        if target is None:
            # Not a local module: an attribute of some object, or a module
            # outside this graph's sources.
            continue
        names = by_source.setdefault(target, set())
        if write.name is None or names is None:
            by_source[target] = None
        else:
            names.add(write.name)
    return PythonGraphNamespaceWrites(
        MappingProxyType(
            {
                source: None if names is None else frozenset(names)
                for source, names in by_source.items()
            }
        ),
        None if any_module is None else frozenset(any_module),
        members,
        members if entry else NO_INVALID_MEMBERS,
        None if reflected is None else frozenset(reflected),
        code_rebinding,
        MappingProxyType(member_stores),
        None if escaped is None else frozenset(escaped),
        None if computed_member_store is None else computed_member_store[1],
        import_redirects(
            global_writers,
            key_writers,
            bound_writers,
            guarded,
            spec_writers,
            spec_member_stores,
            escaped_names,
            import_hook_writers,
        ),
        None if frame_locals is None else frame_locals[1],
        MappingProxyType(local_definition_stores),
    )
