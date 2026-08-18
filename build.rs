fn main() {
    let commit = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|finished| finished.status.success())
        .map(|finished| String::from_utf8_lossy(&finished.stdout).trim().to_string())
        .unwrap_or_default();

    println!("cargo:rustc-env=QWATCH_COMMIT={commit}");
    println!("cargo:rerun-if-changed=.git/HEAD");
}
