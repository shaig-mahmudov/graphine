#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identifier(pub u64);

pub fn normalize(value: String) -> String {
    value.trim().to_owned()
}
