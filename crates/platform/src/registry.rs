//! What modules declare in code for the app to compose: each module lists
//! its own and the app joins the lists, like migrations.

/// Something a module declares under a stable key, used in storage.
pub trait Registered: Copy {
    fn key(&self) -> &'static str;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("two declarations share the key {0}")]
pub struct DuplicateKey(pub &'static str);

/// Every module's declarations of one kind, in the order they were given.
#[derive(Debug, Clone)]
pub struct Registry<T> {
    items: Vec<T>,
}

impl<T: Registered> Registry<T> {
    /// Joins each module's list. Keys are stored, so two that collide would
    /// share their state: that fails here instead.
    pub fn new(modules: &[&[T]]) -> Result<Self, DuplicateKey> {
        let mut items: Vec<T> = Vec::new();
        for item in modules.iter().flat_map(|module| module.iter().copied()) {
            if items.iter().any(|known| known.key() == item.key()) {
                return Err(DuplicateKey(item.key()));
            }
            items.push(item);
        }
        Ok(Self { items })
    }

    pub fn all(&self) -> &[T] {
        &self.items
    }

    pub fn get(&self, key: &str) -> Option<T> {
        self.items.iter().copied().find(|item| item.key() == key)
    }
}
