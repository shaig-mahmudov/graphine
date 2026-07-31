fn main() {
    let mut store = app::MemoryStore::default();
    for value in 0..10 {
        let _ = app::process(&mut store, value.to_string());
    }
}
