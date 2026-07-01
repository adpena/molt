from __future__ import annotations

from .render_rust_common import _render_matches_arm, _rs_bool

_OPERAND_OWNERSHIP_VARIANT = {
    "borrowed": "OperandOwnership::Borrowed",
    "consumed": "OperandOwnership::Consumed",
    # The borrow-of-edge leaf (design 27 §1.5 / §2.1, ladder #73): a per-position
    # opcode operand whose result holds an interior reference into it (the
    # `LoadAttr`/`Index` source — the round-6 `Counter._handle` keepalive). Read by
    # `opcode_borrows_source_operand` and `op_borrow_source` in alias_analysis.rs.
    "interior_borrow_keepalive": "OperandOwnership::InteriorBorrowKeepAlive",
    # Existing-container store leaf: the op borrows the operand while retaining
    # its own container/storage reference. DropInsertion uses this as a release
    # boundary for finalizer-sensitive producer temps.
    "container_absorb": "OperandOwnership::ContainerAbsorb",
    # Move-out leaves used by the per-TERMINATOR table (design 27 §2.4). The
    # opcode `operand_ownership` validator restricts opcodes to
    # borrowed|consumed|interior_borrow_keepalive|container_absorb; these are reachable only via
    # the terminator categories.
    "transferred": "OperandOwnership::Transferred",
    "none": "OperandOwnership::NoOperand",
}

_RESULT_VALIDITY_VARIANT = {
    "conditional_valid_only_on_edge": "ResultValidity::ConditionalValidOnlyOnEdge",
}


def _render_operand_ownership(
    opcodes: list[dict],
    consuming: list[dict],
    absorbing_operands: list[dict],
) -> str:
    """Render the operand-ownership tables (design 27 §2.1/§2.3):

    * ``OperandOwnership`` — the per-operand borrowed|consumed leaf.
    * ``opcode_operand_ownership_table(opcode, operand_idx)`` — the per-OpCode
      DEFAULT, EXHAUSTIVE over the enum (a new variant fails to compile until
      classified). Honors the per-position list form (a list opcode dispatches
      on ``operand_idx``); a uniform opcode ignores the index.
    * ``kind_consumed_operand_table(kind, arity)`` — the per-SPELLING consume
      override keyed on the ``_original_kind`` attr. Returns the 0-based index
      of the consumed operand, resolving ``"last"`` against the op's ``arity``.
      This is the table ``op_consumed_operand_root`` reads (replacing the
      hand-coded ``matches!(_original_kind, "call_bind" | "call_indirect")``).
    """
    out: list[str] = []
    # `operand_idx` is referenced by the match body ONLY when some opcode carries
    # a per-position list (which renders a `match operand_idx { … }` arm). When
    # every opcode is uniform (`all_borrowed`/`all_consumed`), the index is
    # genuinely unused — emit the idiomatic `_operand_idx` so the generated file
    # stays warning-free (rather than an `#[allow]` blanket). The PUBLIC contract
    # is still "indexed by operand position"; the name flips to `operand_idx` the
    # moment a per-position classification lands.
    any_per_position = any(
        isinstance(row["operand_ownership"], list)
        and len(set(row["operand_ownership"])) > 1
        for row in opcodes
    )
    idx_param = "operand_idx" if any_per_position else "_operand_idx"
    out.append(
        "/// Operand-ownership leaf (design 27 §2.1): does an op release this\n"
        "/// operand internally (`Consumed` — the holder must NOT also drop it, a\n"
        "/// double-free otherwise) or merely borrow it (`Borrowed` — the holder\n"
        "/// keeps its obligation and drops at the value's true last use)? molt's\n"
        "/// `callee borrows all args` ABI (design 20 §1.2) makes `Borrowed` the\n"
        "/// universal default; `Consumed` is the CallArgs-builder / move-into class.\n"
        "/// The result-side lattice (Owned/Borrowed/Raw/MaybeUninit) is the\n"
        "/// classifier_* tables — a SEPARATE axis from this operand-side leaf.\n"
        "///\n"
        "/// The variant set models molt's FULL operand-ownership domain so the\n"
        "/// design-27 ownership-boundary lattice (#58) and the next consumer\n"
        "/// migrations are TABLE edits, not enum surgery. `Borrowed`/`Consumed`\n"
        "/// seed the per-OpCode + per-spelling tables; `InteriorBorrowKeepAlive`\n"
        "/// seeds the per-position borrow-of column (ladder #73);\n"
        "/// `ContainerAbsorb` marks borrowed operands retained by container/storage\n"
        "/// mutation; `Transferred`\n"
        "/// seeds the per-TERMINATOR table (design 27 §2.4 transfer sites — ladder\n"
        "/// #72). Every variant below is constructed by a generated table today:\n"
        "///   * `Transferred` — ownership moves OUT of the function/block: a\n"
        "///     `Return` value or a branch-arg passed into a successor block arg.\n"
        "///     LIVE: constructed by `terminator_operand_ownership_table` and read\n"
        "///     by drop_insertion's `terminator_uses_root` / `terminator_branch_args`.\n"
        "///   * `InteriorBorrowKeepAlive` — the round-6 interior-borrow keepalive:\n"
        "///     the operand must stay live because the result holds an INTERIOR\n"
        "///     reference into it (drop deferred to the interior ref's last use).\n"
        "///     LIVE: constructed by `opcode_operand_ownership_table` for the\n"
        "///     `LoadAttr`/`Index` source position and read by\n"
        "///     `opcode_borrows_source_operand` / `op_borrow_source` to build the\n"
        "///     `BorrowProvenance` relation (the `Counter._handle` UAF fix).\n"
        "///   * `ContainerAbsorb` — an existing-container/store mutation retains\n"
        "///     this operand while the caller still owns the producer temp ref. This\n"
        "///     gives DropInsertion a shared release boundary for absorbed temps\n"
        "///     without making the mutator consume the operand.\n"
        "///   * `ConditionalValidOnlyOnEdge` — the §2.8 `IterNextUnboxed` value-out:\n"
        "///     valid only on the not-exhausted edge, NEVER unconditionally\n"
        "///     droppable (non-owned `None` sentinel on the exhaustion edge). The LONE\n"
        "///     remaining `from_str`-only variant (its consumer hand-list —\n"
        "///     `iter_cond_value_results` — migrates in the iter-cond tranche, #74).\n"
        "///   * `NoOperand` — no ref-bearing operand in that category (a\n"
        "///     raw lane; a terminator category absent on a variant — `Branch` has\n"
        "///     no direct operand, `Return` forwards no branch arg).\n"
        "#[derive(Clone, Copy, PartialEq, Eq, Debug)]\n"
        "pub enum OperandOwnership {\n"
        "    Borrowed,\n"
        "    Consumed,\n"
        "    Transferred,\n"
        "    InteriorBorrowKeepAlive,\n"
        "    ContainerAbsorb,\n"
        "    ConditionalValidOnlyOnEdge,\n"
        "    NoOperand,\n"
        "}\n\n"
        "// Parse/render path for the operand-ownership vocabulary. `Transferred`\n"
        "// is LIVE through `terminator_operand_ownership_table` (ladder #72) and\n"
        "// `InteriorBorrowKeepAlive` through `opcode_operand_ownership_table` /\n"
        "// `opcode_borrows_source_operand` (ladder #73); `from_str` remains the\n"
        "// toml-ingest path the LAST migration (the `conditional_valid_only_on_edge`\n"
        "// row, #74) reads and is not yet wired to a runtime caller, so\n"
        "// `from_str`/`as_str`/`ALL` keep allow(dead_code) — SCOPED to this\n"
        "// forward-compat parse API, never the enum (every variant is constructed)\n"
        "// nor the file. `ALL` + the round-trip test keep every variant constructed\n"
        "// and live today.\n"
        "#[allow(dead_code)]\n"
        "impl OperandOwnership {\n"
        "    pub const ALL: [OperandOwnership; 7] = [\n"
        "        OperandOwnership::Borrowed,\n"
        "        OperandOwnership::Consumed,\n"
        "        OperandOwnership::Transferred,\n"
        "        OperandOwnership::InteriorBorrowKeepAlive,\n"
        "        OperandOwnership::ContainerAbsorb,\n"
        "        OperandOwnership::ConditionalValidOnlyOnEdge,\n"
        "        OperandOwnership::NoOperand,\n"
        "    ];\n"
        "    pub fn as_str(self) -> &'static str {\n"
        "        match self {\n"
        '            OperandOwnership::Borrowed => "borrowed",\n'
        '            OperandOwnership::Consumed => "consumed",\n'
        '            OperandOwnership::Transferred => "transferred",\n'
        '            OperandOwnership::InteriorBorrowKeepAlive => "interior_borrow_keepalive",\n'
        '            OperandOwnership::ContainerAbsorb => "container_absorb",\n'
        '            OperandOwnership::ConditionalValidOnlyOnEdge => "conditional_valid_only_on_edge",\n'
        '            OperandOwnership::NoOperand => "no_operand_ownership",\n'
        "        }\n"
        "    }\n"
        "    pub fn from_str(s: &str) -> Option<OperandOwnership> {\n"
        "        match s {\n"
        '            "borrowed" => Some(OperandOwnership::Borrowed),\n'
        '            "consumed" => Some(OperandOwnership::Consumed),\n'
        '            "transferred" => Some(OperandOwnership::Transferred),\n'
        '            "interior_borrow_keepalive" => Some(OperandOwnership::InteriorBorrowKeepAlive),\n'
        '            "container_absorb" => Some(OperandOwnership::ContainerAbsorb),\n'
        '            "conditional_valid_only_on_edge" => Some(OperandOwnership::ConditionalValidOnlyOnEdge),\n'
        '            "no_operand_ownership" => Some(OperandOwnership::NoOperand),\n'
        "            _ => None,\n"
        "        }\n"
        "    }\n"
        "}\n\n"
        "#[cfg(test)]\n"
        "mod operand_ownership_schema_tests {\n"
        "    use super::OperandOwnership;\n"
        "    #[test]\n"
        "    fn every_variant_round_trips() {\n"
        "        // The schema is alive: every declared variant parses + renders +\n"
        "        // round-trips. Dropping or renaming a variant breaks this test.\n"
        "        for v in OperandOwnership::ALL {\n"
        "            assert_eq!(OperandOwnership::from_str(v.as_str()), Some(v));\n"
        "        }\n"
        '        assert_eq!(OperandOwnership::from_str("bogus"), None);\n'
        "    }\n"
        "}\n\n"
    )

    out.append(
        "/// Per-OpCode operand-ownership DEFAULT: how `OpCode` treats the operand\n"
        "/// at `operand_idx`. EXHAUSTIVE over the enum — a new variant fails to\n"
        "/// compile until it is given an `operand_ownership` row in op_kinds.toml.\n"
        "/// A uniform opcode (`all_borrowed`/`all_consumed`) ignores the index; a\n"
        "/// per-position opcode dispatches on it (positions past the listed arity\n"
        "/// fall back to the LAST listed leaf — variadic tails inherit the final\n"
        "/// position's treatment). This is the per-OpCode floor; a finer\n"
        "/// per-`_original_kind` consume is `kind_consumed_operand_table`.\n"
        "#[inline]\n"
        "pub fn opcode_operand_ownership_table(\n"
        "    opcode: OpCode,\n"
        f"    {idx_param}: usize,\n"
        ") -> OperandOwnership {\n"
        "    match opcode {\n"
    )
    for row in opcodes:
        name = row["name"]
        spec = row["operand_ownership"]
        out.append(f"        OpCode::{name} => {_operand_ownership_arm(spec)},\n")
    out.append("    }\n}\n\n")

    # Derived borrow-of authority (design 27 §1.5 / §2.1, ladder #73): the
    # operand index an opcode's result interior-borrows (its
    # `interior_borrow_keepalive` position), or `None`. This is the single
    # declarative fact `op_borrow_source` (alias_analysis.rs) reads — the migrated
    # interior-borrow-keepalive relation, no longer a hardcoded `LoadAttr | Index`
    # match. EXHAUSTIVE over the enum (every opcode is classified by its
    # `operand_ownership` row). A future op whose result interior-borrows an
    # operand gets correct keepalive by setting that position to
    # `interior_borrow_keepalive` in op_kinds.toml — never by editing the pass.
    out.append(
        "/// The operand index whose backing store this op's result interior-borrows\n"
        "/// (design 27 §1.5 borrow-of edge): the operand position classified\n"
        "/// `OperandOwnership::InteriorBorrowKeepAlive`, or `None` if the op's result\n"
        "/// borrows into no operand. Derived from the per-OpCode `operand_ownership`\n"
        "/// row — the SINGLE declarative authority `op_borrow_source`\n"
        "/// (alias_analysis.rs) reads to build the `BorrowProvenance` keepalive\n"
        "/// relation, REPLACING the hand-coded\n"
        "/// `LoadAttr | Index` match (the round-6 `Counter._handle` UAF fix). The\n"
        "/// source object's drop is deferred to the borrow result's last use, so a\n"
        "/// finalizer that owns the backing store cannot run while the borrow lives.\n"
        "/// EXHAUSTIVE over the enum — a new interior-borrowing op is classified by a\n"
        "/// table edit, not a pass edit. At most one interior-borrow operand exists in\n"
        "/// molt's lowering today (the container/object at position 0); the first such\n"
        "/// position is returned.\n"
        "#[inline]\n"
        "pub fn opcode_borrows_source_operand(opcode: OpCode) -> Option<usize> {\n"
        "    match opcode {\n"
    )
    for row in opcodes:
        name = row["name"]
        idx = _borrows_source_operand_index(row["operand_ownership"])
        if idx is not None:
            out.append(f"        OpCode::{name} => Some({idx}),\n")
    out.append("        _ => None,\n")
    out.append("    }\n}\n\n")

    out.append(
        "/// The operand index retained by an existing container/store mutation.\n"
        "/// The op still borrows the operand for ABI/drop purposes; this fact only\n"
        "/// records that the container now owns its own reference, so a\n"
        "/// finalizer-sensitive producer temp can release its caller-owned ref at\n"
        "/// this statement. Derived from `container_absorb` operand rows.\n"
        "#[inline]\n"
        "pub fn opcode_container_absorbed_operand(opcode: OpCode) -> Option<usize> {\n"
        "    match opcode {\n"
    )
    for row in opcodes:
        name = row["name"]
        idx = _container_absorb_operand_index(row["operand_ownership"])
        if idx is not None:
            out.append(f"        OpCode::{name} => Some({idx}),\n")
    out.append("        _ => None,\n")
    out.append("    }\n}\n\n")

    out.append(
        "/// Per-SPELLING consume override (design 27 §2.3): for a `Copy`-lifted op\n"
        "/// carrying `_original_kind = kind`, the 0-based index of the operand the\n"
        "/// op CONSUMES (frees internally), or `None` if it consumes none. `arity`\n"
        '/// is the op\'s operand count, used to resolve a `"last"` selector. The\n'
        "/// drop pass treats a value whose last use is the consumed-operand\n"
        "/// position exactly like a `Return` transfer — no trailing `DecRef`.\n"
        "/// Replaces the hand-coded `op_consumed_operand_root` match.\n"
        "#[inline]\n"
        "pub fn kind_consumed_operand_table(kind: &str, arity: usize) -> Option<usize> {\n"
        "    match kind {\n"
    )
    if consuming:
        for row in consuming:
            kind = row["kind"]
            sel = row["consumed_operand"]
            if sel == "last":
                out.append(f'        "{kind}" => arity.checked_sub(1),\n')
            else:
                out.append(f'        "{kind}" => Some({int(sel)}),\n')
    out.append("        _ => None,\n")
    out.append("    }\n}\n")
    absorbed_uses_arity = any(
        row["absorbed_operand"] == "last" for row in absorbing_operands
    )
    absorbed_arity_param = "arity" if absorbed_uses_arity else "_arity"
    out.append(
        "\n/// Per-SPELLING existing-container absorption override. These preserved\n"
        "/// SimpleIR spellings lower as `Copy` with `_original_kind`, so they need a\n"
        "/// spelling table parallel to `kind_consumed_operand_table`.\n"
        "#[inline]\n"
        f"pub fn kind_container_absorbed_operand_table(kind: &str, {absorbed_arity_param}: usize) -> Option<usize> {{\n"
        "    match kind {\n"
    )
    if absorbing_operands:
        for row in absorbing_operands:
            kind = row["kind"]
            sel = row["absorbed_operand"]
            if sel == "last":
                out.append(f'        "{kind}" => arity.checked_sub(1),\n')
            else:
                out.append(f'        "{kind}" => Some({int(sel)}),\n')
    out.append("        _ => None,\n")
    out.append("    }\n}\n")
    return "".join(out)


def _render_result_absorption(
    opcodes: list[dict],
    absorbing: list[dict],
    result_sources: list[dict],
) -> str:
    """Render the result-absorbs-operands ownership-transfer tables.

    This is a RESULT-side fact: the returned value owns the operands' lifetimes
    even though the operands remain borrowed at the ABI/drop-insertion edge.
    First-class opcodes use the exhaustive opcode bit; Copy-lifted SimpleIR
    spellings use the spelling table.
    """
    out: list[str] = []
    out.append(
        "/// Result-side ownership-transfer fact: this op returns a value whose\n"
        "/// lifetime absorbs the lifetimes of its operands (container builders).\n"
        "/// This is deliberately separate from operand_ownership: operands are still\n"
        "/// borrowed at the call/drop boundary, but a finalizer-sensitive operand\n"
        "/// makes the returned container finalizer-sensitive. EXHAUSTIVE over\n"
        "/// OpCode; Copy-lifted spellings use `kind_result_absorbs_operand_ownership_table`.\n"
        "#[inline]\n"
        "pub fn opcode_result_absorbs_operand_ownership_table(opcode: OpCode) -> bool {\n"
        "    match opcode {\n"
    )
    for row in opcodes:
        out.append(
            f"        OpCode::{row['name']} => {_rs_bool(row['result_absorbs_operands'])},\n"
        )
    out.append("    }\n}\n\n")

    out.append(
        "/// Result-side selected-alias ownership fact. These opcodes return one\n"
        "/// borrowed operand's bits as their result, so backend lowering must\n"
        "/// retain the selected object when an owned boxed result is produced.\n"
        "/// Raw scalar lanes remain refcount-free. The table is keyed by explicit\n"
        "/// `result_mints_owned_selected_operand` rows in op_kinds.toml.\n"
        "#[inline]\n"
        "pub fn opcode_result_mints_owned_selected_operand_table(opcode: OpCode) -> bool {\n"
    )
    selected_owner_opcodes = sorted(
        row["name"]
        for row in opcodes
        if row.get("result_mints_owned_selected_operand", False)
    )
    if selected_owner_opcodes:
        out.append("    matches!(\n        opcode,\n")
        for i, name in enumerate(selected_owner_opcodes):
            sep = "" if i == len(selected_owner_opcodes) - 1 else " |"
            out.append(f"        OpCode::{name}{sep}\n")
        out.append("    )\n")
    else:
        out.append("    let _ = opcode;\n    false\n")
    out.append("}\n\n")

    out.append(
        "/// Same selected-alias result ownership fact keyed by SimpleIR kind spelling.\n"
        "/// String-dispatch backends must query this rather than duplicating an\n"
        "/// `and`/`or` list by hand.\n"
        "#[inline]\n"
        "pub fn kind_result_mints_owned_selected_operand_table(kind: &str) -> bool {\n"
        "    kind_to_opcode_table(kind)\n"
        "        .is_some_and(opcode_result_mints_owned_selected_operand_table)\n"
        "}\n\n"
    )

    out.append(
        "/// Result-side ownership-transfer fact for Copy-lifted SimpleIR spellings.\n"
        "/// These spellings intentionally remain outside `[[kind]]` so backconversion\n"
        "/// and backend dispatch preserve their public wire names while still sharing\n"
        "/// the finalizer/escape ownership fact with first-class Build* opcodes.\n"
        "#[inline]\n"
        "pub fn kind_result_absorbs_operand_ownership_table(kind: &str) -> bool {\n"
        "    matches!(kind,\n"
    )
    absorbing_kinds = sorted(row["kind"] for row in absorbing)
    out.append(_render_matches_arm(absorbing_kinds))
    out.append("    )\n}\n\n")

    result_source_uses_arity = any(
        row["source_operand"] == "last" for row in result_sources
    )
    result_source_arity_param = "arity" if result_source_uses_arity else "_arity"
    out.append(
        "/// Per-SPELLING result finalizer-source facts. These Copy-lifted\n"
        "/// extraction spellings return a fresh owned result whose finalizer\n"
        "/// sensitivity is inherited from one source operand, but whose own\n"
        "/// temporary ref should release at the statement unless Python-bound.\n"
        "#[inline]\n"
        f"pub fn kind_result_finalizer_source_operand_table(kind: &str, {result_source_arity_param}: usize) -> Option<usize> {{\n"
        "    match kind {\n"
    )
    if result_sources:
        for row in result_sources:
            kind = row["kind"]
            sel = row["source_operand"]
            if sel == "last":
                out.append(f'        "{kind}" => arity.checked_sub(1),\n')
            else:
                out.append(f'        "{kind}" => Some({int(sel)}),\n')
    out.append("        _ => None,\n")
    out.append("    }\n}\n")
    return "".join(out)


def _render_result_validity(opcodes: list[dict], rows: list[dict]) -> str:
    """Render per-result validity facts.

    `IterNextUnboxed` result 0 is only initialized on the not-done edge. The
    table keeps that path-sensitive result fact beside the other op-kind
    semantics instead of duplicating it inside drop insertion.
    """
    by_opcode: dict[str, dict[int, str]] = {}
    for row in rows:
        by_opcode.setdefault(row["opcode"], {})[row["result"]] = row["validity"]

    out: list[str] = []
    out.append(
        "/// Result-validity fact for op results whose bits are not valid on every\n"
        "/// outgoing edge. `ConditionalValidOnlyOnEdge` is the §2.8\n"
        "/// `IterNextUnboxed` value-out: result 0 is initialized only on the\n"
        "/// not-done edge and must never be dropped or retained from the exhaustion\n"
        "/// edge. EXHAUSTIVE over OpCode; result indices not listed for an opcode\n"
        "/// are unconditionally valid.\n"
        "#[derive(Clone, Copy, PartialEq, Eq, Debug)]\n"
        "pub enum ResultValidity {\n"
        "    AlwaysValid,\n"
        "    ConditionalValidOnlyOnEdge,\n"
        "}\n\n"
        "#[inline]\n"
        "pub fn opcode_result_validity_table(\n"
        "    opcode: OpCode,\n"
        "    result_idx: usize,\n"
        ") -> ResultValidity {\n"
        "    match opcode {\n"
    )
    for row in opcodes:
        name = row["name"]
        result_rows = by_opcode.get(name, {})
        if not result_rows:
            out.append(f"        OpCode::{name} => ResultValidity::AlwaysValid,\n")
            continue
        out.append(f"        OpCode::{name} => match result_idx {{\n")
        for idx in sorted(result_rows):
            variant = _RESULT_VALIDITY_VARIANT[result_rows[idx]]
            out.append(f"            {idx} => {variant},\n")
        out.append("            _ => ResultValidity::AlwaysValid,\n")
        out.append("        },\n")
    out.append("    }\n}\n\n")
    out.append(
        "#[inline]\n"
        "pub fn opcode_result_is_conditionally_valid_only_on_edge(\n"
        "    opcode: OpCode,\n"
        "    result_idx: usize,\n"
        ") -> bool {\n"
        "    matches!(\n"
        "        opcode_result_validity_table(opcode, result_idx),\n"
        "        ResultValidity::ConditionalValidOnlyOnEdge\n"
        "    )\n"
        "}\n"
    )
    return "".join(out)


def _render_explicit_release_operands(opcodes: list[dict], rows: list[dict]) -> str:
    """Render Python lifetime release-boundary operand facts."""
    by_opcode = {row["opcode"]: row["operand"] for row in rows}
    uses_arity = any(row["operand"] == "last" for row in rows)
    arity_param = "arity" if uses_arity else "_arity"
    out: list[str] = []
    out.append(
        "/// Python lifetime release-boundary fact: which operand roots an opcode\n"
        "/// explicitly releases. This is separate from operand ownership: `DecRef`\n"
        "/// consumes/releases all operands, `DelBoundary` marks a variable lifetime\n"
        "/// boundary, and `DeleteVar` releases the old slot occupant at operand 1\n"
        "/// after storing the missing sentinel. DropInsertion and diagnostics use\n"
        "/// this table to avoid pass-local release hand lists.\n"
        "#[derive(Clone, Copy, PartialEq, Eq, Debug)]\n"
        "pub enum ExplicitReleaseOperands {\n"
        "    None,\n"
        "    All,\n"
        "    One(usize),\n"
        "}\n\n"
        "#[inline]\n"
        "pub fn opcode_explicit_release_operands_table(\n"
        "    opcode: OpCode,\n"
        f"    {arity_param}: usize,\n"
        ") -> ExplicitReleaseOperands {\n"
        "    match opcode {\n"
    )
    for row in opcodes:
        name = row["name"]
        operand = by_opcode.get(name)
        if operand is None:
            out.append(f"        OpCode::{name} => ExplicitReleaseOperands::None,\n")
        elif operand == "all":
            out.append(f"        OpCode::{name} => ExplicitReleaseOperands::All,\n")
        elif operand == "last":
            out.append(
                f"        OpCode::{name} => match arity.checked_sub(1) {{\n"
                "            Some(idx) => ExplicitReleaseOperands::One(idx),\n"
                "            None => ExplicitReleaseOperands::None,\n"
                "        },\n"
            )
        else:
            out.append(
                f"        OpCode::{name} => ExplicitReleaseOperands::One({int(operand)}),\n"
            )
    out.append("    }\n}\n")
    return "".join(out)


def _render_terminator_ownership(terminators: list[dict]) -> str:
    """Render the per-TERMINATOR operand-ownership authority (design 27 §2.4):

    * ``TerminatorKind`` — a zero-cost discriminant of the ``Terminator`` enum
      (blocks.rs) the table is keyed on (the drop pass maps ``&Terminator`` ->
      ``TerminatorKind`` with one structural match). EXHAUSTIVE over the enum.
    * ``OperandCategory`` — ``Direct`` (the terminator's own operands: a
      ``Return`` value, a ``CondBranch``/``Switch`` predicate) vs ``BranchArg``
      (a value forwarded into a successor's phi). The two categories have
      different ownership, so they are classified independently.
    * ``terminator_operand_ownership_table(kind, category)`` — the per-(variant,
      category) ``OperandOwnership`` leaf, EXHAUSTIVE over both axes.
    * ``terminator_operand_is_transferred(kind, category)`` — the derived
      predicate drop_insertion reads: ``true`` iff the leaf is ``Transferred``
      (ownership moves OUT — no trailing ``DecRef`` at the transfer point). This
      is the generated authority that REPLACES the hand-coded transfer carve-out
      in ``terminator_branch_args`` + the ``Return`` arm of ``terminator_uses_root``.
    """
    out: list[str] = []
    out.append(
        "/// Zero-cost discriminant of the `Terminator` enum (blocks.rs) the\n"
        "/// per-terminator operand-ownership table is keyed on. EXHAUSTIVE over the\n"
        "/// enum — a new `Terminator` variant fails to render until it is given a\n"
        "/// [[terminator]] row in op_kinds.toml (the transfer-carve-out kill: an\n"
        "/// unclassified terminator can't silently inherit a borrow/transfer\n"
        "/// assumption). The drop pass maps `&Terminator` -> `TerminatorKind` with\n"
        "/// one structural match; this keeps the ownership FACT declarative while\n"
        "/// the structural shape (which fields carry args) stays in the pass.\n"
        "#[derive(Clone, Copy, PartialEq, Eq, Debug)]\n"
        "pub enum TerminatorKind {\n"
    )
    for row in terminators:
        out.append(f"    {row['name']},\n")
    out.append("}\n\n")

    out.append(
        "/// Which operand CATEGORY of a terminator a query is about: the\n"
        "/// terminator's own `Direct` operands (a `Return` value, a `CondBranch`/\n"
        "/// `Switch` predicate) versus a `BranchArg` forwarded into a successor's\n"
        "/// block-arg (phi). The two have different ownership (a `Return` value\n"
        "/// transfers to the caller; a predicate is borrowed; a branch-arg transfers\n"
        "/// into the phi) so they are classified on separate axes.\n"
        "#[derive(Clone, Copy, PartialEq, Eq, Debug)]\n"
        "pub enum OperandCategory {\n"
        "    Direct,\n"
        "    BranchArg,\n"
        "}\n\n"
    )

    out.append(
        "/// Per-(terminator variant, operand category) ownership leaf (design 27\n"
        "/// §2.4). EXHAUSTIVE over both axes — a new `Terminator` variant fails to\n"
        "/// compile until classified. `Transferred` = ownership moves OUT (a\n"
        "/// `Return` value to the caller; a branch-arg into a successor phi);\n"
        "/// `Borrowed` = the predicate is read but not moved (drop relocated to the\n"
        "/// dying edge); `NoOperand` = the variant has no operand in that\n"
        "/// category. The consume axis is N/A for a terminator (nothing frees a\n"
        "/// terminator operand internally), so `Consumed` never appears here.\n"
        "#[inline]\n"
        "pub fn terminator_operand_ownership_table(\n"
        "    kind: TerminatorKind,\n"
        "    category: OperandCategory,\n"
        ") -> OperandOwnership {\n"
        "    match (kind, category) {\n"
    )
    for row in terminators:
        name = row["name"]
        direct = _OPERAND_OWNERSHIP_VARIANT[row["direct"]]
        branch = _OPERAND_OWNERSHIP_VARIANT[row["branch_arg"]]
        out.append(
            f"        (TerminatorKind::{name}, OperandCategory::Direct) => {direct},\n"
        )
        out.append(
            f"        (TerminatorKind::{name}, OperandCategory::BranchArg) => {branch},\n"
        )
    out.append("    }\n}\n\n")

    out.append(
        "/// Derived transfer predicate drop_insertion reads (design 27 §2.4): does\n"
        "/// the terminator TRANSFER ownership of an operand in `category`? `true`\n"
        "/// iff the leaf is `Transferred` — the drop pass must NOT emit a trailing\n"
        "/// `DecRef` at the transfer point (the caller / successor phi owns it).\n"
        "/// This single declarative authority REPLACES the hand-coded transfer\n"
        "/// carve-out (the `Return` arm of `terminator_uses_root` + the\n"
        "/// `terminator_branch_args` membership). A future terminator transfer fact\n"
        "/// is a [[terminator]] row edit, never a drop-pass edit.\n"
        "#[inline]\n"
        "pub fn terminator_operand_is_transferred(\n"
        "    kind: TerminatorKind,\n"
        "    category: OperandCategory,\n"
        ") -> bool {\n"
        "    matches!(\n"
        "        terminator_operand_ownership_table(kind, category),\n"
        "        OperandOwnership::Transferred\n"
        "    )\n"
        "}\n"
    )
    return "".join(out)


def _borrows_source_operand_index(spec: object) -> int | None:
    """The operand index this op's result interior-borrows (design 27 §1.5), or
    ``None``. The first position whose `operand_ownership` leaf is
    ``interior_borrow_keepalive``. A uniform spec (``all_borrowed`` /
    ``all_consumed``) interior-borrows nothing — only the per-position list form
    can carry the keepalive leaf (the validator forbids it as a uniform shorthand,
    so a borrow-of op MUST spell out its operand positions)."""
    if not isinstance(spec, list):
        return None
    for i, leaf in enumerate(spec):
        if leaf == "interior_borrow_keepalive":
            return i
    return None


def _container_absorb_operand_index(spec: object) -> int | None:
    """The operand index retained by an existing container/store mutation, or
    ``None``. Like interior borrows, this is per-position only: a uniform opcode
    cannot name one absorbed value operand without also identifying container/key
    operands."""
    if not isinstance(spec, list):
        return None
    for i, leaf in enumerate(spec):
        if leaf == "container_absorb":
            return i
    return None


def _operand_ownership_arm(spec: object) -> str:
    """Render the RHS of one `opcode_operand_ownership_table` match arm.

    A uniform spec collapses to a constant variant; a per-position list renders a
    nested `match operand_idx` whose final listed position also serves every
    higher index (the variadic-tail rule), keeping the function total."""
    if spec == "all_borrowed":
        return "OperandOwnership::Borrowed"
    if spec == "all_consumed":
        return "OperandOwnership::Consumed"
    assert isinstance(spec, list)
    leaves = [_OPERAND_OWNERSHIP_VARIANT[x] for x in spec]
    if len(set(leaves)) == 1:
        # A homogeneous list is just the uniform case (e.g. ["borrowed"]).
        return leaves[0]
    arms = []
    for i, leaf in enumerate(leaves[:-1]):
        arms.append(f"{i} => {leaf}")
    # The final listed position is the catch-all (covers its index AND any
    # higher variadic-tail index).
    arms.append(f"_ => {leaves[-1]}")
    return "match operand_idx { " + ", ".join(arms) + " }"


# ---------------------------------------------------------------------------
# Python binary-image ownership/allocation facts
# ---------------------------------------------------------------------------

__all__ = [
    name
    for name in globals()
    if name.startswith("_") and not name.startswith("__") or name == "render_rs"
]
