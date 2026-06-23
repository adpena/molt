use super::types::TirType;

/// Unique identifier for an SSA value within a function.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize, serde::Serialize,
)]
pub struct ValueId(pub u32);

/// A typed SSA value.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct TirValue {
    pub id: ValueId,
    pub ty: TirType,
}

impl std::fmt::Display for ValueId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "%{}", self.0)
    }
}
