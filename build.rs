use std::process::Command;
use std::env;
use std::fs;
use std::path::Path;

fn main() {
    git()
}


fn git() {
    let git_rev = Command::new("git")
        .args(["describe", "--always", "--tags", "--dirty=+"])
        .output()
        .map(|o| String::from_utf8(o.stdout).unwrap())
        .unwrap_or("(unknown)".to_string());

    println!("cargo:rustc-env=GIT_REV={git_rev}");
    println!("cargo:rerun-if-changed=.git/HEAD");
}
