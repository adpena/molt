use crate::Error;
use mlir_sys::{
    MlirDiagnosticSeverity, MlirDiagnosticSeverity_MlirDiagnosticError,
    MlirDiagnosticSeverity_MlirDiagnosticNote, MlirDiagnosticSeverity_MlirDiagnosticRemark,
    MlirDiagnosticSeverity_MlirDiagnosticWarning,
};

/// Diagnostic severity.
#[derive(Clone, Copy, Debug)]
pub enum DiagnosticSeverity {
    Error,
    Note,
    Remark,
    Warning,
}

impl DiagnosticSeverity {
    pub(crate) fn from_raw(severity: MlirDiagnosticSeverity) -> Result<Self, Error> {
        #[allow(non_upper_case_globals)]
        Ok(match severity {
            MlirDiagnosticSeverity_MlirDiagnosticError => Self::Error,
            MlirDiagnosticSeverity_MlirDiagnosticNote => Self::Note,
            MlirDiagnosticSeverity_MlirDiagnosticRemark => Self::Remark,
            MlirDiagnosticSeverity_MlirDiagnosticWarning => Self::Warning,
            _ => return Err(Error::UnknownDiagnosticSeverity(severity as u32)),
        })
    }
}

impl TryFrom<u32> for DiagnosticSeverity {
    type Error = Error;

    fn try_from(severity: u32) -> Result<Self, Error> {
        Self::from_raw(severity as MlirDiagnosticSeverity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_severity_accepts_the_mlir_sys_raw_type() {
        assert!(matches!(
            DiagnosticSeverity::from_raw(MlirDiagnosticSeverity_MlirDiagnosticWarning),
            Ok(DiagnosticSeverity::Warning)
        ));
    }
}
