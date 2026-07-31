use support::{Identifier, normalize};

macro_rules! tracked {
    ($value:expr) => {
        $value
    };
}

pub trait Store {
    type Error;

    fn save(&mut self, value: String) -> Result<Identifier, Self::Error>;

    fn label(&self) -> &'static str {
        "store"
    }
}

#[derive(Default)]
pub struct MemoryStore {
    pub entries: Vec<String>,
    writes: usize,
}

impl Store for MemoryStore {
    type Error = std::convert::Infallible;

    fn save(&mut self, value: String) -> Result<Identifier, Self::Error> {
        self.entries.push(value);
        self.writes += 1;
        Ok(Identifier(self.writes as u64))
    }
}

pub enum Event<T> {
    Created(T),
    Ignored,
}

pub fn process<S: Store>(
    store: &mut S,
    value: String,
) -> Result<Event<Identifier>, S::Error> {
    let normalized = tracked!(normalize(value));
    let identifier = store.save(normalized)?;
    Ok(Event::Created(identifier))
}

pub fn dynamic_process(
    store: &mut dyn Store<Error = std::convert::Infallible>,
    value: String,
) -> Event<Identifier> {
    let identifier = store.save(value).expect("infallible store");
    Event::Created(identifier)
}

pub fn concrete_process(
    store: &mut MemoryStore,
    value: String,
) -> Result<Identifier, std::convert::Infallible> {
    store.save(value)
}

pub async fn async_label<S: Store>(store: &S) -> &'static str {
    store.label()
}

/// # Safety
///
/// The caller must provide a valid pointer.
pub unsafe fn read_pointer(pointer: *const u8) -> u8 {
    unsafe { *pointer }
}

#[local_macro::tag]
pub fn attributed() -> &'static str {
    "tagged"
}

#[cfg(feature = "fancy")]
pub fn feature_only() -> &'static str {
    "fancy"
}

#[cfg(test)]
mod tests {
    use super::{Event, MemoryStore, process};

    #[test]
    fn stores_an_event() {
        let mut store = MemoryStore::default();
        let event = process(&mut store, " value ".to_owned()).unwrap();
        assert!(matches!(event, Event::Created(_)));
        assert_eq!(store.entries.len(), 1);
    }
}
