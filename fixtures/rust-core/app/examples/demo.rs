fn main() {
    let mut store = app::MemoryStore::default();
    let _ = app::process(&mut store, "demo".to_owned());
}
