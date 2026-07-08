use super::*;

fn ensure_locale_env(envs: &mut Vec<(String, String)>) {
    let has_locale = envs.iter().any(|(k, _)| {
        k == "MOLT_WASM_LOCALE_DECIMAL"
            || k == "MOLT_WASM_LOCALE_THOUSANDS"
            || k == "MOLT_WASM_LOCALE_GROUPING"
    });
    if has_locale {
        return;
    }
    let locale = match SystemLocale::default() {
        Ok(locale) => locale,
        Err(_) => return,
    };
    envs.push((
        "MOLT_WASM_LOCALE_DECIMAL".to_string(),
        locale.decimal().to_string(),
    ));
    let sep = locale.separator().to_string();
    if !sep.is_empty() {
        envs.push(("MOLT_WASM_LOCALE_THOUSANDS".to_string(), sep));
        let grouping = match locale.grouping() {
            Grouping::Posix => None,
            Grouping::Standard | Grouping::Indian => Some("3"),
        };
        if let Some(grouping) = grouping {
            envs.push((
                "MOLT_WASM_LOCALE_GROUPING".to_string(),
                grouping.to_string(),
            ));
        }
    }
}

pub(super) fn upsert_extra_env(envs: &mut Vec<(String, String)>, name: &str, value: String) {
    if let Some((_, current)) = envs.iter_mut().find(|(key, _)| key == name) {
        *current = value;
    } else {
        envs.push((name.to_string(), value));
    }
}

pub(super) fn build_wasi_ctx(
    extra_envs: &[(String, String)],
    guest_args: &[String],
) -> Result<WasiP1Ctx> {
    let mut envs = env::vars().collect::<Vec<_>>();
    ensure_locale_env(&mut envs);
    envs.extend(extra_envs.iter().cloned());
    let mut builder = WasiCtxBuilder::new();
    builder.inherit_stdio();
    builder.envs(&envs);
    if guest_args.is_empty() {
        builder.inherit_args();
    } else {
        // Pass only the guest-facing args: ["app", route, query, ...]
        let mut wasi_args: Vec<String> = vec!["app".to_string()];
        wasi_args.extend(guest_args.iter().cloned());
        builder.args(&wasi_args);
    }
    builder.preopened_dir(".", ".", DirPerms::all(), FilePerms::all())?;
    Ok(builder.build_p1())
}
