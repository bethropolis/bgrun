use anyhow::{bail, Result};
use std::fs;
use std::path::PathBuf;

const SKILL: &str = include_str!("../../../../docs/bgrun/SKILL.md");

pub fn install(target_dir: PathBuf) -> Result<()> {
    if target_dir.exists() && !target_dir.is_dir() {
        bail!("Target path is not a directory: {}", target_dir.display());
    }

    fs::create_dir_all(&target_dir)?;
    let skill_path = target_dir.join("SKILL.md");
    fs::write(&skill_path, SKILL)?;
    println!("Installed skill to {}", skill_path.display());
    Ok(())
}
