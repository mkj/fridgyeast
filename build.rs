use std::process::Command;
use std::env;
use std::fs;
use std::path::Path;

fn main() {
    let revid = Command::new("hg").args(&["id", "-i"])
    .output()
    .map(|o| String::from_utf8(o.stdout).unwrap())
    .unwrap_or("(no hg revid)".to_string());

    let out_dir = env::var_os("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("hg-revid.txt");
    fs::write(
        &dest_path,
        revid).unwrap();
    println!("cargo:rerun-if-changed=.hg");
}

