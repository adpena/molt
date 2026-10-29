use super::*;

pub(super) fn upsert_extra_env(envs: &mut Vec<(String, String)>, name: &str, value: String) {
    let mut replacement = Some(value);
    envs.retain_mut(|(key, current)| {
        if key.as_str() != name {
            true
        } else if let Some(value) = replacement.take() {
            *current = value;
            true
        } else {
            // A manifest override owns one value at the original first slot.
            // Retaining another ambient value would make adoption ambiguous.
            false
        }
    });
    if let Some(value) = replacement {
        envs.push((name.to_string(), value));
    }
}

/// Install the producer's projection with the same overwrite semantics as a
/// native launch. Omitted optional policy entries retain their ambient values.
pub(super) fn wasi_environment(
    ambient: impl IntoIterator<Item = (String, String)>,
    launch_environment: &[(String, String)],
    extra_envs: &[(String, String)],
    execution_target: Option<&str>,
) -> Vec<(String, String)> {
    let mut environment = ambient.into_iter().collect::<Vec<_>>();
    let Some(target) = execution_target else {
        // Raw WASI commands retain the original append-only environment,
        // including duplicate entries and their order, without a Molt policy.
        environment.extend(extra_envs.iter().cloned());
        return environment;
    };
    for (name, value) in launch_environment.iter().chain(extra_envs) {
        upsert_extra_env(&mut environment, name, value.clone());
    }
    upsert_extra_env(
        &mut environment,
        "MOLT_EXECUTION_TARGET",
        target.to_string(),
    );
    environment
}

pub(super) fn build_wasi_ctx(
    launch_environment: &[(String, String)],
    extra_envs: &[(String, String)],
    guest_args: &[String],
    execution_target: Option<&str>,
) -> Result<WasiP1Ctx> {
    let envs = wasi_environment(
        env::vars(),
        launch_environment,
        extra_envs,
        execution_target,
    );
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
