use app::{Event, MemoryStore, process};

#[test]
fn integration_flow() {
    let mut store = MemoryStore::default();
    assert!(matches!(
        process(&mut store, "event".to_owned()).unwrap(),
        Event::Created(_)
    ));
}
