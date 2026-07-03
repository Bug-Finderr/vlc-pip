use super::*;

fn tmp(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("pip-req-test-{name}-{}.txt", std::process::id()))
}

#[test]
fn consume_reads_command_and_deletes_file() {
    let path = tmp("consume");
    std::fs::write(&path, "toggle\r\n").unwrap();
    assert_eq!(consume(&path).as_deref(), Some("toggle"));
    assert!(!path.exists());
}

#[test]
fn consume_missing_file_returns_none() {
    assert_eq!(consume(&tmp("nope")), None);
}

#[test]
fn consume_empty_file_is_deleted_and_none() {
    let path = tmp("empty");
    std::fs::write(&path, "  \r\n").unwrap();
    assert_eq!(consume(&path), None);
    assert!(!path.exists());
}
