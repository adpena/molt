//! Class Hierarchy Analysis (CHA) and devirtualization pass for TIR.
//!
//! CHA builds a whole-program class hierarchy and uses it to prove that
//! certain virtual method calls can be resolved statically (devirtualized)
//! into direct function calls.
//!
//! # Devirtualization conditions
//! A `CallMethod` op can be devirtualized when:
//!   1. The receiver type is known (stored as `receiver_type` in op attrs).
//!   2. The receiver class is a *leaf* in the hierarchy (no subclasses),
//!      so the method cannot be overridden by a subclass.
//!   3. The method is reachable via MRO from the receiver class.
//!
//! When devirtualized, the `CallMethod` op is replaced with a `Call` op whose
//! callee attribute (`callee`) is set to `"<DefiningClass>_<method>"`.

use std::collections::{HashMap, HashSet};

use super::PassStats;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, Dialect, OpCode};

// ---------------------------------------------------------------------------
// ClassHierarchy
// ---------------------------------------------------------------------------

/// Whole-program class hierarchy for devirtualization.
#[derive(Debug, Default)]
pub struct ClassHierarchy {
    /// class_name → set of direct child class names
    children: HashMap<String, HashSet<String>>,
    /// class_name → set of method names defined directly on this class
    methods: HashMap<String, HashSet<String>>,
    /// class_name → parent class name (None means no explicit parent / inherits object)
    parent: HashMap<String, Option<String>>,
}

impl ClassHierarchy {
    /// Create an empty hierarchy.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a class with its optional parent and the list of method names
    /// defined directly on it (not inherited).
    ///
    /// If `parent` is `Some(p)`, the parent is also registered as a known class
    /// (if not already present) and the child → parent edge is recorded.
    pub fn add_class(&mut self, name: &str, parent: Option<&str>, methods: &[&str]) {
        // Ensure the class has an entry in every map so lookups never miss.
        self.children.entry(name.to_string()).or_default();
        self.methods
            .entry(name.to_string())
            .or_default()
            .extend(methods.iter().map(|m| m.to_string()));
        self.parent
            .insert(name.to_string(), parent.map(|p| p.to_string()));

        // Register parent edge if present.
        if let Some(p) = parent {
            self.children
                .entry(p.to_string())
                .or_default()
                .insert(name.to_string());
            // Ensure parent exists in the other maps without overwriting.
            self.methods.entry(p.to_string()).or_default();
            self.parent.entry(p.to_string()).or_insert(None);
        }
    }

    /// Returns `true` if the class has no subclasses registered in the whole program.
    ///
    /// A class not registered at all is treated as a leaf (open-world assumption
    /// would disagree, but the TIR operates under a closed-world assumption for
    /// registered modules).
    pub fn is_leaf_class(&self, class_name: &str) -> bool {
        self.children.get(class_name).map_or(true, |c| c.is_empty())
    }

    /// For a method call on a known receiver type, returns the fully-qualified
    /// function name `"DefiningClass_method"` if the call can be devirtualized.
    ///
    /// Returns `None` if:
    ///   - The class is not a leaf (may be overridden by a subclass).
    ///   - The method is not found anywhere in the MRO.
    pub fn resolve_method(&self, class_name: &str, method_name: &str) -> Option<String> {
        if !self.is_leaf_class(class_name) {
            return None;
        }
        let defining_class = self.find_defining_class(class_name, method_name)?;
        Some(format!("{}_{}", defining_class, method_name))
    }

    /// Walk up the MRO (single-inheritance chain) from `class_name` and return
    /// the first class that directly defines `method_name`.
    ///
    /// This is O(depth) per call, which is bounded by the class hierarchy depth.
    fn find_defining_class(&self, class_name: &str, method_name: &str) -> Option<String> {
        let mut current: Option<String> = Some(class_name.to_string());
        while let Some(cls) = current {
            if let Some(method_set) = self.methods.get(&cls) {
                if method_set.contains(method_name) {
                    return Some(cls);
                }
            }
            // Ascend to parent; if not in the map, stop.
            current = self
                .parent
                .get(&cls)
                .and_then(|opt_parent| opt_parent.clone());
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Devirtualization pass
// ---------------------------------------------------------------------------

/// Replace `CallMethod` ops with direct `Call` ops when CHA proves a single
/// concrete target.
///
/// # Attr conventions
/// A `CallMethod` op is expected to carry:
///   - `"method"` → `AttrValue::Str(method_name)` — the method name.
///   - `"receiver_type"` → `AttrValue::Str(class_name)` — optional concrete
///     class of the receiver (set by a prior type-inference pass).
///
/// On devirtualization the op is rewritten to `OpCode::Call` with:
///   - `"callee"` → `AttrValue::Str(resolved_name)` — e.g. `"MyClass_foo"`.
///
/// Operands and results are preserved unchanged (the receiver operand remains
/// as the first argument to the resolved function).
pub fn run(func: &mut TirFunction, hierarchy: &ClassHierarchy) -> PassStats {
    let mut stats = PassStats {
        name: "cha_devirt",
        ..Default::default()
    };

    let block_ids: Vec<_> = func.blocks.keys().copied().collect();

    for bid in block_ids {
        let block = match func.blocks.get_mut(&bid) {
            Some(b) => b,
            None => continue,
        };

        for op in &mut block.ops {
            if op.opcode != OpCode::CallMethod {
                continue;
            }

            // Extract the method name from attrs.
            let method_name = match op.attrs.get("method") {
                Some(AttrValue::Str(m)) => m.clone(),
                _ => continue, // No method attr — cannot devirtualize.
            };

            // Extract the receiver type from attrs.
            let receiver_class = match op.attrs.get("receiver_type") {
                Some(AttrValue::Str(cls)) => cls.clone(),
                _ => continue, // Unknown receiver type — skip.
            };

            // Ask CHA whether this call has a unique target.
            let resolved = match hierarchy.resolve_method(&receiver_class, &method_name) {
                Some(r) => r,
                None => continue, // Not a leaf / not found — keep virtual.
            };

            // Rewrite CallMethod → Call with concrete callee.
            op.opcode = OpCode::Call;
            op.dialect = Dialect::Molt;
            op.attrs
                .insert("callee".to_string(), AttrValue::Str(resolved));
            // Remove the virtual-dispatch attrs to keep the op clean.
            op.attrs.remove("method");
            op.attrs.remove("receiver_type");

            stats.values_changed += 1;
        }
    }

    stats
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::Terminator;
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

    // -----------------------------------------------------------------------
    // Helper builders
    // -----------------------------------------------------------------------

    fn make_call_method(
        method: &str,
        receiver_type: Option<&str>,
        operands: Vec<ValueId>,
        results: Vec<ValueId>,
    ) -> TirOp {
        let mut attrs = AttrDict::new();
        attrs.insert("method".into(), AttrValue::Str(method.to_string()));
        if let Some(cls) = receiver_type {
            attrs.insert("receiver_type".into(), AttrValue::Str(cls.to_string()));
        }
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::CallMethod,
            operands,
            results,
            attrs,
            source_span: None,
        }
    }

    fn build_func_with_ops(ops: Vec<TirOp>) -> TirFunction {
        let mut func = TirFunction::new("test".into(), vec![TirType::DynBox], TirType::DynBox);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = ops;
        entry.terminator = Terminator::Return { values: vec![] };
        func
    }

    fn get_entry_ops(func: &TirFunction) -> &Vec<TirOp> {
        &func.blocks[&func.entry_block].ops
    }

    // -----------------------------------------------------------------------
    // Test 1: Leaf class method call → devirtualized to direct Call
    // -----------------------------------------------------------------------
    #[test]
    fn leaf_class_devirtualized() {
        let mut hierarchy = ClassHierarchy::new();
        hierarchy.add_class("Dog", None, &["bark", "run"]);

        let op = make_call_method("bark", Some("Dog"), vec![ValueId(0)], vec![ValueId(1)]);
        let mut func = build_func_with_ops(vec![op]);

        let stats = run(&mut func, &hierarchy);
        assert_eq!(stats.values_changed, 1);

        let ops = get_entry_ops(&func);
        assert_eq!(ops[0].opcode, OpCode::Call);
        assert_eq!(
            ops[0].attrs.get("callee"),
            Some(&AttrValue::Str("Dog_bark".to_string()))
        );
        // Virtual-dispatch attrs must be removed.
        assert!(!ops[0].attrs.contains_key("method"));
        assert!(!ops[0].attrs.contains_key("receiver_type"));
    }

    // -----------------------------------------------------------------------
    // Test 2: Non-leaf class (has subclasses) → NOT devirtualized
    // -----------------------------------------------------------------------
    #[test]
    fn non_leaf_class_not_devirtualized() {
        let mut hierarchy = ClassHierarchy::new();
        // Animal has a child Dog, so it is NOT a leaf.
        hierarchy.add_class("Animal", None, &["speak"]);
        hierarchy.add_class("Dog", Some("Animal"), &["speak"]);

        let op = make_call_method("speak", Some("Animal"), vec![ValueId(0)], vec![ValueId(1)]);
        let mut func = build_func_with_ops(vec![op]);

        let stats = run(&mut func, &hierarchy);
        assert_eq!(stats.values_changed, 0);

        let ops = get_entry_ops(&func);
        assert_eq!(ops[0].opcode, OpCode::CallMethod);
    }

    // -----------------------------------------------------------------------
    // Test 3: Method inherited from parent → resolved to parent's method
    // -----------------------------------------------------------------------
    #[test]
    fn inherited_method_resolved_to_parent() {
        let mut hierarchy = ClassHierarchy::new();
        // Base defines `greet`; Child inherits it and adds nothing new.
        hierarchy.add_class("Base", None, &["greet"]);
        hierarchy.add_class("Child", Some("Base"), &[]);

        // Child is a leaf (no further subclasses).
        assert!(hierarchy.is_leaf_class("Child"));

        let resolved = hierarchy.resolve_method("Child", "greet");
        assert_eq!(resolved, Some("Base_greet".to_string()));

        // Run the pass to confirm the rewrite happens.
        let op = make_call_method("greet", Some("Child"), vec![ValueId(0)], vec![ValueId(1)]);
        let mut func = build_func_with_ops(vec![op]);

        let stats = run(&mut func, &hierarchy);
        assert_eq!(stats.values_changed, 1);

        let ops = get_entry_ops(&func);
        assert_eq!(ops[0].opcode, OpCode::Call);
        assert_eq!(
            ops[0].attrs.get("callee"),
            Some(&AttrValue::Str("Base_greet".to_string()))
        );
    }

    // -----------------------------------------------------------------------
    // Test 4: No CallMethod ops → no changes
    // -----------------------------------------------------------------------
    #[test]
    fn no_call_method_no_changes() {
        let hierarchy = ClassHierarchy::new();

        let op = TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![ValueId(2)],
            attrs: AttrDict::new(),
            source_span: None,
        };
        let mut func = build_func_with_ops(vec![op]);

        let stats = run(&mut func, &hierarchy);
        assert_eq!(stats.values_changed, 0);
        assert_eq!(get_entry_ops(&func)[0].opcode, OpCode::Add);
    }

    // -----------------------------------------------------------------------
    // Test 5: Unknown receiver type (DynBox / missing attr) → NOT devirtualized
    // -----------------------------------------------------------------------
    #[test]
    fn unknown_receiver_type_not_devirtualized() {
        let mut hierarchy = ClassHierarchy::new();
        hierarchy.add_class("Cat", None, &["meow"]);

        // No receiver_type attr — pass cannot know the class.
        let op = make_call_method("meow", None, vec![ValueId(0)], vec![ValueId(1)]);
        let mut func = build_func_with_ops(vec![op]);

        let stats = run(&mut func, &hierarchy);
        assert_eq!(stats.values_changed, 0);
        assert_eq!(get_entry_ops(&func)[0].opcode, OpCode::CallMethod);
    }

    // -----------------------------------------------------------------------
    // Test 6: 3-class hierarchy — B is leaf, A is not
    //   A (parent, defines "act")
    //   B(A) — leaf (no children)
    //   C      — unrelated, also a leaf
    // -----------------------------------------------------------------------
    #[test]
    fn three_class_hierarchy_leaf_vs_non_leaf() {
        let mut hierarchy = ClassHierarchy::new();
        hierarchy.add_class("A", None, &["act"]);
        hierarchy.add_class("B", Some("A"), &[]);
        hierarchy.add_class("C", None, &["act"]);

        // A has child B → not a leaf.
        assert!(!hierarchy.is_leaf_class("A"));
        // B has no children → leaf.
        assert!(hierarchy.is_leaf_class("B"));
        // C has no children → leaf.
        assert!(hierarchy.is_leaf_class("C"));

        // Call on A → not devirtualized.
        assert!(hierarchy.resolve_method("A", "act").is_none());

        // Call on B → resolved to A's "act" (inherited).
        assert_eq!(
            hierarchy.resolve_method("B", "act"),
            Some("A_act".to_string())
        );

        // Call on C → resolved to C's own "act".
        assert_eq!(
            hierarchy.resolve_method("C", "act"),
            Some("C_act".to_string())
        );

        // Run the pass with two CallMethod ops (one for A, one for B).
        let op_a = make_call_method("act", Some("A"), vec![ValueId(0)], vec![ValueId(1)]);
        let op_b = make_call_method("act", Some("B"), vec![ValueId(2)], vec![ValueId(3)]);
        let mut func = build_func_with_ops(vec![op_a, op_b]);

        let stats = run(&mut func, &hierarchy);
        // Only the B call should be devirtualized.
        assert_eq!(stats.values_changed, 1);

        let ops = get_entry_ops(&func);
        assert_eq!(ops[0].opcode, OpCode::CallMethod); // A — not devirtualized
        assert_eq!(ops[1].opcode, OpCode::Call); // B — devirtualized
        assert_eq!(
            ops[1].attrs.get("callee"),
            Some(&AttrValue::Str("A_act".to_string()))
        );
    }
}
