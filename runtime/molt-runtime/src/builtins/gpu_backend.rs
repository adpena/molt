pub(crate) const MOLT_GPU_BACKEND_ENV: &str = "MOLT_GPU_BACKEND";

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum GpuBackend {
    Metal,
    WebGpu,
    Cuda,
    Hip,
}

impl GpuBackend {
    pub(crate) fn from_env_value(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "metal" => Some(Self::Metal),
            "webgpu" => Some(Self::WebGpu),
            "cuda" => Some(Self::Cuda),
            "hip" => Some(Self::Hip),
            "" => None,
            _ => None,
        }
    }
}

pub(crate) fn requested_gpu_backend() -> Option<GpuBackend> {
    std::env::var(MOLT_GPU_BACKEND_ENV)
        .ok()
        .and_then(|raw| GpuBackend::from_env_value(&raw))
}

#[cfg(test)]
mod tests {
    use super::GpuBackend;

    #[test]
    fn gpu_backend_from_env_value_normalizes_explicit_backends() {
        assert_eq!(
            GpuBackend::from_env_value("  METAL "),
            Some(GpuBackend::Metal)
        );
        assert_eq!(
            GpuBackend::from_env_value("  WEBGPU "),
            Some(GpuBackend::WebGpu)
        );
        assert_eq!(GpuBackend::from_env_value(" cuda "), Some(GpuBackend::Cuda));
        assert_eq!(GpuBackend::from_env_value(" HIP "), Some(GpuBackend::Hip));
        assert_eq!(GpuBackend::from_env_value(" "), None);
        assert_eq!(GpuBackend::from_env_value("native"), None);
        assert_eq!(GpuBackend::from_env_value("web-gpu"), None);
    }
}
