use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize)]
pub struct HealthResponse {
    pub ok: bool,
}

#[derive(Serialize, Deserialize)]
pub struct OffloadTableRequest {
    pub rows: usize,
}

#[derive(Serialize, Deserialize)]
pub struct OffloadTableResponse {
    pub rows: usize,
    pub sample: Vec<HashMap<String, i64>>,
}

#[derive(Serialize, Deserialize)]
pub struct ComputeRequest {
    pub values: Vec<f64>,
    pub scale: Option<f64>,
    pub offset: Option<f64>,
}

#[derive(Serialize, Deserialize)]
pub struct ComputeResponse {
    pub count: usize,
    pub sum: f64,
    pub scaled: Vec<f64>,
}
