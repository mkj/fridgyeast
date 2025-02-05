use std::process::Command;

fn main() {
    git();
    rustver();
}

fn git() {
    let git_rev = Command::new("git")
        .args(["describe", "--always", "--tags", "--dirty=+"])
        .output().unwrap().stdout;
    let git_rev = String::from_utf8(git_rev).unwrap();

    println!("cargo:rustc-env=GIT_REV={git_rev}");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads/main");
}

fn rustver() {
    let ver = Command::new(std::env::var("RUSTC").unwrap())
        .args(["--version"])
        .output().unwrap().stdout;
    let ver = String::from_utf8(ver).unwrap();
    println!("cargo:rustc-env=RUSTC_VER={ver}");
}
