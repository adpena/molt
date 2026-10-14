use super::*;

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
    envs.extend(extra_envs.iter().cloned());
    let mut builder = WasiCtxBuilder::new();
    builder.inherit_stdio();
    builder.envs(&envs);
    // Keep host flags and the selected manifest/module path out of guest argv,
    // including the zero-tail case. Both application and command execution use
    // this single canonical guest-facing argv authority.
    let mut wasi_args: Vec<String> = vec!["app".to_string()];
    wasi_args.extend(guest_args.iter().cloned());
    builder.args(&wasi_args);
    builder.preopened_dir(".", ".", FsPerms::ReadWrite)?;
    Ok(builder.build_p1())
}
