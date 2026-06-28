#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LirRuntimeCall {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    IsTruthy,
    IncRefObj,
    DecRefObj,
    IntFromI64,
}

impl LirRuntimeCall {
    #[cfg(test)]
    pub(crate) const ALL: &'static [Self] = &[
        Self::Add,
        Self::Sub,
        Self::Mul,
        Self::Div,
        Self::FloorDiv,
        Self::Mod,
        Self::Eq,
        Self::Ne,
        Self::Lt,
        Self::Le,
        Self::Gt,
        Self::Ge,
        Self::IsTruthy,
        Self::IncRefObj,
        Self::DecRefObj,
        Self::IntFromI64,
    ];

    pub(crate) const fn import_name(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Sub => "sub",
            Self::Mul => "mul",
            Self::Div => "div",
            Self::FloorDiv => "floordiv",
            Self::Mod => "mod",
            Self::Eq => "eq",
            Self::Ne => "ne",
            Self::Lt => "lt",
            Self::Le => "le",
            Self::Gt => "gt",
            Self::Ge => "ge",
            Self::IsTruthy => "is_truthy",
            Self::IncRefObj => "inc_ref_obj",
            Self::DecRefObj => "dec_ref_obj",
            Self::IntFromI64 => "int_from_i64",
        }
    }
}
