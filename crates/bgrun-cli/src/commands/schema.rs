use anyhow::Result;

pub fn print_schema(command: Option<&str>) -> Result<()> {
    let schema = match command {
        Some("run") => schemars::schema_for!(bgrun_proto::RunArgs),
        Some("kill") => schemars::schema_for!(bgrun_proto::KillArgs),
        Some("tail") => schemars::schema_for!(bgrun_proto::TailArgs),
        None => schemars::schema_for!(bgrun_proto::Command),
        Some(other) => anyhow::bail!("unknown command: {other} (valid: run, kill, tail)"),
    };
    println!("{}", serde_json::to_string_pretty(&schema)?);
    Ok(())
}
