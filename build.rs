use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn git_commit_short() -> String {
    let output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output();
    match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        _ => "unknown".to_string(),
    }
}

fn build_unix_timestamp() -> String {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(dur) => dur.as_secs().to_string(),
        Err(_) => "0".to_string(),
    }
}

fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    println!("cargo:rustc-env=MDTALK_GIT_COMMIT={}", git_commit_short());
    println!(
        "cargo:rustc-env=MDTALK_BUILD_UNIX={}",
        build_unix_timestamp()
    );
}
