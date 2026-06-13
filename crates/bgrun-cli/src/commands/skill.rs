use anyhow::{bail, Result};
use std::path::PathBuf;

const SKILL: &str = include_str!("../../../../docs/bgrun/SKILL.md");

/// Installs the embedded skill to a target directory asynchronously.
pub async fn install(target_dir: PathBuf) -> Result<()> {
    if target_dir.exists() && !target_dir.is_dir() {
        bail!("Target path is not a directory: {}", target_dir.display());
    }

    tokio::fs::create_dir_all(&target_dir).await?;
    let skill_path = target_dir.join("SKILL.md");
    tokio::fs::write(&skill_path, SKILL).await?;
    println!("Installed skill to {}", skill_path.display());
    Ok(())
}
