use std::sync::OnceLock;

pub(crate) fn pep649_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        let raw = match std::env::var("MOLT_SYS_VERSION_INFO") {
            Ok(val) => val,
            Err(_) => return true,
        };
        let mut parts = raw.split(',');
        let major = parts
            .next()
            .and_then(|v| v.trim().parse::<i64>().ok())
            .unwrap_or(3);
        let minor = parts
            .next()
            .and_then(|v| v.trim().parse::<i64>().ok())
            .unwrap_or(14);
        major > 3 || (major == 3 && minor >= 14)
    })
}
