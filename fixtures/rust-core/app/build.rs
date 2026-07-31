fn main() {
    println!("cargo:rerun-if-env-changed=GRAPHINE_TEST_SENTINEL_PATH");
    if let Ok(path) = std::env::var("GRAPHINE_TEST_SENTINEL_PATH") {
        std::fs::write(path, "build script executed").expect("write trusted-mode sentinel");
    }
}
