//! Equality saturation for pure arithmetic expression simplification.
//!
//! This is a proof-of-concept integration of the `egg` e-graph library.
//! It demonstrates algebraic simplification via equality saturation for
//! a restricted expression language (pure integer/float arithmetic).
//!
//! **Status**: Prototype. Not wired into the compilation pipeline.
//! Gated behind the `egraphs` Cargo feature.
//!
//! ## References
//! - egg: <https://egraphs-good.github.io/>
//! - "egg: Fast and Extensible Equality Saturation" (POPL 2021)

#![cfg(feature = "egraphs")]

use egg::{define_language, rewrite, Id, RecExpr, Runner, Extractor, AstSize};

define_language! {
    /// A minimal arithmetic language for e-graph simplification.
    pub enum ArithLang {
        // Constants
        Num(i64),

        // Binary operations
        "+" = Add([Id; 2]),
        "-" = Sub([Id; 2]),
        "*" = Mul([Id; 2]),

        // Unary
        "neg" = Neg([Id; 1]),

        // Variables
        Symbol(egg::Symbol),
    }
}

/// Standard algebraic simplification rules.
pub fn arith_rules() -> Vec<egg::Rewrite<ArithLang, ()>> {
    vec![
        // Additive identity
        rewrite!("add-zero-r"; "(+ ?x 0)" => "?x"),
        rewrite!("add-zero-l"; "(+ 0 ?x)" => "?x"),

        // Multiplicative identity
        rewrite!("mul-one-r"; "(* ?x 1)" => "?x"),
        rewrite!("mul-one-l"; "(* 1 ?x)" => "?x"),

        // Multiplicative annihilation
        rewrite!("mul-zero-r"; "(* ?x 0)" => "0"),
        rewrite!("mul-zero-l"; "(* 0 ?x)" => "0"),

        // Self-subtraction
        rewrite!("sub-self"; "(- ?x ?x)" => "0"),

        // Double negation
        rewrite!("neg-neg"; "(neg (neg ?x))" => "?x"),

        // Commutativity
        rewrite!("add-comm"; "(+ ?x ?y)" => "(+ ?y ?x)"),
        rewrite!("mul-comm"; "(* ?x ?y)" => "(* ?y ?x)"),

        // Associativity
        rewrite!("add-assoc"; "(+ (+ ?x ?y) ?z)" => "(+ ?x (+ ?y ?z))"),
        rewrite!("mul-assoc"; "(* (* ?x ?y) ?z)" => "(* ?x (* ?y ?z))"),

        // Distributivity (limited — can blow up e-graph if unrestricted)
        // rewrite!("distribute"; "(* ?x (+ ?y ?z))" => "(+ (* ?x ?y) (* ?x ?z))"),
    ]
}

/// Simplify an arithmetic expression via equality saturation.
///
/// Takes an s-expression string (e.g., `"(+ x 0)"`) and returns the
/// simplified form (e.g., `"x"`).
///
/// # Example
/// ```ignore
/// assert_eq!(simplify_arith("(+ x 0)"), "x");
/// assert_eq!(simplify_arith("(* y 0)"), "0");
/// assert_eq!(simplify_arith("(- z z)"), "0");
/// ```
pub fn simplify_arith(expr_str: &str) -> String {
    let expr: RecExpr<ArithLang> = expr_str
        .parse()
        .unwrap_or_else(|e| panic!("Failed to parse expression '{}': {}", expr_str, e));

    let runner = Runner::default()
        .with_expr(&expr)
        .run(&arith_rules());

    let extractor = Extractor::new(&runner.egraph, AstSize);
    let (_, best_expr) = extractor.find_best(runner.roots[0]);
    best_expr.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_zero() {
        assert_eq!(simplify_arith("(+ x 0)"), "x");
        assert_eq!(simplify_arith("(+ 0 y)"), "y");
    }

    #[test]
    fn test_mul_identity() {
        assert_eq!(simplify_arith("(* x 1)"), "x");
        assert_eq!(simplify_arith("(* 1 y)"), "y");
    }

    #[test]
    fn test_mul_zero() {
        assert_eq!(simplify_arith("(* x 0)"), "0");
        assert_eq!(simplify_arith("(* 0 y)"), "0");
    }

    #[test]
    fn test_sub_self() {
        assert_eq!(simplify_arith("(- z z)"), "0");
    }

    #[test]
    fn test_double_neg() {
        assert_eq!(simplify_arith("(neg (neg x))"), "x");
    }

    #[test]
    fn test_nested() {
        // (x + 0) * 1 => x
        assert_eq!(simplify_arith("(* (+ x 0) 1)"), "x");
    }

    #[test]
    fn test_literal_fold() {
        // Constants should survive simplification
        assert_eq!(simplify_arith("(+ 0 42)"), "42");
    }
}
