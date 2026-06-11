use anyhow::Result;

pub fn print_schema(command: &str) -> Result<()> {
    let schema = match command {
        "run" => schemars::schema_for!(bgrun_proto::RunArgs),
        "kill" => schemars::schema_for!(bgrun_proto::KillArgs),
        "tail" => schemars::schema_for!(bgrun_proto::TailArgs),
        _ => anyhow::bail!("unknown command: {command}"),
    };
    println!("{}", serde_json::to_string_pretty(&schema)?);
    Ok(())
}
