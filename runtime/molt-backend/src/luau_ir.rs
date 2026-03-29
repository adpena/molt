//! Structured Luau IR — an intermediate representation between Molt's SimpleIR
//! and Luau source text. Optimization passes operate on this AST rather than
//! on raw source strings, enabling precise, safe, and composable transforms.

use std::collections::BTreeMap;

/// A Luau literal value.
#[derive(Debug, Clone, PartialEq)]
pub enum LuauLit {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
}

/// A Luau type annotation.
#[derive(Debug, Clone, PartialEq)]
pub enum LuauType {
    Any,
    Number,
    String,
    Boolean,
    Nil,
    Table(Option<Box<LuauType>>),       // {T} for arrays
    Dict(Box<LuauType>, Box<LuauType>), // {[K]: V}
    Function,
    Union(Vec<LuauType>),
}

/// A Luau expression.
#[derive(Debug, Clone)]
pub enum LuauExpr {
    /// Literal value
    Lit(LuauLit),
    /// Variable reference
    Var(String),
    /// Binary operation: lhs op rhs
    BinOp(Box<LuauExpr>, LuauBinOp, Box<LuauExpr>),
    /// Unary operation
    UnOp(LuauUnOp, Box<LuauExpr>),
    /// Table index: table[key]
    Index(Box<LuauExpr>, Box<LuauExpr>),
    /// Field access: obj.field
    Field(Box<LuauExpr>, String),
    /// Function call: func(args...)
    Call(Box<LuauExpr>, Vec<LuauExpr>),
    /// Method call: obj:method(args...)
    MethodCall(Box<LuauExpr>, String, Vec<LuauExpr>),
    /// Table constructor: {items}
    Table(Vec<LuauTableEntry>),
    /// If expression: if cond then true_val else false_val
    IfExpr(Box<LuauExpr>, Box<LuauExpr>, Box<LuauExpr>),
    /// String concatenation: a .. b
    Concat(Box<LuauExpr>, Box<LuauExpr>),
    /// Length operator: #expr
    Len(Box<LuauExpr>),
    /// Type cast: (expr :: type)
    TypeAssert(Box<LuauExpr>, LuauType),
    /// Parenthesized expression (for precedence control)
    Paren(Box<LuauExpr>),
    /// Raw Luau code (escape hatch for complex patterns)
    Raw(String),
}

/// Table constructor entry.
#[derive(Debug, Clone)]
pub enum LuauTableEntry {
    /// Positional: {expr, expr, ...}
    Positional(LuauExpr),
    /// Keyed: {[key] = value}
    Keyed(LuauExpr, LuauExpr),
    /// Named: {name = value}
    Named(String, LuauExpr),
}

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LuauBinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Band,
    Bor,
    Bxor,
    Lshift,
    Rshift,
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LuauUnOp {
    /// -x
    Neg,
    /// not x
    Not,
    /// bit32.bnot(x) — represented as unary for analysis
    Bnot,
}

/// A Luau statement.
#[derive(Debug, Clone)]
pub enum LuauStmt {
    /// Local variable declaration: local name = expr
    Local(String, Option<LuauType>, LuauExpr),
    /// Local declaration without initializer: local name
    LocalDecl(String),
    /// Assignment: target = expr
    Assign(LuauExpr, LuauExpr),
    /// Multi-assignment: a, b, c = expr (for multi-return)
    MultiAssign(Vec<String>, LuauExpr),
    /// If/elseif/else chain
    If {
        cond: LuauExpr,
        then_body: Vec<LuauStmt>,
        elseif_chains: Vec<(LuauExpr, Vec<LuauStmt>)>,
        else_body: Option<Vec<LuauStmt>>,
    },
    /// While loop
    While(LuauExpr, Vec<LuauStmt>),
    /// Numeric for: for var = start, stop, step do body end
    ForNumeric {
        var: String,
        start: LuauExpr,
        stop: LuauExpr,
        step: Option<LuauExpr>,
        body: Vec<LuauStmt>,
    },
    /// Generic for: for vars in iter do body end
    ForGeneric {
        vars: Vec<String>,
        iter: LuauExpr,
        body: Vec<LuauStmt>,
    },
    /// Break
    Break,
    /// Continue
    Continue,
    /// Return with optional values
    Return(Vec<LuauExpr>),
    /// Expression statement (function call)
    ExprStmt(LuauExpr),
    /// Label: ::name::
    Label(String),
    /// Goto: goto name
    Goto(String),
    /// Do block: do body end
    DoBlock(Vec<LuauStmt>),
    /// Comment
    Comment(String),
    /// Raw Luau code (escape hatch)
    Raw(String),
}

/// A Luau function definition.
#[derive(Debug, Clone)]
pub struct LuauFunction {
    pub name: String,
    pub params: Vec<(String, LuauType)>,
    pub return_type: Option<LuauType>,
    pub body: Vec<LuauStmt>,
    pub is_native: bool,
    pub is_local: bool,
}

/// A complete Luau module (transpilation unit).
#[derive(Debug, Clone)]
pub struct LuauModule {
    /// File-level directives (--!strict, --!native)
    pub directives: Vec<String>,
    /// Top-level statements (helpers, module setup)
    pub prelude: Vec<LuauStmt>,
    /// Function definitions
    pub functions: Vec<LuauFunction>,
    /// Entry point code
    pub entry: Vec<LuauStmt>,
}

// ---------------------------------------------------------------------------
// Def-use chain infrastructure
// ---------------------------------------------------------------------------

/// Def-use analysis for LuauIR.
/// Tracks where each variable is defined and used,
/// enabling safe optimization passes.
pub struct DefUseInfo {
    /// Variable -> definition site (function_idx, stmt_idx path)
    pub defs: BTreeMap<String, Vec<DefSite>>,
    /// Variable -> use sites
    pub uses: BTreeMap<String, Vec<UseSite>>,
    /// Variable -> use count (fast path for single-use checks)
    pub use_counts: BTreeMap<String, usize>,
}

#[derive(Debug, Clone)]
pub struct DefSite {
    pub func_idx: usize,
    pub stmt_path: Vec<usize>, // Path through nested blocks
    pub is_local: bool,
}

#[derive(Debug, Clone)]
pub struct UseSite {
    pub func_idx: usize,
    pub stmt_path: Vec<usize>,
}

impl DefUseInfo {
    /// Build def-use info from a LuauModule.
    pub fn analyze(module: &LuauModule) -> Self {
        let mut info = DefUseInfo {
            defs: BTreeMap::new(),
            uses: BTreeMap::new(),
            use_counts: BTreeMap::new(),
        };

        for (func_idx, func) in module.functions.iter().enumerate() {
            // Parameters are definitions
            for (name, _ty) in &func.params {
                info.defs.entry(name.clone()).or_default().push(DefSite {
                    func_idx,
                    stmt_path: vec![],
                    is_local: true,
                });
            }
            info.analyze_stmts(&func.body, func_idx, &mut vec![]);
        }

        info
    }

    fn analyze_stmts(&mut self, stmts: &[LuauStmt], func_idx: usize, path: &mut Vec<usize>) {
        for (i, stmt) in stmts.iter().enumerate() {
            path.push(i);
            self.analyze_stmt(stmt, func_idx, path);
            path.pop();
        }
    }

    fn analyze_stmt(&mut self, stmt: &LuauStmt, func_idx: usize, path: &[usize]) {
        match stmt {
            LuauStmt::Local(name, _, expr) => {
                self.defs.entry(name.clone()).or_default().push(DefSite {
                    func_idx,
                    stmt_path: path.to_vec(),
                    is_local: true,
                });
                self.analyze_expr(expr, func_idx, path);
            }
            LuauStmt::Assign(target, expr) => {
                // The target's variable is a def
                if let LuauExpr::Var(name) = target {
                    self.defs.entry(name.clone()).or_default().push(DefSite {
                        func_idx,
                        stmt_path: path.to_vec(),
                        is_local: false,
                    });
                } else {
                    self.analyze_expr(target, func_idx, path);
                }
                self.analyze_expr(expr, func_idx, path);
            }
            LuauStmt::MultiAssign(vars, expr) => {
                for v in vars {
                    self.defs.entry(v.clone()).or_default().push(DefSite {
                        func_idx,
                        stmt_path: path.to_vec(),
                        is_local: false,
                    });
                }
                self.analyze_expr(expr, func_idx, path);
            }
            LuauStmt::If {
                cond,
                then_body,
                elseif_chains,
                else_body,
            } => {
                self.analyze_expr(cond, func_idx, path);
                let mut p = path.to_vec();
                self.analyze_stmts(then_body, func_idx, &mut p);
                for (econd, ebody) in elseif_chains {
                    self.analyze_expr(econd, func_idx, path);
                    self.analyze_stmts(ebody, func_idx, &mut p);
                }
                if let Some(eb) = else_body {
                    self.analyze_stmts(eb, func_idx, &mut p);
                }
            }
            LuauStmt::While(cond, body) => {
                self.analyze_expr(cond, func_idx, path);
                let mut p = path.to_vec();
                self.analyze_stmts(body, func_idx, &mut p);
            }
            LuauStmt::ForNumeric {
                var,
                start,
                stop,
                step,
                body,
            } => {
                self.defs.entry(var.clone()).or_default().push(DefSite {
                    func_idx,
                    stmt_path: path.to_vec(),
                    is_local: true,
                });
                self.analyze_expr(start, func_idx, path);
                self.analyze_expr(stop, func_idx, path);
                if let Some(s) = step {
                    self.analyze_expr(s, func_idx, path);
                }
                let mut p = path.to_vec();
                self.analyze_stmts(body, func_idx, &mut p);
            }
            LuauStmt::ForGeneric { vars, iter, body } => {
                for v in vars {
                    self.defs.entry(v.clone()).or_default().push(DefSite {
                        func_idx,
                        stmt_path: path.to_vec(),
                        is_local: true,
                    });
                }
                self.analyze_expr(iter, func_idx, path);
                let mut p = path.to_vec();
                self.analyze_stmts(body, func_idx, &mut p);
            }
            LuauStmt::Return(exprs) => {
                for e in exprs {
                    self.analyze_expr(e, func_idx, path);
                }
            }
            LuauStmt::ExprStmt(expr) => {
                self.analyze_expr(expr, func_idx, path);
            }
            LuauStmt::DoBlock(body) => {
                let mut p = path.to_vec();
                self.analyze_stmts(body, func_idx, &mut p);
            }
            LuauStmt::LocalDecl(_)
            | LuauStmt::Break
            | LuauStmt::Continue
            | LuauStmt::Label(_)
            | LuauStmt::Goto(_)
            | LuauStmt::Comment(_)
            | LuauStmt::Raw(_) => {}
        }
    }

    fn analyze_expr(&mut self, expr: &LuauExpr, func_idx: usize, path: &[usize]) {
        match expr {
            LuauExpr::Var(name) => {
                self.uses.entry(name.clone()).or_default().push(UseSite {
                    func_idx,
                    stmt_path: path.to_vec(),
                });
                *self.use_counts.entry(name.clone()).or_insert(0) += 1;
            }
            LuauExpr::BinOp(l, _, r) | LuauExpr::Concat(l, r) => {
                self.analyze_expr(l, func_idx, path);
                self.analyze_expr(r, func_idx, path);
            }
            LuauExpr::UnOp(_, e)
            | LuauExpr::Len(e)
            | LuauExpr::Paren(e)
            | LuauExpr::TypeAssert(e, _) => {
                self.analyze_expr(e, func_idx, path);
            }
            LuauExpr::Index(t, k) => {
                self.analyze_expr(t, func_idx, path);
                self.analyze_expr(k, func_idx, path);
            }
            LuauExpr::Field(obj, _) => {
                self.analyze_expr(obj, func_idx, path);
            }
            LuauExpr::Call(func, args) => {
                self.analyze_expr(func, func_idx, path);
                for a in args {
                    self.analyze_expr(a, func_idx, path);
                }
            }
            LuauExpr::MethodCall(obj, _, args) => {
                self.analyze_expr(obj, func_idx, path);
                for a in args {
                    self.analyze_expr(a, func_idx, path);
                }
            }
            LuauExpr::Table(entries) => {
                for e in entries {
                    match e {
                        LuauTableEntry::Positional(v) => self.analyze_expr(v, func_idx, path),
                        LuauTableEntry::Keyed(k, v) => {
                            self.analyze_expr(k, func_idx, path);
                            self.analyze_expr(v, func_idx, path);
                        }
                        LuauTableEntry::Named(_, v) => self.analyze_expr(v, func_idx, path),
                    }
                }
            }
            LuauExpr::IfExpr(c, t, f) => {
                self.analyze_expr(c, func_idx, path);
                self.analyze_expr(t, func_idx, path);
                self.analyze_expr(f, func_idx, path);
            }
            LuauExpr::Lit(_) | LuauExpr::Raw(_) => {}
        }
    }

    /// Check if a variable is used exactly N times.
    pub fn use_count(&self, var: &str) -> usize {
        self.use_counts.get(var).copied().unwrap_or(0)
    }

    /// Check if a variable is single-use (defined once, used once).
    pub fn is_single_use(&self, var: &str) -> bool {
        self.use_count(var) == 1
    }
}

// ---------------------------------------------------------------------------
// Pretty printer (LuauIR -> Luau source text)
// ---------------------------------------------------------------------------

/// Emit a LuauModule as Luau source text.
pub fn emit_luau(module: &LuauModule) -> String {
    let mut out = String::with_capacity(8192);
    let mut emitter = LuauEmitter {
        out: &mut out,
        indent: 0,
    };

    // Directives
    for d in &module.directives {
        emitter.line(d);
    }
    if !module.directives.is_empty() {
        emitter.out.push('\n');
    }

    // Prelude
    for stmt in &module.prelude {
        emitter.emit_stmt(stmt);
    }
    if !module.prelude.is_empty() {
        emitter.out.push('\n');
    }

    // Functions
    for func in &module.functions {
        emitter.emit_function(func);
        emitter.out.push('\n');
    }

    // Entry
    for stmt in &module.entry {
        emitter.emit_stmt(stmt);
    }

    out
}

struct LuauEmitter<'a> {
    out: &'a mut String,
    indent: usize,
}

impl<'a> LuauEmitter<'a> {
    fn line(&mut self, text: &str) {
        for _ in 0..self.indent {
            self.out.push('\t');
        }
        self.out.push_str(text);
        self.out.push('\n');
    }

    fn emit_function(&mut self, func: &LuauFunction) {
        if func.is_native {
            self.line("@native");
        }
        let params: Vec<String> = func
            .params
            .iter()
            .map(|(name, ty)| format!("{}: {}", name, fmt_type(ty)))
            .collect();
        let params_str = params.join(", ");

        if func.is_local {
            self.line(&format!("local function {}({})", func.name, params_str));
        } else {
            self.line(&format!("{} = function({})", func.name, params_str));
        }
        self.indent += 1;
        for stmt in &func.body {
            self.emit_stmt(stmt);
        }
        self.indent -= 1;
        self.line("end");
    }

    fn emit_stmt(&mut self, stmt: &LuauStmt) {
        match stmt {
            LuauStmt::Local(name, ty, expr) => {
                let ty_str = ty
                    .as_ref()
                    .map(|t| format!(": {}", fmt_type(t)))
                    .unwrap_or_default();
                self.line(&format!("local {}{} = {}", name, ty_str, fmt_expr(expr)));
            }
            LuauStmt::LocalDecl(name) => {
                self.line(&format!("local {}", name));
            }
            LuauStmt::Assign(target, expr) => {
                self.line(&format!("{} = {}", fmt_expr(target), fmt_expr(expr)));
            }
            LuauStmt::MultiAssign(vars, expr) => {
                self.line(&format!("{} = {}", vars.join(", "), fmt_expr(expr)));
            }
            LuauStmt::If {
                cond,
                then_body,
                elseif_chains,
                else_body,
            } => {
                self.line(&format!("if {} then", fmt_expr(cond)));
                self.indent += 1;
                for s in then_body {
                    self.emit_stmt(s);
                }
                self.indent -= 1;
                for (econd, ebody) in elseif_chains {
                    self.line(&format!("elseif {} then", fmt_expr(econd)));
                    self.indent += 1;
                    for s in ebody {
                        self.emit_stmt(s);
                    }
                    self.indent -= 1;
                }
                if let Some(eb) = else_body {
                    self.line("else");
                    self.indent += 1;
                    for s in eb {
                        self.emit_stmt(s);
                    }
                    self.indent -= 1;
                }
                self.line("end");
            }
            LuauStmt::While(cond, body) => {
                self.line(&format!("while {} do", fmt_expr(cond)));
                self.indent += 1;
                for s in body {
                    self.emit_stmt(s);
                }
                self.indent -= 1;
                self.line("end");
            }
            LuauStmt::ForNumeric {
                var,
                start,
                stop,
                step,
                body,
            } => {
                let step_str = step
                    .as_ref()
                    .map(|s| format!(", {}", fmt_expr(s)))
                    .unwrap_or_default();
                self.line(&format!(
                    "for {} = {}, {}{} do",
                    var,
                    fmt_expr(start),
                    fmt_expr(stop),
                    step_str
                ));
                self.indent += 1;
                for s in body {
                    self.emit_stmt(s);
                }
                self.indent -= 1;
                self.line("end");
            }
            LuauStmt::ForGeneric { vars, iter, body } => {
                self.line(&format!("for {} in {} do", vars.join(", "), fmt_expr(iter)));
                self.indent += 1;
                for s in body {
                    self.emit_stmt(s);
                }
                self.indent -= 1;
                self.line("end");
            }
            LuauStmt::Break => self.line("break"),
            LuauStmt::Continue => self.line("continue"),
            LuauStmt::Return(exprs) => {
                if exprs.is_empty() {
                    self.line("return");
                } else {
                    let vals: Vec<String> = exprs.iter().map(fmt_expr).collect();
                    self.line(&format!("return {}", vals.join(", ")));
                }
            }
            LuauStmt::ExprStmt(expr) => {
                self.line(&fmt_expr(expr));
            }
            LuauStmt::Label(name) => {
                self.line(&format!("::{}::", name));
            }
            LuauStmt::Goto(name) => {
                self.line(&format!("goto {}", name));
            }
            LuauStmt::DoBlock(body) => {
                self.line("do");
                self.indent += 1;
                for s in body {
                    self.emit_stmt(s);
                }
                self.indent -= 1;
                self.line("end");
            }
            LuauStmt::Comment(text) => {
                self.line(&format!("-- {}", text));
            }
            LuauStmt::Raw(code) => {
                self.line(code);
            }
        }
    }
}

fn fmt_expr(expr: &LuauExpr) -> String {
    match expr {
        LuauExpr::Lit(lit) => match lit {
            LuauLit::Nil => "nil".to_string(),
            LuauLit::Bool(b) => if *b { "true" } else { "false" }.to_string(),
            LuauLit::Int(n) => n.to_string(),
            LuauLit::Float(f) => format!("{f}"),
            LuauLit::Str(s) => format!("\"{}\"", escape_luau_str(s)),
        },
        LuauExpr::Var(name) => name.clone(),
        LuauExpr::BinOp(l, op, r) => {
            match op {
                LuauBinOp::Band => {
                    format!("bit32.band({}, {})", fmt_expr(l), fmt_expr(r))
                }
                LuauBinOp::Bor => format!("bit32.bor({}, {})", fmt_expr(l), fmt_expr(r)),
                LuauBinOp::Bxor => {
                    format!("bit32.bxor({}, {})", fmt_expr(l), fmt_expr(r))
                }
                LuauBinOp::Lshift => {
                    format!("bit32.lshift({}, {})", fmt_expr(l), fmt_expr(r))
                }
                LuauBinOp::Rshift => {
                    format!("bit32.rshift({}, {})", fmt_expr(l), fmt_expr(r))
                }
                _ => {
                    let op_str = match op {
                        LuauBinOp::Add => "+",
                        LuauBinOp::Sub => "-",
                        LuauBinOp::Mul => "*",
                        LuauBinOp::Div => "/",
                        LuauBinOp::Mod => "%",
                        LuauBinOp::Pow => "^",
                        LuauBinOp::Eq => "==",
                        LuauBinOp::Ne => "~=",
                        LuauBinOp::Lt => "<",
                        LuauBinOp::Le => "<=",
                        LuauBinOp::Gt => ">",
                        LuauBinOp::Ge => ">=",
                        LuauBinOp::And => "and",
                        LuauBinOp::Or => "or",
                        // Bitwise ops handled above
                        _ => unreachable!(),
                    };
                    format!("{} {} {}", fmt_expr(l), op_str, fmt_expr(r))
                }
            }
        }
        LuauExpr::UnOp(op, e) => match op {
            LuauUnOp::Neg => format!("-{}", fmt_expr(e)),
            LuauUnOp::Not => format!("not {}", fmt_expr(e)),
            LuauUnOp::Bnot => format!("bit32.bnot({})", fmt_expr(e)),
        },
        LuauExpr::Index(t, k) => format!("{}[{}]", fmt_expr(t), fmt_expr(k)),
        LuauExpr::Field(obj, field) => format!("{}.{}", fmt_expr(obj), field),
        LuauExpr::Call(func, args) => {
            let args_str: Vec<String> = args.iter().map(fmt_expr).collect();
            format!("{}({})", fmt_expr(func), args_str.join(", "))
        }
        LuauExpr::MethodCall(obj, method, args) => {
            let args_str: Vec<String> = args.iter().map(fmt_expr).collect();
            format!("{}:{}({})", fmt_expr(obj), method, args_str.join(", "))
        }
        LuauExpr::Table(entries) => {
            if entries.is_empty() {
                return "{}".to_string();
            }
            let parts: Vec<String> = entries
                .iter()
                .map(|e| match e {
                    LuauTableEntry::Positional(v) => fmt_expr(v),
                    LuauTableEntry::Keyed(k, v) => {
                        format!("[{}] = {}", fmt_expr(k), fmt_expr(v))
                    }
                    LuauTableEntry::Named(n, v) => format!("{} = {}", n, fmt_expr(v)),
                })
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
        LuauExpr::IfExpr(c, t, f) => {
            format!(
                "if {} then {} else {}",
                fmt_expr(c),
                fmt_expr(t),
                fmt_expr(f)
            )
        }
        LuauExpr::Concat(l, r) => format!("{} .. {}", fmt_expr(l), fmt_expr(r)),
        LuauExpr::Len(e) => format!("#{}", fmt_expr(e)),
        LuauExpr::TypeAssert(e, ty) => format!("{} :: {}", fmt_expr(e), fmt_type(ty)),
        LuauExpr::Paren(e) => format!("({})", fmt_expr(e)),
        LuauExpr::Raw(code) => code.clone(),
    }
}

fn fmt_type(ty: &LuauType) -> String {
    match ty {
        LuauType::Any => "any".to_string(),
        LuauType::Number => "number".to_string(),
        LuauType::String => "string".to_string(),
        LuauType::Boolean => "boolean".to_string(),
        LuauType::Nil => "nil".to_string(),
        LuauType::Table(inner) => match inner {
            Some(t) => format!("{{{}}}", fmt_type(t)),
            None => "{any}".to_string(),
        },
        LuauType::Dict(k, v) => format!("{{[{}]: {}}}", fmt_type(k), fmt_type(v)),
        LuauType::Function => "((...any) -> ...any)".to_string(),
        LuauType::Union(types) => types.iter().map(fmt_type).collect::<Vec<_>>().join(" | "),
    }
}

fn escape_luau_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            _ => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_emit_simple_function() {
        let module = LuauModule {
            directives: vec!["--!strict".to_string(), "--!native".to_string()],
            prelude: vec![],
            functions: vec![LuauFunction {
                name: "molt_main".to_string(),
                params: vec![],
                return_type: None,
                body: vec![
                    LuauStmt::Local(
                        "x".to_string(),
                        Some(LuauType::Number),
                        LuauExpr::Lit(LuauLit::Int(42)),
                    ),
                    LuauStmt::ExprStmt(LuauExpr::Call(
                        Box::new(LuauExpr::Var("print".to_string())),
                        vec![LuauExpr::Var("x".to_string())],
                    )),
                ],
                is_native: true,
                is_local: true,
            }],
            entry: vec![LuauStmt::ExprStmt(LuauExpr::Call(
                Box::new(LuauExpr::Var("molt_main".to_string())),
                vec![],
            ))],
        };

        let output = emit_luau(&module);
        assert!(output.contains("--!strict"));
        assert!(output.contains("--!native"));
        assert!(output.contains("@native"));
        assert!(output.contains("local function molt_main()"));
        assert!(output.contains("local x: number = 42"));
        assert!(output.contains("print(x)"));
        assert!(output.contains("molt_main()"));
    }

    #[test]
    fn test_def_use_analysis() {
        let module = LuauModule {
            directives: vec![],
            prelude: vec![],
            functions: vec![LuauFunction {
                name: "test".to_string(),
                params: vec![("n".to_string(), LuauType::Number)],
                return_type: None,
                body: vec![
                    LuauStmt::Local(
                        "x".to_string(),
                        None,
                        LuauExpr::BinOp(
                            Box::new(LuauExpr::Var("n".to_string())),
                            LuauBinOp::Add,
                            Box::new(LuauExpr::Lit(LuauLit::Int(1))),
                        ),
                    ),
                    LuauStmt::Return(vec![LuauExpr::Var("x".to_string())]),
                ],
                is_native: true,
                is_local: true,
            }],
            entry: vec![],
        };

        let info = DefUseInfo::analyze(&module);
        assert_eq!(info.use_count("n"), 1); // Used once in x = n + 1
        assert_eq!(info.use_count("x"), 1); // Used once in return x
        assert!(info.is_single_use("x"));
    }

    #[test]
    fn test_fmt_expressions() {
        let expr = LuauExpr::BinOp(
            Box::new(LuauExpr::Var("a".to_string())),
            LuauBinOp::Add,
            Box::new(LuauExpr::Var("b".to_string())),
        );
        assert_eq!(fmt_expr(&expr), "a + b");

        let idx = LuauExpr::Index(
            Box::new(LuauExpr::Var("t".to_string())),
            Box::new(LuauExpr::BinOp(
                Box::new(LuauExpr::Var("i".to_string())),
                LuauBinOp::Add,
                Box::new(LuauExpr::Lit(LuauLit::Int(1))),
            )),
        );
        assert_eq!(fmt_expr(&idx), "t[i + 1]");

        let tbl = LuauExpr::Table(vec![
            LuauTableEntry::Positional(LuauExpr::Lit(LuauLit::Int(1))),
            LuauTableEntry::Positional(LuauExpr::Lit(LuauLit::Int(2))),
        ]);
        assert_eq!(fmt_expr(&tbl), "{1, 2}");
    }

    #[test]
    fn test_emit_if_elseif_else() {
        let module = LuauModule {
            directives: vec![],
            prelude: vec![],
            functions: vec![LuauFunction {
                name: "check".to_string(),
                params: vec![("x".to_string(), LuauType::Number)],
                return_type: None,
                body: vec![LuauStmt::If {
                    cond: LuauExpr::BinOp(
                        Box::new(LuauExpr::Var("x".to_string())),
                        LuauBinOp::Gt,
                        Box::new(LuauExpr::Lit(LuauLit::Int(0))),
                    ),
                    then_body: vec![LuauStmt::Return(vec![LuauExpr::Lit(LuauLit::Str(
                        "positive".to_string(),
                    ))])],
                    elseif_chains: vec![(
                        LuauExpr::BinOp(
                            Box::new(LuauExpr::Var("x".to_string())),
                            LuauBinOp::Lt,
                            Box::new(LuauExpr::Lit(LuauLit::Int(0))),
                        ),
                        vec![LuauStmt::Return(vec![LuauExpr::Lit(LuauLit::Str(
                            "negative".to_string(),
                        ))])],
                    )],
                    else_body: Some(vec![LuauStmt::Return(vec![LuauExpr::Lit(LuauLit::Str(
                        "zero".to_string(),
                    ))])]),
                }],
                is_native: false,
                is_local: true,
            }],
            entry: vec![],
        };

        let output = emit_luau(&module);
        assert!(output.contains("if x > 0 then"));
        assert!(output.contains("elseif x < 0 then"));
        assert!(output.contains("else"));
        assert!(output.contains("end"));
    }

    #[test]
    fn test_emit_for_loops() {
        let module = LuauModule {
            directives: vec![],
            prelude: vec![],
            functions: vec![LuauFunction {
                name: "loops".to_string(),
                params: vec![],
                return_type: None,
                body: vec![
                    LuauStmt::ForNumeric {
                        var: "i".to_string(),
                        start: LuauExpr::Lit(LuauLit::Int(1)),
                        stop: LuauExpr::Lit(LuauLit::Int(10)),
                        step: None,
                        body: vec![LuauStmt::ExprStmt(LuauExpr::Call(
                            Box::new(LuauExpr::Var("print".to_string())),
                            vec![LuauExpr::Var("i".to_string())],
                        ))],
                    },
                    LuauStmt::ForGeneric {
                        vars: vec!["k".to_string(), "v".to_string()],
                        iter: LuauExpr::Call(
                            Box::new(LuauExpr::Var("pairs".to_string())),
                            vec![LuauExpr::Var("t".to_string())],
                        ),
                        body: vec![LuauStmt::Break],
                    },
                ],
                is_native: false,
                is_local: true,
            }],
            entry: vec![],
        };

        let output = emit_luau(&module);
        assert!(output.contains("for i = 1, 10 do"));
        assert!(output.contains("for k, v in pairs(t) do"));
        assert!(output.contains("break"));
    }

    #[test]
    fn test_emit_bitwise_ops() {
        let band = LuauExpr::BinOp(
            Box::new(LuauExpr::Var("a".to_string())),
            LuauBinOp::Band,
            Box::new(LuauExpr::Var("b".to_string())),
        );
        assert_eq!(fmt_expr(&band), "bit32.band(a, b)");

        let bnot = LuauExpr::UnOp(LuauUnOp::Bnot, Box::new(LuauExpr::Var("x".to_string())));
        assert_eq!(fmt_expr(&bnot), "bit32.bnot(x)");
    }

    #[test]
    fn test_emit_method_call() {
        let mc = LuauExpr::MethodCall(
            Box::new(LuauExpr::Var("obj".to_string())),
            "foo".to_string(),
            vec![
                LuauExpr::Lit(LuauLit::Int(1)),
                LuauExpr::Lit(LuauLit::Int(2)),
            ],
        );
        assert_eq!(fmt_expr(&mc), "obj:foo(1, 2)");
    }

    #[test]
    fn test_emit_type_annotations() {
        assert_eq!(fmt_type(&LuauType::Any), "any");
        assert_eq!(fmt_type(&LuauType::Number), "number");
        assert_eq!(
            fmt_type(&LuauType::Table(Some(Box::new(LuauType::Number)))),
            "{number}"
        );
        assert_eq!(
            fmt_type(&LuauType::Dict(
                Box::new(LuauType::String),
                Box::new(LuauType::Any)
            )),
            "{[string]: any}"
        );
        assert_eq!(fmt_type(&LuauType::Function), "((...any) -> ...any)");
        assert_eq!(
            fmt_type(&LuauType::Union(vec![LuauType::Number, LuauType::Nil])),
            "number | nil"
        );
    }

    #[test]
    fn test_escape_luau_str() {
        assert_eq!(escape_luau_str("hello"), "hello");
        assert_eq!(escape_luau_str("he\"llo"), "he\\\"llo");
        assert_eq!(escape_luau_str("line\nnext"), "line\\nnext");
        assert_eq!(escape_luau_str("tab\there"), "tab\\there");
        assert_eq!(escape_luau_str("back\\slash"), "back\\\\slash");
    }

    #[test]
    fn test_def_use_multi_use() {
        let module = LuauModule {
            directives: vec![],
            prelude: vec![],
            functions: vec![LuauFunction {
                name: "test".to_string(),
                params: vec![],
                return_type: None,
                body: vec![
                    LuauStmt::Local("x".to_string(), None, LuauExpr::Lit(LuauLit::Int(1))),
                    LuauStmt::ExprStmt(LuauExpr::BinOp(
                        Box::new(LuauExpr::Var("x".to_string())),
                        LuauBinOp::Add,
                        Box::new(LuauExpr::Var("x".to_string())),
                    )),
                ],
                is_native: false,
                is_local: true,
            }],
            entry: vec![],
        };

        let info = DefUseInfo::analyze(&module);
        assert_eq!(info.use_count("x"), 2);
        assert!(!info.is_single_use("x"));
    }
}
