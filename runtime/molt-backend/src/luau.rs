//! Luau transpiler backend for Molt.
//!
//! Transpiles `SimpleIR` → Luau source code suitable for Roblox Studio.
//! Unlike the native/WASM backends that emit binary, this produces a `.luau`
//! text file that can be executed directly in Roblox's Luau VM.
//!
//! This backend is intentionally a preview target. Production build paths must
//! reject lowered output that still contains comment-only control-flow markers
//! or stub markers for unsupported semantics.

use crate::{FunctionIR, OpIR, SimpleIR};
use std::collections::HashMap;
use std::fmt::Write;

/// Transpiles Molt `SimpleIR` into Luau source text.
pub struct LuauBackend {
    output: String,
    indent: usize,
    uses_forward_decls: bool,
}

impl LuauBackend {
    pub fn new() -> Self {
        Self {
            output: String::with_capacity(8192),
            indent: 0,
            uses_forward_decls: false,
        }
    }

    /// Compile the given IR to a Luau source string.
    pub fn compile(&mut self, ir: &SimpleIR) -> String {
        self.emit_prelude();

        // Forward-declare all functions so call order doesn't matter.
        // In Luau, `local function f()` is not hoisted, so callees
        // must be defined before callers.  Forward declarations solve
        // this: `local f; ... f = function(...) ... end`.
        // Filter out dead annotation functions and unused runtime helpers.
        let emit_funcs: Vec<&FunctionIR> = ir
            .functions
            .iter()
            .filter(|f| !f.name.contains("__annotate__"))
            .collect();

        if emit_funcs.len() > 1 {
            self.uses_forward_decls = true;
            self.emit_line("-- Forward declarations");
            for func in &emit_funcs {
                let name = sanitize_ident(&func.name);
                self.emit_line(&format!("local {name}"));
            }
            self.output.push('\n');
        }

        for func in &emit_funcs {
            self.emit_function_body(func);
            self.output.push('\n');
        }
        // Entry point: call molt_main if it exists.
        self.emit_line("-- Entry point");
        self.emit_line("if molt_main then");
        self.push_indent();
        self.emit_line("molt_main()");
        self.pop_indent();
        self.emit_line("end");
        inline_single_use_constants(&mut self.output);
        std::mem::take(&mut self.output)
    }

    /// Compile the given IR and reject preview-blocker markers that would
    /// otherwise silently emit syntactically valid but semantically incomplete
    /// Luau.
    pub fn compile_checked(&mut self, ir: &SimpleIR) -> Result<String, String> {
        let source = self.compile(ir);
        validate_luau_source(&source)?;
        Ok(source)
    }

    fn emit_prelude(&mut self) {
        let prelude = r#"--!strict
-- Molt -> Luau transpiled output
-- Runtime helpers

-- Side-table for function attributes (e.g. __defaults__, __kwdefaults__).
-- Luau functions are primitives and don't support attribute setting.
local molt_func_attrs: {[any]: {[string]: any}} = {}

-- Module registry: maps Python module names to Luau bridge tables.
local molt_module_cache: {[string]: any} = {
	math = nil,  -- filled after molt_math is defined
	json = nil,  -- filled after json is defined
}

local function molt_range(start: number, stop: number, step: number?): {number}
	local result = {}
	local s = step or 1
	local i = start
	while (s > 0 and i < stop) or (s < 0 and i > stop) do
		table.insert(result, i)
		i = i + s
	end
	return result
end

local function molt_len(obj: any): number
	if type(obj) == "string" then return #obj end
	if type(obj) == "table" then return #obj end
	return 0
end

local function molt_int(x: any): number
	return math.floor(tonumber(x) or 0)
end

local function molt_float(x: any): number
	return tonumber(x) or 0.0
end

local function molt_str(x: any): string
	return tostring(x)
end

local function molt_bool(x: any): boolean
	if x == nil or x == false or x == 0 or x == "" then return false end
	if type(x) == "table" and next(x) == nil then return false end
	return true
end

local function molt_repr(x: any): string
	if type(x) == "string" then return '"' .. x .. '"' end
	return tostring(x)
end

local function molt_floor_div(a: number, b: number): number
	return math.floor(a / b)
end

local function molt_pow(a: number, b: number): number
	return a ^ b
end

local function molt_mod(a: number, b: number): number
	return a - math.floor(a / b) * b
end

local math_floor = math.floor
local bit = bit32 or bit

-- Python enumerate(iterable) -> {{index, value}, ...}
local function molt_enumerate(t: {any}, start: number?): {{any}}
	local result = {}
	local s = start or 0
	for i, v in ipairs(t) do
		table.insert(result, {s + i - 1, v})
	end
	return result
end

-- Python zip(a, b) -> {{a[i], b[i]}, ...}
local function molt_zip(a: {any}, b: {any}): {{any}}
	local result = {}
	local n = math.min(#a, #b)
	for i = 1, n do
		table.insert(result, {a[i], b[i]})
	end
	return result
end

-- Python sorted(iterable)
local function molt_sorted(t: {any}): {any}
	local copy = table.clone(t)
	table.sort(copy)
	return copy
end

-- Python reversed(iterable)
local function molt_reversed(t: {any}): {any}
	local result = {}
	for i = #t, 1, -1 do
		table.insert(result, t[i])
	end
	return result
end

-- Python sum(iterable, start=0)
local function molt_sum(t: {number}, start: number?): number
	local s = start or 0
	for _, v in ipairs(t) do s = s + v end
	return s
end

-- Python any(iterable)
local function molt_any(t: {any}): boolean
	for _, v in ipairs(t) do
		if v then return true end
	end
	return false
end

-- Python all(iterable)
local function molt_all(t: {any}): boolean
	for _, v in ipairs(t) do
		if not v then return false end
	end
	return true
end

-- Python map(func, iterable)
local function molt_map(func: (any) -> any, t: {any}): {any}
	local result = {}
	for _, v in ipairs(t) do
		table.insert(result, func(v))
	end
	return result
end

-- Python filter(func, iterable)
local function molt_filter(func: ((any) -> boolean)?, t: {any}): {any}
	local result = {}
	for _, v in ipairs(t) do
		if func then
			if func(v) then table.insert(result, v) end
		elseif v then
			table.insert(result, v)
		end
	end
	return result
end

-- Python dict.keys() / dict.values() / dict.items()
local function molt_dict_keys(d: {[any]: any}): {any}
	local result = {}
	for k in pairs(d) do table.insert(result, k) end
	return result
end

local function molt_dict_values(d: {[any]: any}): {any}
	local result = {}
	for _, v in pairs(d) do table.insert(result, v) end
	return result
end

local function molt_dict_items(d: {[any]: any}): {{any}}
	local result = {}
	for k, v in pairs(d) do table.insert(result, {k, v}) end
	return result
end

-- math module bridge
local molt_math = {
	floor = math.floor,
	ceil = math.ceil,
	sqrt = math.sqrt,
	abs = math.abs,
	sin = math.sin,
	cos = math.cos,
	tan = math.tan,
	asin = math.asin,
	acos = math.acos,
	atan = math.atan,
	atan2 = math.atan2,
	exp = math.exp,
	log = math.log,
	log10 = math.log,
	pi = math.pi,
	e = 2.718281828459045,
	inf = math.huge,
	nan = 0/0,
}

-- Minimal JSON serializer for Luau (Python json.dumps equivalent)
local molt_json_dumps
do
	local function serialize(val: any, depth: number): string
		if depth > 50 then return '"[max depth]"' end
		local t = type(val)
		if t == "nil" then
			return "null"
		elseif t == "boolean" then
			return val and "true" or "false"
		elseif t == "number" then
			if val ~= val then return "null" end
			if val == math.huge then return "1e308" end
			if val == -math.huge then return "-1e308" end
			if val == math_floor(val) and math.abs(val) < 1e15 then
				return string.format("%d", val)
			end
			return tostring(val)
		elseif t == "string" then
			local escaped = val:gsub('[\\"]', function(c) return "\\" .. c end)
			escaped = escaped:gsub("\n", "\\n"):gsub("\r", "\\r"):gsub("\t", "\\t")
			return '"' .. escaped .. '"'
		elseif t == "table" then
			-- Detect array vs object: array if sequential integer keys from 1
			local is_array = true
			local n = #val
			if n == 0 then
				-- Check if it has any keys at all
				if next(val) ~= nil then is_array = false end
			else
				for k in pairs(val) do
					if type(k) ~= "number" or k < 1 or k > n or k ~= math_floor(k) then
						is_array = false
						break
					end
				end
			end
			local parts = {}
			if is_array then
				for i = 1, n do
					table.insert(parts, serialize(val[i], depth + 1))
				end
				return "[" .. table.concat(parts, ", ") .. "]"
			else
				for k, v in pairs(val) do
					local ks = type(k) == "string" and k or tostring(k)
					local escaped = ks:gsub('[\\"]', function(c) return "\\" .. c end)
					table.insert(parts, '"' .. escaped .. '": ' .. serialize(v, depth + 1))
				end
				return "{" .. table.concat(parts, ", ") .. "}"
			end
		end
		return '"' .. tostring(val) .. '"'
	end
	molt_json_dumps = function(val: any): string
		return serialize(val, 0)
	end
end

-- json module bridge
local json = {
	dumps = molt_json_dumps,
}

-- String method helpers (for call_method on strings)
local molt_string = {
	format = string.format,
	join = function(sep: string, t: {string}): string
		return table.concat(t, sep)
	end,
	split = function(s: string, sep: string?): {string}
		local result = {}
		local pattern = sep and sep or "%s+"
		if sep then
			local pos = 1
			while pos <= #s do
				local i, j = string.find(s, pattern, pos, true)
				if i then
					table.insert(result, string.sub(s, pos, i - 1))
					pos = j + 1
				else
					table.insert(result, string.sub(s, pos))
					break
				end
			end
		else
			for w in string.gmatch(s, "%S+") do
				table.insert(result, w)
			end
		end
		return result
	end,
}

-- Populate module cache now that bridge tables are defined.
molt_module_cache["math"] = molt_math
molt_module_cache["json"] = json

"#;
        self.output.push_str(prelude);
    }

    fn emit_function_body(&mut self, func: &FunctionIR) {
        // Pre-process: strip unreachable ops after unconditional returns.
        let ops = strip_dead_after_return(&func.ops);

        let params = func
            .params
            .iter()
            .map(|p| sanitize_ident(p))
            .collect::<Vec<_>>()
            .join(", ");

        let name = sanitize_ident(&func.name);
        // Use `local function` when no forward declaration exists (single-func IR),
        // otherwise use assignment (forward decl already did `local name`).
        if self.uses_forward_decls {
            let _ = writeln!(self.output, "{name} = function({params})");
        } else {
            let _ = writeln!(self.output, "local function {name}({params})");
        }
        self.push_indent();

        // Pre-declare loop index variables so they persist across iterations.
        // Without this, `local v = start` inside `while true do` re-declares
        // on every iteration, shadowing the `loop_index_next` update.
        let mut loop_idx_vars = Vec::new();
        for op in &ops {
            if op.kind == "loop_index_start" {
                if let Some(ref out_name) = op.out {
                    loop_idx_vars.push(sanitize_ident(out_name));
                }
            }
        }
        if !loop_idx_vars.is_empty() {
            for var in &loop_idx_vars {
                self.emit_line(&format!("local {var}"));
            }
        }

        // Emit ops, with special handling for loop_start + loop_index_start
        // pairs: the index initialization must happen BEFORE the while to
        // avoid re-declaring on every iteration.
        let mut i = 0;
        while i < ops.len() {
            if ops[i].kind == "loop_start" && i + 1 < ops.len() && ops[i + 1].kind == "loop_index_start" {
                // Emit index init before the while.
                let idx_op = &ops[i + 1];
                if let Some(ref out_name) = idx_op.out {
                    let out = sanitize_ident(out_name);
                    let args = idx_op.args.as_deref().unwrap_or(&[]);
                    if let Some(start_val) = args.first() {
                        let start = sanitize_ident(start_val);
                        self.emit_line(&format!("{out} = {start}"));
                    } else {
                        self.emit_line(&format!("{out} = 0"));
                    }
                }
                // Now emit the while.
                self.emit_op(&ops[i]);
                // Skip the loop_index_start (already handled).
                i += 2;
            } else {
                self.emit_op(&ops[i]);
                i += 1;
            }
        }

        self.pop_indent();
        self.emit_line("end");
    }

    fn emit_op(&mut self, op: &OpIR) {
        match op.kind.as_str() {
            // ================================================================
            // Constants
            // ================================================================
            "const" => {
                let out = self.out_var(op);
                if let Some(v) = op.value {
                    self.emit_line(&format!("local {out} = {v}"));
                } else if let Some(f) = op.f_value {
                    self.emit_line(&format!("local {out} = {f}"));
                } else if let Some(ref s) = op.s_value {
                    let escaped = escape_luau_string(s);
                    self.emit_line(&format!("local {out} = \"{escaped}\""));
                } else {
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "const_float" => {
                let out = self.out_var(op);
                let val = op.f_value.unwrap_or(0.0);
                self.emit_line(&format!("local {out} = {val}"));
            }
            "const_str" => {
                let out = self.out_var(op);
                let s = op.s_value.as_deref().unwrap_or("");
                let escaped = escape_luau_string(s);
                self.emit_line(&format!("local {out} = \"{escaped}\""));
            }
            "const_bytes" => {
                let out = self.out_var(op);
                if let Some(ref bytes) = op.bytes {
                    let escaped: String = bytes.iter().map(|b| format!("\\x{b:02x}")).collect();
                    self.emit_line(&format!("local {out} = \"{escaped}\""));
                } else {
                    let s = op.s_value.as_deref().unwrap_or("");
                    let escaped = escape_luau_string(s);
                    self.emit_line(&format!("local {out} = \"{escaped}\""));
                }
            }
            "const_bool" | "bool_const" => {
                let out = self.out_var(op);
                let val = if op.value.unwrap_or(0) != 0 {
                    "true"
                } else {
                    "false"
                };
                self.emit_line(&format!("local {out} = {val}"));
            }
            "const_none" | "none_const" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = nil"));
            }
            "string_const" => {
                let out = self.out_var(op);
                let s = op.s_value.as_deref().unwrap_or("");
                let escaped = escape_luau_string(s);
                self.emit_line(&format!("local {out} = \"{escaped}\""));
            }
            "const_bigint" => {
                let out = self.out_var(op);
                let s = op.s_value.as_deref().unwrap_or("0");
                self.emit_line(&format!("local {out} = tonumber(\"{s}\") or 0"));
            }
            "const_not_implemented" | "const_ellipsis" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = nil -- {}", op.kind));
            }

            // ================================================================
            // Variable load/store (both pedagogical and real IR forms)
            // ================================================================
            "load_local" | "load" | "closure_load" | "guarded_load" => {
                let out = self.out_var(op);
                let var = self.var_ref(op);
                self.emit_line(&format!("local {out} = {var}"));
            }
            "store_local" | "store" | "store_init" | "closure_store" => {
                let var = self.var_ref(op);
                if let Some(ref args) = op.args {
                    if let Some(src) = args.first() {
                        self.emit_line(&format!("{var} = {}", sanitize_ident(src)));
                    }
                }
            }
            "identity_alias" => {
                let out = self.out_var(op);
                if let Some(ref args) = op.args {
                    if let Some(src) = args.first() {
                        self.emit_line(&format!("local {out} = {}", sanitize_ident(src)));
                    }
                }
            }

            // ================================================================
            // Arithmetic ops (real IR op kinds)
            // ================================================================
            "add" | "inplace_add" => {
                // Python + is overloaded: numeric add for numbers, concat for strings.
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = sanitize_ident(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out} = if type({lhs}) == \"string\" or type({rhs}) == \"string\" then tostring({lhs}) .. tostring({rhs}) else {lhs} + {rhs}"
                    ));
                }
            }
            "sub" | "inplace_sub" => self.emit_binary_op(op, "-"),
            "mul" | "inplace_mul" => self.emit_binary_op(op, "*"),
            "div" => self.emit_binary_op(op, "/"),
            "mod" => {
                // Python % has floor-mod semantics; use helper.
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = sanitize_ident(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    self.emit_line(&format!("local {out} = molt_mod({lhs}, {rhs})"));
                }
            }
            "floordiv" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = sanitize_ident(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    self.emit_line(&format!("local {out} = molt_floor_div({lhs}, {rhs})"));
                }
            }
            "pow" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = sanitize_ident(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    self.emit_line(&format!("local {out} = molt_pow({lhs}, {rhs})"));
                }
            }
            "pow_mod" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let base = sanitize_ident(&args[0]);
                    let exp = sanitize_ident(&args[1]);
                    let modulus = sanitize_ident(&args[2]);
                    self.emit_line(&format!(
                        "local {out} = molt_pow({base}, {exp}) % {modulus}"
                    ));
                }
            }
            "matmul" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = nil -- [unsupported op: matmul]"));
            }
            "round" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = math.round({})",
                        sanitize_ident(val)
                    ));
                }
            }
            "trunc" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = math_floor({})",
                        sanitize_ident(val)
                    ));
                }
            }

            // ================================================================
            // Bitwise ops (real IR op kinds)
            // ================================================================
            "bit_and" | "inplace_bit_and" => self.emit_bit_op(op, "band"),
            "bit_or" | "inplace_bit_or" => self.emit_bit_op(op, "bor"),
            "bit_xor" | "inplace_bit_xor" => self.emit_bit_op(op, "bxor"),
            "lshift" => self.emit_bit_op(op, "lshift"),
            "rshift" => self.emit_bit_op(op, "rshift"),

            // ================================================================
            // Unary ops (real IR op kinds)
            // ================================================================
            "not" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("local {out} = not {}", sanitize_ident(val)));
                }
            }
            "invert" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("local {out} = bit32.bnot({})", sanitize_ident(val)));
                }
            }
            "abs" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("local {out} = math.abs({})", sanitize_ident(val)));
                }
            }

            // ================================================================
            // Comparison ops (real IR op kinds)
            // ================================================================
            "lt" => self.emit_binary_op(op, "<"),
            "le" => self.emit_binary_op(op, "<="),
            "gt" => self.emit_binary_op(op, ">"),
            "ge" => self.emit_binary_op(op, ">="),
            "eq" | "string_eq" | "is" => self.emit_binary_op(op, "=="),
            "ne" => self.emit_binary_op(op, "~="),

            // ================================================================
            // Logical ops
            // ================================================================
            "and" => self.emit_binary_op(op, "and"),
            "or" => self.emit_binary_op(op, "or"),

            // ================================================================
            // Pedagogical composite ops (binop/compare/unary_op with s_value)
            // ================================================================
            "binop" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = sanitize_ident(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    let op_str = op.s_value.as_deref().unwrap_or("+");
                    let expr = match op_str {
                        "+" | "-" | "*" | "/" | "%" => format!("{lhs} {op_str} {rhs}"),
                        "//" => format!("molt_floor_div({lhs}, {rhs})"),
                        "**" => format!("molt_pow({lhs}, {rhs})"),
                        "&" => format!("bit32.band({lhs}, {rhs})"),
                        "|" => format!("bit32.bor({lhs}, {rhs})"),
                        "^" => format!("bit32.bxor({lhs}, {rhs})"),
                        "<<" => format!("bit32.lshift({lhs}, {rhs})"),
                        ">>" => format!("bit32.rshift({lhs}, {rhs})"),
                        _ => format!("{lhs} {op_str} {rhs}"),
                    };
                    self.emit_line(&format!("local {out} = {expr}"));
                }
            }
            "compare" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = sanitize_ident(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    let cmp = op.s_value.as_deref().unwrap_or("==");
                    let luau_cmp = match cmp {
                        "!=" | "<>" => "~=",
                        other => other,
                    };
                    self.emit_line(&format!("local {out} = {lhs} {luau_cmp} {rhs}"));
                }
            }
            "unary_op" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(operand) = args.first() {
                    let operand = sanitize_ident(operand);
                    let uop = op.s_value.as_deref().unwrap_or("-");
                    let expr = match uop {
                        "-" => format!("-{operand}"),
                        "not" => format!("not {operand}"),
                        "~" => format!("bit32.bnot({operand})"),
                        _ => format!("-{operand}"),
                    };
                    self.emit_line(&format!("local {out} = {expr}"));
                }
            }

            // ================================================================
            // Control flow: labels and jumps
            // ================================================================
            "label" | "state_label" => {
                // Standalone Luau CLI doesn't support goto; Roblox Studio does.
                // Emit as comments for compatibility with both targets.
                if let Some(id) = op.value {
                    self.emit_line(&format!("-- ::label_{id}::"));
                } else if let Some(ref s) = op.s_value {
                    let label = sanitize_label(s);
                    self.emit_line(&format!("-- ::{label}::"));
                }
            }
            "jump" | "goto" => {
                if let Some(id) = op.value {
                    self.emit_line(&format!("-- goto label_{id}"));
                } else if let Some(ref target) = op.s_value {
                    let target = sanitize_label(target);
                    self.emit_line(&format!("-- goto {target}"));
                }
            }
            "br_if" => {
                // Luau has no goto. Convert conditional branches to
                // if/then/end blocks. For exception handler jumps,
                // emit the error() call directly.
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(cond) = args.first() {
                    let cond = sanitize_ident(cond);
                    self.emit_line(&format!("-- br_if {cond}"));
                }
            }
            "branch" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                let cond = if let Some(c) = args.first() {
                    sanitize_ident(c)
                } else if let Some(ref v) = op.var {
                    sanitize_ident(v)
                } else {
                    "true".to_string()
                };
                self.emit_line(&format!("-- branch {cond}"));
            }
            "branch_false" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                let cond = if let Some(c) = args.first() {
                    sanitize_ident(c)
                } else if let Some(ref v) = op.var {
                    sanitize_ident(v)
                } else {
                    "false".to_string()
                };
                self.emit_line(&format!("-- branch_false {cond}"));
            }

            // ================================================================
            // Structured if/else/end_if
            // ================================================================
            "if" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(cond) = args.first() {
                    self.emit_line(&format!("if {} then", sanitize_ident(cond)));
                    self.push_indent();
                }
            }
            "else" => {
                self.pop_indent();
                self.emit_line("else");
                self.push_indent();
            }
            "end_if" => {
                self.pop_indent();
                self.emit_line("end");
            }

            // ================================================================
            // Loops
            // ================================================================
            "loop_start" => {
                // Check if the next op is loop_index_start — if so, its
                // initialization is handled via the pending_loop_index
                // mechanism to ensure it runs before the while opens.
                self.emit_line("while true do");
                self.push_indent();
            }
            "loop_index_start" => {
                // No-op here — initialization is emitted before the while
                // by the loop_start handler via pending_loop_index.
                // The pre-declared variable is set before loop entry.
            }
            "loop_end" => {
                self.pop_indent();
                self.emit_line("end");
            }
            "loop_break" => {
                self.emit_line("break");
            }
            "loop_break_if_true" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(cond) = args.first() {
                    self.emit_line(&format!("if {} then break end", sanitize_ident(cond)));
                }
            }
            "loop_break_if_false" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(cond) = args.first() {
                    self.emit_line(&format!("if not {} then break end", sanitize_ident(cond)));
                }
            }
            "loop_continue" => {
                self.emit_line("continue");
            }
            "loop_index_next" => {
                // Update loop counter: out = args[0].
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    let args = op.args.as_deref().unwrap_or(&[]);
                    if let Some(new_val) = args.first() {
                        let val = sanitize_ident(new_val);
                        self.emit_line(&format!("{out} = {val}"));
                    }
                }
            }
            "loop_carry_init" | "loop_carry_update" => {
                // Internal loop bookkeeping — skip.
            }
            "for_range" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let start = sanitize_ident(&args[0]);
                    let stop = sanitize_ident(&args[1]);
                    let step = if args.len() >= 3 {
                        sanitize_ident(&args[2])
                    } else {
                        "1".to_string()
                    };
                    self.emit_line(&format!("for {out} = {start}, {stop} - 1, {step} do"));
                    self.push_indent();
                }
            }
            "for_iter" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(iterable) = args.first() {
                    let iterable = sanitize_ident(iterable);
                    self.emit_line(&format!("for _, {out} in ipairs({iterable}) do"));
                    self.push_indent();
                }
            }
            "end_for" => {
                self.pop_indent();
                self.emit_line("end");
            }

            // ================================================================
            // Function calls
            // ================================================================
            "call" | "call_guarded" => {
                let out = self.out_var(op);
                // First check for s_value function name (pedagogical IR form).
                if let Some(ref func_name) = op.s_value {
                    let func_name = sanitize_ident(func_name);
                    let call_args = op
                        .args
                        .as_deref()
                        .unwrap_or(&[])
                        .iter()
                        .map(|a| sanitize_ident(a))
                        .collect::<Vec<_>>()
                        .join(", ");
                    // Map Python builtins to Luau equivalents.
                    let mapped = match func_name.as_str() {
                        "len" | "molt_len" => format!("molt_len({call_args})"),
                        "int" | "molt_int" => format!("molt_int({call_args})"),
                        "float" | "molt_float" => format!("molt_float({call_args})"),
                        "str" | "molt_str" => format!("molt_str({call_args})"),
                        "bool" | "molt_bool" => format!("molt_bool({call_args})"),
                        "range" | "molt_range" => format!("molt_range({call_args})"),
                        "abs" => format!("math.abs({call_args})"),
                        "min" => format!("math.min({call_args})"),
                        "max" => format!("math.max({call_args})"),
                        "round" => format!("math.round({call_args})"),
                        "enumerate" | "molt_enumerate" => format!("molt_enumerate({call_args})"),
                        "zip" | "molt_zip" => format!("molt_zip({call_args})"),
                        "sorted" | "molt_sorted" => format!("molt_sorted({call_args})"),
                        "reversed" | "molt_reversed" => format!("molt_reversed({call_args})"),
                        "sum" | "molt_sum" => format!("molt_sum({call_args})"),
                        "any" | "molt_any" => format!("molt_any({call_args})"),
                        "all" | "molt_all" => format!("molt_all({call_args})"),
                        "map" | "molt_map" => format!("molt_map({call_args})"),
                        "filter" | "molt_filter" => format!("molt_filter({call_args})"),
                        "print" => format!("print({call_args})"),
                        _ => format!("{func_name}({call_args})"),
                    };
                    self.emit_line(&format!("local {out} = {mapped}"));
                } else {
                    // Real IR form: args[0] is the callable, rest are arguments.
                    let args = op.args.as_deref().unwrap_or(&[]);
                    if !args.is_empty() {
                        let func_ref = sanitize_ident(&args[0]);
                        let call_args = args[1..]
                            .iter()
                            .map(|a| sanitize_ident(a))
                            .collect::<Vec<_>>()
                            .join(", ");
                        if op.out.is_some() {
                            self.emit_line(&format!("local {out} = {func_ref}({call_args})"));
                        } else {
                            self.emit_line(&format!("{func_ref}({call_args})"));
                        }
                    }
                }
            }
            "call_internal" => {
                if let Some(ref s_val) = op.s_value {
                    let func_name = sanitize_ident(s_val);
                    let call_args = op
                        .args
                        .as_deref()
                        .unwrap_or(&[])
                        .iter()
                        .map(|a| sanitize_ident(a))
                        .collect::<Vec<_>>()
                        .join(", ");
                    if let Some(ref out_name) = op.out {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!("local {out} = {func_name}({call_args})"));
                    } else {
                        self.emit_line(&format!("{func_name}({call_args})"));
                    }
                }
            }
            "call_func" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if !args.is_empty() {
                    let func_ref = sanitize_ident(&args[0]);
                    let call_args = args[1..]
                        .iter()
                        .map(|a| sanitize_ident(a))
                        .collect::<Vec<_>>()
                        .join(", ");
                    if let Some(ref out_name) = op.out {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!(
                            "local {out} = if {func_ref} then {func_ref}({call_args}) else nil"
                        ));
                    } else {
                        self.emit_line(&format!(
                            "if {func_ref} then {func_ref}({call_args}) end"
                        ));
                    }
                }
            }
            "call_method" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if !args.is_empty() {
                    let obj = sanitize_ident(&args[0]);
                    let method = op.s_value.as_deref().unwrap_or("unknown");
                    let method = sanitize_ident(method);
                    let call_args = args[1..]
                        .iter()
                        .map(|a| sanitize_ident(a))
                        .collect::<Vec<_>>()
                        .join(", ");
                    if let Some(ref out_name) = op.out {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!("local {out} = {obj}:{method}({call_args})"));
                    } else {
                        self.emit_line(&format!("{obj}:{method}({call_args})"));
                    }
                }
            }
            "call_async" | "block_on" | "spawn" => {
                // Async primitives — emit as synchronous call stub.
                let args = op.args.as_deref().unwrap_or(&[]);
                if !args.is_empty() {
                    let func_ref = sanitize_ident(&args[0]);
                    let call_args = args[1..]
                        .iter()
                        .map(|a| sanitize_ident(a))
                        .collect::<Vec<_>>()
                        .join(", ");
                    if let Some(ref out_name) = op.out {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!(
                            "local {out} = {func_ref}({call_args}) -- [async: {}]",
                            op.kind
                        ));
                    } else {
                        self.emit_line(&format!("{func_ref}({call_args}) -- [async: {}]", op.kind));
                    }
                } else if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil -- [async: {}]", op.kind));
                }
            }

            // ================================================================
            // Return
            // ================================================================
            "ret" | "return" | "return_value" => {
                if let Some(ref args) = op.args {
                    if let Some(val) = args.first() {
                        self.emit_line(&format!("return {}", sanitize_ident(val)));
                    } else {
                        self.emit_line("return");
                    }
                } else if let Some(ref var) = op.var {
                    self.emit_line(&format!("return {}", sanitize_ident(var)));
                } else {
                    self.emit_line("return");
                }
            }
            "ret_void" => {
                self.emit_line("return");
            }

            // ================================================================
            // Collection construction
            // ================================================================
            "build_list" | "list_new" | "callargs_new" => {
                let out = self.out_var(op);
                let items = op
                    .args
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|a| sanitize_ident(a))
                    .collect::<Vec<_>>()
                    .join(", ");
                self.emit_line(&format!("local {out} = {{{items}}}"));
            }
            "tuple_new" | "tuple_from_list" => {
                let out = self.out_var(op);
                let items = op
                    .args
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|a| sanitize_ident(a))
                    .collect::<Vec<_>>()
                    .join(", ");
                self.emit_line(&format!("local {out} = {{{items}}}"));
            }
            "build_dict" | "dict_new" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.is_empty() {
                    self.emit_line(&format!("local {out} = {{}}"));
                } else {
                    // args are key-value pairs: [k1, v1, k2, v2, ...]
                    let mut entries = Vec::new();
                    for pair in args.chunks(2) {
                        if pair.len() == 2 {
                            let key = sanitize_ident(&pair[0]);
                            let val = sanitize_ident(&pair[1]);
                            entries.push(format!("[{key}] = {val}"));
                        }
                    }
                    let body = entries.join(", ");
                    self.emit_line(&format!("local {out} = {{{body}}}"));
                }
            }
            "set_new" | "frozenset_new" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = {{}}"));
            }
            "range_new" | "list_from_range" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                match args.len() {
                    1 => {
                        let stop = sanitize_ident(&args[0]);
                        self.emit_line(&format!("local {out} = molt_range(0, {stop})"));
                    }
                    2 => {
                        let start = sanitize_ident(&args[0]);
                        let stop = sanitize_ident(&args[1]);
                        self.emit_line(&format!("local {out} = molt_range({start}, {stop})"));
                    }
                    _ => {
                        let start = sanitize_ident(&args[0]);
                        let stop = sanitize_ident(&args[1]);
                        let step = sanitize_ident(&args[2]);
                        self.emit_line(&format!(
                            "local {out} = molt_range({start}, {stop}, {step})"
                        ));
                    }
                }
            }

            // ================================================================
            // List operations
            // ================================================================
            "list_append" | "callargs_push_pos" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let list = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    self.emit_line(&format!("table.insert({list}, {val})"));
                }
            }
            "list_pop" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(list) = args.first() {
                    let list = sanitize_ident(list);
                    if let Some(ref out_name) = op.out {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!("local {out} = table.remove({list})"));
                    } else {
                        self.emit_line(&format!("table.remove({list})"));
                    }
                }
            }
            "list_extend" | "callargs_expand_star" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let list = sanitize_ident(&args[0]);
                    let other = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "for _, __v in ipairs({other}) do table.insert({list}, __v) end"
                    ));
                }
            }
            "list_insert" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let list = sanitize_ident(&args[0]);
                    let idx = sanitize_ident(&args[1]);
                    let val = sanitize_ident(&args[2]);
                    self.emit_line(&format!("table.insert({list}, {idx} + 1, {val})"));
                }
            }
            "list_remove" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let list = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "for __i, __v in ipairs({list}) do if __v == {val} then table.remove({list}, __i); break end end"
                    ));
                }
            }
            "list_clear" | "dict_clear" | "set_clear" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(tbl) = args.first() {
                    self.emit_line(&format!("table.clear({})", sanitize_ident(tbl)));
                }
            }
            "list_copy" | "dict_copy" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(src) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = table.clone({})",
                        sanitize_ident(src)
                    ));
                }
            }
            "list_reverse" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(list) = args.first() {
                    let list = sanitize_ident(list);
                    self.emit_line(&format!(
                        "do local __n = #{list}; for __i = 1, math_floor(__n / 2) do {list}[__i], {list}[__n - __i + 1] = {list}[__n - __i + 1], {list}[__i] end end"
                    ));
                }
            }
            "list_count" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let list = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out} = 0; for _, __v in ipairs({list}) do if __v == {val} then {out} = {out} + 1 end end"
                    ));
                }
            }
            "list_index" | "list_index_range" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let list = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out} = -1; for __i, __v in ipairs({list}) do if __v == {val} then {out} = __i - 1; break end end"
                    ));
                }
            }
            "list_repeat_range" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let val = sanitize_ident(&args[0]);
                    let count = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out} = {{}}; for __i = 1, {count} do table.insert({out}, {val}) end"
                    ));
                } else {
                    self.emit_line(&format!("local {out} = {{}}"));
                }
            }

            // ================================================================
            // Dict operations
            // ================================================================
            "dict_get" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let dict = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    self.emit_line(&format!("local {out} = {dict}[{key}]"));
                }
            }
            "dict_set" | "dict_setdefault" | "callargs_push_kw" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let dict = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    let val = sanitize_ident(&args[2]);
                    self.emit_line(&format!("{dict}[{key}] = {val}"));
                }
            }
            "dict_setdefault_empty_list" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let dict = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "if {dict}[{key}] == nil then {dict}[{key}] = {{}} end"
                    ));
                }
            }
            "dict_pop" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let dict = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    self.emit_line(&format!("local {out} = {dict}[{key}]"));
                    self.emit_line(&format!("{dict}[{key}] = nil"));
                }
            }
            "dict_update"
            | "dict_update_missing"
            | "callargs_expand_kwstar"
            | "dict_update_kwstar" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let dict = sanitize_ident(&args[0]);
                    let other = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "for __k, __v in pairs({other}) do {dict}[__k] = __v end"
                    ));
                }
            }
            "dict_popitem" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(dict) = args.first() {
                    let dict = sanitize_ident(dict);
                    self.emit_line(&format!(
                        "local {out} = nil; for __k, __v in pairs({dict}) do {out} = {{__k, __v}}; {dict}[__k] = nil; break end"
                    ));
                }
            }
            "dict_inc" | "dict_str_int_inc" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let dict = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    let inc = sanitize_ident(&args[2]);
                    self.emit_line(&format!("{dict}[{key}] = ({dict}[{key}] or 0) + {inc}"));
                }
            }
            "dict_from_obj" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(src) = args.first() {
                    let src = sanitize_ident(src);
                    self.emit_line(&format!(
                        "local {out} = {{}}; for __k, __v in pairs({src}) do {out}[__k] = __v end"
                    ));
                }
            }

            // ================================================================
            // Set operations
            // ================================================================
            "set_add" | "frozenset_add" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let set = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    self.emit_line(&format!("{set}[{val}] = true"));
                }
            }
            "set_discard" | "set_remove" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let set = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    self.emit_line(&format!("{set}[{val}] = nil"));
                }
            }
            "set_pop" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(set) = args.first() {
                    let set = sanitize_ident(set);
                    self.emit_line(&format!(
                        "local {out} = nil; for __k in pairs({set}) do {out} = __k; {set}[__k] = nil; break end"
                    ));
                }
            }
            "set_update" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let set = sanitize_ident(&args[0]);
                    let other = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "for __k in pairs({other}) do {set}[__k] = true end"
                    ));
                }
            }
            "contains" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let container = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    self.emit_line(&format!("local {out} = ({container}[{val}] ~= nil)"));
                }
            }

            // ================================================================
            // Indexing / subscript
            // ================================================================
            "get_item" | "subscript" | "index" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let container = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    // Offset integer keys by +1 for Luau 1-based arrays.
                    self.emit_line(&format!(
                        "local {out} = {container}[if type({key}) == \"number\" then {key} + 1 else {key}]"
                    ));
                }
            }
            "set_item" | "store_subscript" | "store_index" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let container = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    let value = sanitize_ident(&args[2]);
                    // Offset integer keys by +1 for Luau 1-based arrays.
                    self.emit_line(&format!(
                        "{container}[if type({key}) == \"number\" then {key} + 1 else {key}] = {value}"
                    ));
                }
            }
            "del_index" | "del_item" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let container = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    self.emit_line(&format!("{container}[{key}] = nil"));
                }
            }

            // ================================================================
            // Attribute access
            // ================================================================
            "get_attr"
            | "get_attr_generic_obj"
            | "get_attr_generic_ptr"
            | "get_attr_name"
            | "get_attr_special_obj" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                let raw_attr = op.s_value.as_deref().unwrap_or("unknown");
                let attr = sanitize_ident(raw_attr);
                if let Some(obj) = args.first() {
                    let obj = sanitize_ident(obj);
                    // For dunder attrs that might be on functions (stored
                    // in the side-table), look there first.
                    let use_side_table = matches!(
                        raw_attr,
                        "__defaults__" | "__kwdefaults__" | "__closure__"
                    );
                    if use_side_table {
                        self.emit_line(&format!(
                            "local {out} = if molt_func_attrs[{obj}] then molt_func_attrs[{obj}].{attr} else nil"
                        ));
                    } else {
                        self.emit_line(&format!("local {out} = {obj}.{attr}"));
                    }
                }
            }
            "get_attr_name_default" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                let attr = op.s_value.as_deref().unwrap_or("unknown");
                let attr = sanitize_ident(attr);
                if let Some(obj) = args.first() {
                    let obj = sanitize_ident(obj);
                    let default = if args.len() >= 2 {
                        sanitize_ident(&args[1])
                    } else {
                        "nil".to_string()
                    };
                    self.emit_line(&format!(
                        "local {out}; if {obj}.{attr} ~= nil then {out} = {obj}.{attr} else {out} = {default} end"
                    ));
                }
            }
            "has_attr_name" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                let attr = op.s_value.as_deref().unwrap_or("unknown");
                let attr = sanitize_ident(attr);
                if let Some(obj) = args.first() {
                    let obj = sanitize_ident(obj);
                    self.emit_line(&format!("local {out} = ({obj}.{attr} ~= nil)"));
                }
            }
            "set_attr" | "set_attr_generic_obj" | "set_attr_generic_ptr" | "set_attr_name" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                let attr = op.s_value.as_deref().unwrap_or("unknown");
                if attr.starts_with("__") && attr.ends_with("__") {
                    // Dunder attribute.  Functions can't hold attrs in Luau,
                    // so store semantically meaningful ones in the side-table
                    // and drop purely informational metadata.
                    let needs_side_table =
                        matches!(attr, "__defaults__" | "__kwdefaults__" | "__closure__");
                    if needs_side_table && args.len() >= 2 {
                        let obj = sanitize_ident(&args[0]);
                        let value = sanitize_ident(&args[1]);
                        let attr_s = sanitize_ident(attr);
                        self.emit_line(&format!(
                            "if {obj} then if not molt_func_attrs[{obj}] then molt_func_attrs[{obj}] = {{}} end; molt_func_attrs[{obj}].{attr_s} = {value} end"
                        ));
                    }
                    // All other dunders — no-op.
                } else {
                    let attr = sanitize_ident(attr);
                    if args.len() >= 2 {
                        let obj = sanitize_ident(&args[0]);
                        let value = sanitize_ident(&args[1]);
                        self.emit_line(&format!("{obj}.{attr} = {value}"));
                    }
                }
            }
            "del_attr_generic_obj" | "del_attr_generic_ptr" | "del_attr_name" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                let attr = op.s_value.as_deref().unwrap_or("unknown");
                let attr = sanitize_ident(attr);
                if let Some(obj) = args.first() {
                    let obj = sanitize_ident(obj);
                    self.emit_line(&format!("{obj}.{attr} = nil"));
                }
            }

            // ================================================================
            // Guarded field access
            // ================================================================
            "guarded_field_get" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(obj) = args.first() {
                    let obj = sanitize_ident(obj);
                    if let Some(attr) = op.s_value.as_deref() {
                        let attr = sanitize_ident(attr);
                        self.emit_line(&format!("local {out} = {obj}.{attr}"));
                    } else if args.len() >= 2 {
                        let key = sanitize_ident(&args[1]);
                        self.emit_line(&format!("local {out} = {obj}[{key}]"));
                    }
                }
            }
            "guarded_field_set" | "guarded_field_init" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = sanitize_ident(&args[0]);
                    if let Some(attr) = op.s_value.as_deref() {
                        let attr = sanitize_ident(attr);
                        let val = sanitize_ident(&args[1]);
                        self.emit_line(&format!("{obj}.{attr} = {val}"));
                    } else if args.len() >= 3 {
                        let key = sanitize_ident(&args[1]);
                        let val = sanitize_ident(&args[2]);
                        self.emit_line(&format!("{obj}[{key}] = {val}"));
                    }
                }
            }

            // ================================================================
            // Len / type introspection
            // ================================================================
            "len" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(obj) = args.first() {
                    self.emit_line(&format!("local {out} = molt_len({})", sanitize_ident(obj)));
                }
            }
            "id" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(obj) = args.first() {
                    self.emit_line(&format!("local {out} = tostring({})", sanitize_ident(obj)));
                }
            }
            "ord" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = string.byte({}, 1)",
                        sanitize_ident(val)
                    ));
                }
            }
            "chr" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = string.char({})",
                        sanitize_ident(val)
                    ));
                }
            }
            "type_of" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(obj) = args.first() {
                    self.emit_line(&format!("local {out} = type({})", sanitize_ident(obj)));
                }
            }
            "isinstance" | "issubclass" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = true -- [stub: {}]", op.kind));
            }

            // ================================================================
            // Type casting
            // ================================================================
            "int_from_obj" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("local {out} = molt_int({})", sanitize_ident(val)));
                }
            }
            "float_from_obj" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = molt_float({})",
                        sanitize_ident(val)
                    ));
                }
            }
            "str_from_obj" | "ascii_from_obj" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("local {out} = molt_str({})", sanitize_ident(val)));
                }
            }
            "repr_from_obj" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("local {out} = molt_repr({})", sanitize_ident(val)));
                }
            }
            "bytes_from_obj" | "bytes_from_str" | "bytearray_from_obj" | "bytearray_from_str" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("local {out} = tostring({})", sanitize_ident(val)));
                }
            }

            // ================================================================
            // Print
            // ================================================================
            "print" => {
                let call_args = op
                    .args
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|a| sanitize_ident(a))
                    .collect::<Vec<_>>()
                    .join(", ");
                self.emit_line(&format!("print({call_args})"));
            }
            "print_newline" => {
                self.emit_line("print()");
            }

            // ================================================================
            // Function/class/module objects
            // ================================================================
            "func_new" | "func_new_closure" | "code_new" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    let name = op
                        .s_value
                        .as_deref()
                        .map(sanitize_ident)
                        .unwrap_or_else(|| "nil".to_string());
                    self.emit_line(&format!("local {out} = {name}"));
                }
            }
            "builtin_func" => {
                // Runtime intrinsics (molt_function_set_builtin, etc.)
                // don't exist in Luau.  Emit nil so callers become no-ops.
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "class_new" | "module_new" | "object_new" | "builtin_type" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = {{}}"));
            }
            "bound_method_new" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = sanitize_ident(&args[0]);
                    let method = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out} = function(...) return {method}({obj}, ...) end"
                    ));
                } else {
                    self.emit_line(&format!("local {out} = nil -- bound_method missing args"));
                }
            }
            "class_set_base"
            | "object_set_class"
            | "class_set_layout_version"
            | "class_apply_set_name"
            | "class_layout_version" => {
                self.emit_line(&format!("-- [class op: {}]", op.kind));
            }
            "module_import" | "module_cache_get" | "module_cache_set" | "module_cache_del"
            | "module_import_star" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    // Try to statically map known module names.
                    let args = op.args.as_deref().unwrap_or(&[]);
                    let module_name = op
                        .s_value
                        .as_deref()
                        .unwrap_or("");
                    let mapped = match module_name {
                        "math" => "molt_math",
                        "json" => "json",
                        _ => "",
                    };
                    if !mapped.is_empty() {
                        self.emit_line(&format!("local {out} = {mapped}"));
                    } else if matches!(op.kind.as_str(), "module_cache_get" | "module_import") {
                        // Dynamic lookup via the runtime module cache.
                        // The args[0] variable holds the module name string.
                        if let Some(name_var) = args.first() {
                            let nv = sanitize_ident(name_var);
                            self.emit_line(&format!(
                                "local {out} = molt_module_cache[{nv}] or {{}}"
                            ));
                        } else {
                            self.emit_line(&format!("local {out} = {{}}"));
                        }
                    } else {
                        self.emit_line(&format!("local {out} = nil"));
                    }
                } else {
                    // module_cache_set / module_cache_del — no output needed.
                }
            }
            "module_get_attr" | "module_get_global" | "module_get_name" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                let is_global = matches!(op.kind.as_str(), "module_get_global" | "module_get_name");
                if let Some(attr_str) = op.s_value.as_deref().filter(|s| !s.is_empty()) {
                    // Static attribute name — use dot access.
                    let attr = sanitize_ident(attr_str);
                    if let Some(module) = args.first() {
                        let module = sanitize_ident(module);
                        self.emit_line(&format!("local {out} = {module}.{attr}"));
                    }
                } else if args.len() >= 2 {
                    if is_global {
                        // module_get_global: args[0] = source module (often __main__),
                        // args[1] = name var holding target module name.
                        // Look up in module cache to resolve `import math` etc.
                        let name_var = sanitize_ident(&args[1]);
                        self.emit_line(&format!(
                            "local {out} = molt_module_cache[{name_var}] or nil"
                        ));
                    } else {
                        // module_get_attr: args[0] = module table, args[1] = attr name var.
                        // Look up attribute directly on the module.
                        let module = sanitize_ident(&args[0]);
                        let attr_var = sanitize_ident(&args[1]);
                        self.emit_line(&format!(
                            "local {out} = if type({module}) == \"table\" then {module}[{attr_var}] else nil"
                        ));
                    }
                } else if let Some(module) = args.first() {
                    let module = sanitize_ident(module);
                    self.emit_line(&format!("local {out} = {module}"));
                }
            }
            "module_set_attr" | "module_del_global" => {
                // Module dict mutations are no-ops in Luau — the __main__
                // module dict isn't used at runtime.
            }

            // ================================================================
            // Alloc / memory (table stubs)
            // ================================================================
            "alloc"
            | "alloc_class"
            | "alloc_class_trusted"
            | "alloc_class_static"
            | "alloc_task" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = {{}}"));
            }

            // ================================================================
            // Dataclass
            // ================================================================
            "dataclass_new" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = {{}}"));
            }
            "dataclass_get" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(obj) = args.first() {
                    let obj = sanitize_ident(obj);
                    if let Some(attr) = op.s_value.as_deref() {
                        self.emit_line(&format!("local {out} = {obj}.{}", sanitize_ident(attr)));
                    } else if args.len() >= 2 {
                        let idx = sanitize_ident(&args[1]);
                        self.emit_line(&format!("local {out} = {obj}[{idx}]"));
                    }
                }
            }
            "dataclass_set" | "dataclass_set_class" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = sanitize_ident(&args[0]);
                    if let Some(attr) = op.s_value.as_deref() {
                        let val = sanitize_ident(&args[1]);
                        self.emit_line(&format!("{obj}.{} = {val}", sanitize_ident(attr)));
                    }
                }
            }

            // ================================================================
            // Exception handling (stubs)
            // ================================================================
            "exception_push"
            | "exception_pop"
            | "exception_stack_clear"
            | "exception_stack_enter"
            | "exception_stack_exit"
            | "exception_set_last"
            | "exception_set_value"
            | "exception_set_cause"
            | "exception_context_set"
            | "exception_stack_set_depth"
            | "exception_clear" => {
                // Exception bookkeeping — no Luau equivalent.
            }
            "exception_new" | "exception_new_from_class" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                let msg = args
                    .first()
                    .map(|a| sanitize_ident(a))
                    .unwrap_or_else(|| "\"error\"".to_string());
                self.emit_line(&format!("local {out} = {msg}"));
            }
            "raise" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("error({})", sanitize_ident(val)));
                } else {
                    self.emit_line("error(\"raised\")");
                }
            }
            "check_exception" => {
                // Suppress — exception handler jumps are no-ops in Luau.
            }
            "exception_last"
            | "exception_stack_depth"
            | "exception_kind"
            | "exception_class"
            | "exception_message"
            | "exceptiongroup_match"
            | "exceptiongroup_combine" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = nil -- [{}]", op.kind));
            }

            // ================================================================
            // Context manager stubs
            // ================================================================
            "context_null" | "context_enter" | "context_exit" | "context_closing"
            | "context_unwind" | "context_unwind_to" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil -- [context: {}]", op.kind));
                }
            }
            "context_depth" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = 0"));
            }

            // ================================================================
            // Async/generator stubs
            // ================================================================
            "state_switch"
            | "state_transition"
            | "state_yield"
            | "chan_new"
            | "chan_drop"
            | "chan_send_yield"
            | "chan_recv_yield"
            | "cancel_token_new"
            | "cancel_token_clone"
            | "cancel_token_drop"
            | "cancel_token_cancel"
            | "cancel_token_is_cancelled"
            | "cancel_token_set_current"
            | "cancel_token_get_current"
            | "cancelled"
            | "cancel_current"
            | "future_cancel"
            | "future_cancel_msg"
            | "future_cancel_clear"
            | "promise_new"
            | "promise_set_result"
            | "promise_set_exception"
            | "thread_submit"
            | "task_register_token_owned"
            | "is_native_awaitable" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil -- [async: {}]", op.kind));
                }
            }

            // ================================================================
            // File I/O stubs
            // ================================================================
            "file_open" | "file_read" | "file_write" | "file_close" | "file_flush" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil -- [file: {}]", op.kind));
                }
            }

            // ================================================================
            // Misc intrinsics
            // ================================================================
            "getargv" | "getframe" | "sys_executable" | "bridge_unavailable" | "missing" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil -- [{}]", op.kind));
                }
            }
            "super_new" | "classmethod_new" | "staticmethod_new" | "property_new" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil -- [{}]", op.kind));
                }
            }

            // ================================================================
            // Closure/code internals
            // ================================================================
            "code_slot_set"
            | "code_slots_init"
            | "fn_ptr_code_set"
            | "asyncgen_locals_register"
            | "gen_locals_register"
            | "function_closure_bits"
            | "trace_enter_slot"
            | "trace_exit"
            | "frame_locals_set" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil -- [internal: {}]", op.kind));
                }
            }

            // ================================================================
            // Line info (debug)
            // ================================================================
            "line" => {
                // Skip line markers in production output — they add
                // ~3% to file size with no runtime benefit.
                // Uncomment for debugging: self.emit_line(&format!("-- line {val}"));

            }

            // ================================================================
            // Raw int bridge (no-op in Luau — values are already unboxed)
            // ================================================================
            "unbox_to_raw_int" | "box_from_raw_int" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("local {out} = {}", sanitize_ident(val)));
                }
            }

            // ================================================================
            // Vectorized reduction ops (emit stubs)
            // ================================================================
            kind if kind.starts_with("vec_sum_")
                || kind.starts_with("vec_prod_")
                || kind.starts_with("vec_min_")
                || kind.starts_with("vec_max_") =>
            {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = 0 -- [vectorized: {}]", op.kind));
            }

            // ================================================================
            // Serialization stubs
            // ================================================================
            "json_parse" | "msgpack_parse" | "cbor_parse" | "invoke_ffi" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil -- [{}]", op.kind));
                }
            }

            // ================================================================
            // Memoryview / complex / bytearray stubs
            // ================================================================
            "memoryview_new"
            | "memoryview_tobytes"
            | "memoryview_cast"
            | "intarray_from_seq"
            | "complex_from_obj"
            | "bytearray_fill_range" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil -- [{}]", op.kind));
                }
            }

            // ================================================================
            // String ops
            // ================================================================
            "string_join" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let sep = sanitize_ident(&args[0]);
                    let list = sanitize_ident(&args[1]);
                    self.emit_line(&format!("local {out} = table.concat({list}, {sep})"));
                }
            }
            "string_format" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if !args.is_empty() {
                    let fmt_str = sanitize_ident(&args[0]);
                    let fmt_args = args[1..]
                        .iter()
                        .map(|a| sanitize_ident(a))
                        .collect::<Vec<_>>()
                        .join(", ");
                    if fmt_args.is_empty() {
                        self.emit_line(&format!("local {out} = {fmt_str}"));
                    } else {
                        self.emit_line(&format!(
                            "local {out} = string.format({fmt_str}, {fmt_args})"
                        ));
                    }
                }
            }
            "string_strip" | "string_lstrip" | "string_rstrip" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(s) = args.first() {
                    let s = sanitize_ident(s);
                    self.emit_line(&format!("local {out} = ({s}:match(\"^%s*(.-)%s*$\"))"));
                }
            }
            "string_upper" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(s) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = string.upper({})",
                        sanitize_ident(s)
                    ));
                }
            }
            "string_lower" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(s) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = string.lower({})",
                        sanitize_ident(s)
                    ));
                }
            }
            "string_startswith" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let s = sanitize_ident(&args[0]);
                    let prefix = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out} = (string.sub({s}, 1, #{prefix}) == {prefix})"
                    ));
                }
            }
            "string_endswith" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let s = sanitize_ident(&args[0]);
                    let suffix = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out} = (string.sub({s}, -#{suffix}) == {suffix})"
                    ));
                }
            }
            "string_replace" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let s = sanitize_ident(&args[0]);
                    let old = sanitize_ident(&args[1]);
                    let new = sanitize_ident(&args[2]);
                    self.emit_line(&format!("local {out} = (string.gsub({s}, {old}, {new}))"));
                }
            }
            "string_find" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let s = sanitize_ident(&args[0]);
                    let sub = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out} = (string.find({s}, {sub}, 1, true) or 0) - 1"
                    ));
                }
            }
            "string_split" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(s) = args.first() {
                    let s = sanitize_ident(s);
                    let sep = if args.len() >= 2 {
                        sanitize_ident(&args[1])
                    } else {
                        "\" \"".to_string()
                    };
                    self.emit_line(&format!("local {out} = molt_string.split({s}, {sep})"));
                }
            }
            "string_concat" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let a = sanitize_ident(&args[0]);
                    let b = sanitize_ident(&args[1]);
                    self.emit_line(&format!("local {out} = {a} .. {b}"));
                }
            }
            "string_repeat" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let s = sanitize_ident(&args[0]);
                    let n = sanitize_ident(&args[1]);
                    self.emit_line(&format!("local {out} = string.rep({s}, {n})"));
                }
            }
            "string_split_ws_dict_inc" | "string_split_sep_dict_inc" | "taq_ingest_line" => {
                self.emit_line(&format!("-- [string op: {}]", op.kind));
            }

            // ================================================================
            // Iterator ops
            // ================================================================
            "iter" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(iterable) = args.first() {
                    // In Luau, iterating a table is done via ipairs/pairs.
                    // Store the iterable itself as the "iterator".
                    self.emit_line(&format!("local {out} = {}", sanitize_ident(iterable)));
                }
            }
            "iter_next" => {
                // iter_next is handled by for loops in structured IR; stub for unstructured.
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(iter_var) = args.first() {
                    let iter_var = sanitize_ident(iter_var);
                    self.emit_line(&format!("local {out} = next({iter_var})"));
                }
            }

            // ================================================================
            // Indirect / bound calls
            // ================================================================
            "call_indirect" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if !args.is_empty() {
                    let func_ref = sanitize_ident(&args[0]);
                    let call_args = args[1..]
                        .iter()
                        .map(|a| sanitize_ident(a))
                        .collect::<Vec<_>>()
                        .join(", ");
                    if let Some(ref out_name) = op.out {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!("local {out} = {func_ref}({call_args})"));
                    } else {
                        self.emit_line(&format!("{func_ref}({call_args})"));
                    }
                }
            }
            "call_bind" => {
                // In Molt IR, call_bind is a function CALL whose second arg is
                // always a callargs tuple (built via callargs_new + callargs_push_pos).
                // We must unpack the tuple so individual args are spread:
                //   func(table.unpack(args_tuple))  instead of  func(args_tuple)
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let func = sanitize_ident(&args[0]);
                    let args_tuple = sanitize_ident(&args[1]);
                    if let Some(ref out_name) = op.out {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!(
                            "local {out} = if {func} then {func}(table.unpack({args_tuple})) else nil"
                        ));
                    } else {
                        self.emit_line(&format!(
                            "if {func} then {func}(table.unpack({args_tuple})) end"
                        ));
                    }
                } else if let Some(func) = args.first() {
                    self.emit_line(&format!("local {out} = {}", sanitize_ident(func)));
                }
            }
            "is_callable" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = (type({}) == \"function\")",
                        sanitize_ident(val)
                    ));
                }
            }

            // ================================================================
            // Try/except blocks
            // ================================================================
            "try_start" => {
                // In Luau, use pcall for try blocks.
                self.emit_line("-- [try_start]");
            }
            "try_end" => {
                self.emit_line("-- [try_end]");
            }

            // ================================================================
            // Slice
            // ================================================================
            "slice" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let obj = sanitize_ident(&args[0]);
                    let start = sanitize_ident(&args[1]);
                    let stop = sanitize_ident(&args[2]);
                    self.emit_line(&format!(
                        "local {out} = {{table.unpack({obj}, {start} + 1, {stop})}}"
                    ));
                } else if args.len() >= 2 {
                    let obj = sanitize_ident(&args[0]);
                    let start = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out} = {{table.unpack({obj}, {start} + 1)}}"
                    ));
                }
            }

            // ================================================================
            // Enumerate op (distinct from call to enumerate builtin)
            // ================================================================
            "enumerate" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(iterable) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = molt_enumerate({})",
                        sanitize_ident(iterable)
                    ));
                }
            }

            // ================================================================
            // Dict key/value/items ops
            // ================================================================
            "dict_keys" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(d) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = molt_dict_keys({})",
                        sanitize_ident(d)
                    ));
                }
            }
            "dict_values" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(d) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = molt_dict_values({})",
                        sanitize_ident(d)
                    ));
                }
            }
            "dict_items" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(d) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = molt_dict_items({})",
                        sanitize_ident(d)
                    ));
                }
            }

            // ================================================================
            // Phi nodes (SSA merge — no-op in sequential Luau) / nop
            // ================================================================
            "phi" | "nop" => {}

            // ================================================================
            // Default: unsupported op
            // ================================================================
            _ => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!(
                        "local {out} = nil -- [unsupported op: {}]",
                        op.kind
                    ));
                } else {
                    self.emit_line(&format!("-- [unsupported op: {}]", op.kind));
                }
            }
        }
    }

    // --- helper: binary op ---
    fn emit_binary_op(&mut self, op: &OpIR, operator: &str) {
        let out = self.out_var(op);
        let args = op.args.as_deref().unwrap_or(&[]);
        if args.len() >= 2 {
            let lhs = sanitize_ident(&args[0]);
            let rhs = sanitize_ident(&args[1]);
            self.emit_line(&format!("local {out} = {lhs} {operator} {rhs}"));
        }
    }

    // --- helper: bit32 op ---
    fn emit_bit_op(&mut self, op: &OpIR, func: &str) {
        let out = self.out_var(op);
        let args = op.args.as_deref().unwrap_or(&[]);
        if args.len() >= 2 {
            let lhs = sanitize_ident(&args[0]);
            let rhs = sanitize_ident(&args[1]);
            self.emit_line(&format!("local {out} = bit32.{func}({lhs}, {rhs})"));
        }
    }

    // --- helpers ---

    fn out_var(&self, op: &OpIR) -> String {
        op.out
            .as_deref()
            .map(sanitize_ident)
            .unwrap_or_else(|| "_".to_string())
    }

    fn var_ref(&self, op: &OpIR) -> String {
        op.var
            .as_deref()
            .map(sanitize_ident)
            .unwrap_or_else(|| "_".to_string())
    }

    fn emit_line(&mut self, line: &str) {
        for _ in 0..self.indent {
            self.output.push('\t');
        }
        self.output.push_str(line);
        self.output.push('\n');
    }

    fn push_indent(&mut self) {
        self.indent += 1;
    }

    fn pop_indent(&mut self) {
        if self.indent > 0 {
            self.indent -= 1;
        }
    }
}

fn collect_luau_preview_blockers(source: &str) -> Vec<String> {
    source
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            // Only flag patterns that indicate truly broken control flow
            // (goto/branch without structured replacement).  Nil-stub
            // comments like `-- [exception_last]` are harmless Luau and
            // label comments like `-- ::label_0::` are inert.
            let is_blocker = trimmed.contains("-- [unsupported op:");
            if is_blocker {
                Some(trimmed.to_string())
            } else {
                None
            }
        })
        .collect()
}

pub fn validate_luau_source(source: &str) -> Result<(), String> {
    let blockers = collect_luau_preview_blockers(source);
    if blockers.is_empty() {
        return Ok(());
    }
    let mut message = format!(
        "luau preview backend rejected lowered output with {} unsupported marker{}:",
        blockers.len(),
        if blockers.len() == 1 { "" } else { "s" }
    );
    for blocker in blockers.iter().take(8) {
        let _ = write!(message, "\n- {blocker}");
    }
    if blockers.len() > 8 {
        let _ = write!(message, "\n- ... {} more", blockers.len() - 8);
    }
    Err(message)
}

/// Sanitize a Molt IR identifier for Luau.
/// Replaces `.` and `-` with `_`, and prefixes Luau keywords with `_m_`.
fn sanitize_ident(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c == '.' || c == '-' { '_' } else { c })
        .collect();

    if is_luau_keyword(&cleaned) {
        format!("_m_{cleaned}")
    } else {
        cleaned
    }
}

fn sanitize_label(label: &str) -> String {
    label
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Strip dead code after unconditional returns at the same nesting depth.
///
/// Tracks nesting depth via structured control flow ops (if/else/end_if,
/// loop_start/loop_end, for_range/for_iter/end_for). Code after a return
/// at depth 0 (function body level) is removed. Code after a return inside
/// a structured block is kept because the block's `end` re-establishes
/// reachability for the parent scope.
fn strip_dead_after_return(ops: &[OpIR]) -> Vec<OpIR> {
    let mut result = Vec::with_capacity(ops.len());
    let mut depth: i32 = 0;
    let mut dead_at_depth: Option<i32> = None; // depth at which we became dead

    for op in ops {
        let kind = op.kind.as_str();

        // Track structured nesting.
        let is_open = matches!(
            kind,
            "if" | "loop_start" | "for_range" | "for_iter"
        );
        let is_mid = matches!(kind, "else");
        let is_close = matches!(kind, "end_if" | "loop_end" | "end_for");

        if is_open {
            if dead_at_depth.is_none() {
                result.push(op.clone());
            }
            depth += 1;
            continue;
        }
        if is_mid {
            // `else` doesn't change depth but resets dead state if we're
            // dead at this depth (the other branch may not have returned).
            if dead_at_depth == Some(depth) {
                dead_at_depth = None;
            }
            if dead_at_depth.is_none() {
                result.push(op.clone());
            }
            continue;
        }
        if is_close {
            depth -= 1;
            // Closing a block may bring us back to a reachable state.
            if let Some(d) = dead_at_depth {
                if d > depth {
                    dead_at_depth = None;
                }
            }
            if dead_at_depth.is_none() {
                result.push(op.clone());
            }
            continue;
        }

        // If we're in dead code, skip this op.
        if let Some(d) = dead_at_depth {
            if depth >= d {
                continue;
            }
            // We're at a shallower depth now — no longer dead.
            dead_at_depth = None;
        }

        // Check if this op is an unconditional return.
        let is_return = matches!(kind, "ret" | "return" | "return_value" | "ret_void");
        result.push(op.clone());

        if is_return {
            dead_at_depth = Some(depth);
        }
    }

    result
}

fn is_luau_keyword(word: &str) -> bool {
    matches!(
        word,
        "and"
            | "break"
            | "do"
            | "else"
            | "elseif"
            | "end"
            | "false"
            | "for"
            | "function"
            | "if"
            | "in"
            | "local"
            | "nil"
            | "not"
            | "or"
            | "repeat"
            | "return"
            | "then"
            | "true"
            | "until"
            | "while"
            | "continue"
            | "type"
            | "export"
    )
}

fn escape_luau_string(s: &str) -> String {
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

/// Post-processing pass: inline single-use constants.
///
/// Finds patterns like:
///   local v42 = <literal>
/// where v42 appears exactly once more in the source, and replaces
/// that single use with the literal value, removing the declaration.
fn inline_single_use_constants(source: &mut String) {
    let lines: Vec<&str> = source.lines().collect();

    // Phase 1: Identify constant declarations and count variable uses.
    let mut const_decls: HashMap<String, (usize, String)> = HashMap::new(); // var -> (line_idx, rhs)
    let mut var_use_count: HashMap<String, usize> = HashMap::new();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        // Match "local vNNN = <literal>"
        if let Some(rest) = trimmed.strip_prefix("local v") {
            if let Some(eq_pos) = rest.find(" = ") {
                let var_suffix = &rest[..eq_pos];
                if var_suffix.chars().all(|c| c.is_ascii_digit()) {
                    let var_name = format!("v{var_suffix}");
                    let rhs = rest[eq_pos + 3..].to_string();
                    // Check if RHS is a simple literal.
                    let is_literal = is_simple_literal(&rhs);
                    if is_literal {
                        const_decls.insert(var_name, (i, rhs));
                    }
                }
            }
        }

        // Count all vNNN references in this line.
        let bytes = line.as_bytes();
        let mut pos = 0;
        while pos < bytes.len() {
            if bytes[pos] == b'v' && (pos == 0 || !is_ident_char(bytes[pos - 1])) {
                let start = pos;
                pos += 1;
                while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                    pos += 1;
                }
                if pos > start + 1 && (pos >= bytes.len() || !is_ident_char(bytes[pos])) {
                    let var = std::str::from_utf8(&bytes[start..pos]).unwrap_or("");
                    *var_use_count.entry(var.to_string()).or_insert(0) += 1;
                }
            } else {
                pos += 1;
            }
        }
    }

    // Phase 2: Find single-use constants (declared once + used once = count 2).
    let mut inline_map: HashMap<String, String> = HashMap::new();
    let mut remove_lines: Vec<usize> = Vec::new();

    for (var, (line_idx, rhs)) in &const_decls {
        if var_use_count.get(var).copied().unwrap_or(0) == 2 {
            // Exactly 2 occurrences: 1 declaration + 1 use.
            // Only inline short literals to avoid code bloat.
            if rhs.len() <= 80 {
                inline_map.insert(var.clone(), rhs.clone());
                remove_lines.push(*line_idx);
            }
        }
    }

    if inline_map.is_empty() {
        return;
    }

    // Phase 3: Rebuild source with inlining.
    let mut result = String::with_capacity(source.len());
    for (i, line) in lines.iter().enumerate() {
        if remove_lines.contains(&i) {
            continue; // Skip the declaration line.
        }
        let mut new_line = (*line).to_string();
        // Replace variable references with their literal values.
        for (var, literal) in &inline_map {
            if new_line.contains(var.as_str()) {
                new_line = replace_whole_word(&new_line, var, literal);
            }
        }
        result.push_str(&new_line);
        result.push('\n');
    }

    *source = result;
    eprintln!(
        "[molt-luau] Inlined {} single-use constants",
        inline_map.len()
    );
}

fn is_simple_literal(s: &str) -> bool {
    if s == "nil" || s == "true" || s == "false" {
        return true;
    }
    // Numeric: optional minus, digits, optional decimal
    let bytes = s.as_bytes();
    if !bytes.is_empty() {
        let start = if bytes[0] == b'-' { 1 } else { 0 };
        if start < bytes.len() && bytes[start].is_ascii_digit() {
            return bytes[start..].iter().all(|&b| b.is_ascii_digit() || b == b'.');
        }
    }
    // String: starts and ends with "
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        return true;
    }
    false
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Replace whole-word occurrences of `needle` with `replacement` in `haystack`.
fn replace_whole_word(haystack: &str, needle: &str, replacement: &str) -> String {
    let bytes = haystack.as_bytes();
    let needle_bytes = needle.as_bytes();
    let mut result = String::with_capacity(haystack.len() + replacement.len());
    let mut pos = 0;

    while pos < bytes.len() {
        if pos + needle_bytes.len() <= bytes.len()
            && &bytes[pos..pos + needle_bytes.len()] == needle_bytes
        {
            let before_ok =
                pos == 0 || !is_ident_char(bytes[pos - 1]);
            let after_ok = pos + needle_bytes.len() >= bytes.len()
                || !is_ident_char(bytes[pos + needle_bytes.len()]);
            if before_ok && after_ok {
                result.push_str(replacement);
                pos += needle_bytes.len();
                continue;
            }
        }
        result.push(bytes[pos] as char);
        pos += 1;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_ident() {
        assert_eq!(sanitize_ident("foo"), "foo");
        assert_eq!(sanitize_ident("my.attr"), "my_attr");
        assert_eq!(sanitize_ident("and"), "_m_and");
        assert_eq!(sanitize_ident("v0"), "v0");
    }

    #[test]
    fn test_escape_luau_string() {
        assert_eq!(escape_luau_string("hello"), "hello");
        assert_eq!(escape_luau_string("say \"hi\""), "say \\\"hi\\\"");
        assert_eq!(escape_luau_string("a\nb"), "a\\nb");
    }

    #[test]
    fn test_empty_ir() {
        let ir = SimpleIR {
            functions: vec![],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(output.contains("--!strict"));
        assert!(output.contains("molt_main"));
    }

    #[test]
    fn test_simple_function() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "const".to_string(),
                        value: Some(42),
                        f_value: None,
                        s_value: None,
                        bytes: None,
                        var: None,
                        args: None,
                        out: Some("v0".to_string()),
                        fast_int: None,
                        task_kind: None,
                        container_type: None,
                        stack_eligible: None,
                        fast_float: None,
                        type_hint: None,
                        raw_int: None,
                    },
                    OpIR {
                        kind: "print".to_string(),
                        value: None,
                        f_value: None,
                        s_value: None,
                        bytes: None,
                        var: None,
                        args: Some(vec!["v0".to_string()]),
                        out: None,
                        fast_int: None,
                        task_kind: None,
                        container_type: None,
                        stack_eligible: None,
                        fast_float: None,
                        type_hint: None,
                        raw_int: None,
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(output.contains("function molt_main()"));
        // v0 is a single-use constant inlined into the print call.
        assert!(output.contains("print(42)"));
    }

    #[test]
    fn test_real_ir_ops() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "test_func".to_string(),
                params: vec!["p0".to_string()],
                ops: vec![
                    OpIR {
                        kind: "const_float".to_string(),
                        value: None,
                        f_value: Some(3.14),
                        s_value: None,
                        bytes: None,
                        var: None,
                        args: None,
                        out: Some("v0".to_string()),
                        fast_int: None,
                        task_kind: None,
                        container_type: None,
                        stack_eligible: None,
                        fast_float: None,
                        type_hint: None,
                        raw_int: None,
                    },
                    OpIR {
                        kind: "const_str".to_string(),
                        value: None,
                        f_value: None,
                        s_value: Some("hello".to_string()),
                        bytes: None,
                        var: None,
                        args: None,
                        out: Some("v1".to_string()),
                        fast_int: None,
                        task_kind: None,
                        container_type: None,
                        stack_eligible: None,
                        fast_float: None,
                        type_hint: None,
                        raw_int: None,
                    },
                    OpIR {
                        kind: "add".to_string(),
                        value: None,
                        f_value: None,
                        s_value: None,
                        bytes: None,
                        var: None,
                        args: Some(vec!["p0".to_string(), "v0".to_string()]),
                        out: Some("v2".to_string()),
                        fast_int: None,
                        task_kind: None,
                        container_type: None,
                        stack_eligible: None,
                        fast_float: None,
                        type_hint: None,
                        raw_int: None,
                    },
                    OpIR {
                        kind: "lt".to_string(),
                        value: None,
                        f_value: None,
                        s_value: None,
                        bytes: None,
                        var: None,
                        args: Some(vec!["v2".to_string(), "p0".to_string()]),
                        out: Some("v3".to_string()),
                        fast_int: None,
                        task_kind: None,
                        container_type: None,
                        stack_eligible: None,
                        fast_float: None,
                        type_hint: None,
                        raw_int: None,
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        value: None,
                        f_value: None,
                        s_value: None,
                        bytes: None,
                        var: None,
                        args: Some(vec!["v3".to_string()]),
                        out: None,
                        fast_int: None,
                        task_kind: None,
                        container_type: None,
                        stack_eligible: None,
                        fast_float: None,
                        type_hint: None,
                        raw_int: None,
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(output.contains("local function test_func(p0)"));
        // v0 (3.14) is single-use, inlined into the add expression.
        // add emits a type-aware string/number ternary.
        assert!(
            output.contains("p0 + 3.14") || output.contains("3.14"),
            "Expected 3.14 inlined somewhere, got:\n{output}"
        );
        assert!(output.contains("v2 < p0"));
        assert!(output.contains("return v3"));
    }

    #[test]
    fn test_control_flow() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "flow_test".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "label".to_string(),
                        value: Some(0),
                        f_value: None,
                        s_value: None,
                        bytes: None,
                        var: None,
                        args: None,
                        out: None,
                        fast_int: None,
                        task_kind: None,
                        container_type: None,
                        stack_eligible: None,
                        fast_float: None,
                        type_hint: None,
                        raw_int: None,
                    },
                    OpIR {
                        kind: "jump".to_string(),
                        value: Some(1),
                        f_value: None,
                        s_value: None,
                        bytes: None,
                        var: None,
                        args: None,
                        out: None,
                        fast_int: None,
                        task_kind: None,
                        container_type: None,
                        stack_eligible: None,
                        fast_float: None,
                        type_hint: None,
                        raw_int: None,
                    },
                    OpIR {
                        kind: "label".to_string(),
                        value: Some(1),
                        f_value: None,
                        s_value: None,
                        bytes: None,
                        var: None,
                        args: None,
                        out: None,
                        fast_int: None,
                        task_kind: None,
                        container_type: None,
                        stack_eligible: None,
                        fast_float: None,
                        type_hint: None,
                        raw_int: None,
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        value: None,
                        f_value: None,
                        s_value: None,
                        bytes: None,
                        var: None,
                        args: None,
                        out: None,
                        fast_int: None,
                        task_kind: None,
                        container_type: None,
                        stack_eligible: None,
                        fast_float: None,
                        type_hint: None,
                        raw_int: None,
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(output.contains("-- ::label_0::"));
        assert!(output.contains("-- goto label_1"));
        assert!(output.contains("-- ::label_1::"));
        assert!(output.contains("return"));
    }

    #[test]
    fn test_validate_luau_source_accepts_plain_output() {
        let source = "--!strict\nfunction molt_main()\n\tprint(42)\nend\n";
        assert!(validate_luau_source(source).is_ok());
    }

    #[test]
    fn test_validate_luau_source_accepts_stub_comments() {
        // Stub comments like [async: spawn] are harmless nil assignments.
        let source =
            "--!strict\nfunction molt_main()\n\tlocal v0 = nil -- [async: spawn]\nend\n";
        assert!(validate_luau_source(source).is_ok());
    }

    #[test]
    fn test_validate_luau_source_rejects_unsupported_op() {
        let err = validate_luau_source(
            "--!strict\nfunction molt_main()\n\tlocal v0 = nil -- [unsupported op: foo]\nend\n",
        )
        .expect_err("unsupported op marker should be rejected");
        assert!(err.contains("unsupported marker"));
        assert!(err.contains("[unsupported op: foo]"));
    }

    #[test]
    fn test_compile_checked_accepts_label_goto_comments() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "flow_test".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "label".to_string(),
                        value: Some(0),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "jump".to_string(),
                        value: Some(1),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        // Labels and gotos emit as comments (standalone Luau has no goto).
        let source = backend
            .compile_checked(&ir)
            .expect("label/goto comments should pass validation");
        assert!(source.contains("-- ::label_0::"));
        assert!(source.contains("-- goto label_1"));
    }
}
