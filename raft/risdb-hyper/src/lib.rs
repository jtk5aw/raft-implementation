use std::io;
use std::path::{Path, PathBuf};

// Make this a workspace wide helper function rather than copying it around
pub fn get_workspace_base_dir() -> PathBuf {
    let output = std::process::Command::new(env!("CARGO"))
        .arg("locate-project")
        .arg("--workspace")
        .arg("--message-format=plain")
        .output()
        .unwrap()
        .stdout;
    let cargo_path = Path::new(std::str::from_utf8(&output).unwrap().trim());
    cargo_path.parent().unwrap().to_path_buf()
}

pub fn error(err: String) -> io::Error {
    io::Error::new(io::ErrorKind::Other, err)
}