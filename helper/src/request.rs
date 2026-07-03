use std::path::{Path, PathBuf};

pub fn request_path() -> PathBuf {
    crate::state::temp_path("vlc-pip-request.txt")
}

pub fn consume(path: &Path) -> Option<String> {
    let cmd = std::fs::read_to_string(path).ok()?; // missing or mid-write: retry next poll
    std::fs::remove_file(path).ok()?; // couldn't delete: leave the command for next poll
    let cmd = cmd.trim();
    if cmd.is_empty() { None } else { Some(cmd.to_string()) }
}
