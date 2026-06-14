use anyhow::{bail, Context, Result};
use std::path::PathBuf;

const SKILL: &str = include_str!("../../../../docs/bgrun/SKILL.md");

const PRESETS: &[(&str, &str, &str)] = &[
    ("opencode", "~/.config/opencode/skills", ".opencode/skills"),
    ("claude", "~/.claude/skills", ".claude/skills"),
    ("codex", "~/.codex/skills", ".codex/skills"),
];

pub async fn install(target: String, project: bool) -> Result<()> {
    let dirs = resolve_targets(&target, project)?;

    for dir in &dirs {
        tokio::fs::create_dir_all(dir)
            .await
            .with_context(|| format!("failed to create directory: {}", dir.display()))?;
        let skill_path = dir.join("SKILL.md");
        tokio::fs::write(&skill_path, SKILL)
            .await
            .with_context(|| format!("failed to write {}", skill_path.display()))?;
        println!("Installed skill to {}", skill_path.display());
    }

    if dirs.is_empty() {
        bail!("no targets matched for '{target}'");
    }

    Ok(())
}

fn resolve_targets(target: &str, project: bool) -> Result<Vec<PathBuf>> {
    if target == "all" {
        return PRESETS
            .iter()
            .map(|(_, global, proj)| {
                let base = if project { proj } else { global };
                resolve_path(base).map(|p| p.join("bgrun"))
            })
            .collect::<Result<Vec<_>>>();
    }

    for (name, global, proj) in PRESETS {
        if *name == target {
            let base = if project { proj } else { global };
            let dir = resolve_path(base)?.join("bgrun");
            return Ok(vec![dir]);
        }
    }

    Ok(vec![PathBuf::from(target)])
}

fn resolve_path(path: &str) -> Result<PathBuf> {
    if let Some(rest) = path.strip_prefix("~/") {
        let home = std::env::var("HOME")
            .map_err(|_| anyhow::anyhow!("HOME environment variable not set"))?;
        Ok(PathBuf::from(home).join(rest))
    } else if path.starts_with('.') {
        let cwd = std::env::current_dir()
            .map_err(|_| anyhow::anyhow!("failed to get current working directory"))?;
        Ok(cwd.join(path))
    } else {
        Ok(PathBuf::from(path))
    }
}
