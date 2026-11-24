//! Entry point for `bgrun -i`: the full-screen Ratatui interface.

use anyhow::Result;

pub async fn start_menu() -> Result<()> {
    crate::tui::run().await
}
