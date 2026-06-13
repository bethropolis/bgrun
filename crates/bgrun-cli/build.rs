use std::process::Command;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rerun-if-changed={manifest_dir}/../../.git/HEAD");

    let desc = Command::new("git")
        .args(["describe", "--tags", "--dirty", "--always"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            (!s.is_empty()).then_some(s)
        });

    let hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            (!s.is_empty()).then_some(s)
        });

    let version = match (desc, hash) {
        (Some(d), Some(h)) if d.ends_with("-dirty") => {
            let base = d.strip_suffix("-dirty").unwrap_or(&d);
            format!("{base}+g{h}")
        }
        (Some(d), _) => d,
        (None, Some(h)) => format!("g{h}"),
        (None, None) => return,
    };

    println!("cargo:rustc-env=BGRUN_VERSION={version}");
}
