use super::*;
use molt_tir::tir::simple_def_use::{SimpleIrResultField, visit_simple_ir_results};

impl LuauBackend {
    pub(super) fn emit_scalar_kernel_op(&mut self, op: &OpIR) -> bool {
        match op.kind.as_str() {
            // A fused-loop reduction may stand in for items of its loop only
            // when the result is exactly the loop's: exact ints and in-order
            // IEEE float operations. Luau numbers cannot hold Python ints
            // exactly, so the op consumes nothing, `(None, None, 0, False)`,
            // and the ordinary loop lowered after it runs every item.
            "vec_sum" | "vec_prod" | "vec_min" | "vec_max" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = {{nil, nil, 0, false}}"));
            }
            "checked_add" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = sanitize_ident(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    let mut sum_out = None;
                    let mut flag_out = None;
                    visit_simple_ir_results(op, |result| match result.field {
                        SimpleIrResultField::Var => sum_out = result.name,
                        SimpleIrResultField::Out => flag_out = result.name,
                        SimpleIrResultField::Arg(_) => {}
                    });
                    let sum_out = sum_out.map(sanitize_ident);
                    let flag_out = flag_out.map(sanitize_ident);
                    match (sum_out, flag_out) {
                        (Some(sum), Some(flag)) => {
                            self.emit_line(&format!(
                                "local {sum}: number, {flag}: boolean = molt_checked_i64_add({lhs}, {rhs})"
                            ));
                        }
                        (Some(sum), None) => {
                            self.emit_line(&format!(
                                "local {sum}: number = molt_checked_i64_add({lhs}, {rhs})"
                            ));
                        }
                        (None, Some(flag)) => {
                            self.emit_line(&format!(
                                "local _, {flag}: boolean = molt_checked_i64_add({lhs}, {rhs})"
                            ));
                        }
                        (None, None) => {}
                    }
                }
            }
            "checked_mul" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = sanitize_ident(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    let mut product_out = None;
                    let mut flag_out = None;
                    visit_simple_ir_results(op, |result| match result.field {
                        SimpleIrResultField::Var => product_out = result.name,
                        SimpleIrResultField::Out => flag_out = result.name,
                        SimpleIrResultField::Arg(_) => {}
                    });
                    let product_out = product_out.map(sanitize_ident);
                    let flag_out = flag_out.map(sanitize_ident);
                    match (product_out, flag_out) {
                        (Some(product), Some(flag)) => {
                            self.emit_line(&format!(
                                "local {product}: number, {flag}: boolean = molt_checked_i64_mul({lhs}, {rhs})"
                            ));
                        }
                        (Some(product), None) => {
                            self.emit_line(&format!(
                                "local {product}: number = molt_checked_i64_mul({lhs}, {rhs})"
                            ));
                        }
                        (None, Some(flag)) => {
                            self.emit_line(&format!(
                                "local _, {flag}: boolean = molt_checked_i64_mul({lhs}, {rhs})"
                            ));
                        }
                        (None, None) => {}
                    }
                }
            }
            _ => return false,
        }
        true
    }
}
