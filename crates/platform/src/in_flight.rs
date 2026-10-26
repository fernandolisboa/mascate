//! What is on its way to a Platform, so a second click while the first is
//! still sending never sends it again.

use std::collections::HashSet;
use std::hash::Hash;
use std::sync::Mutex;

/// The keys of what is being sent right now.
pub struct InFlight<K> {
    keys: Mutex<HashSet<K>>,
}

impl<K> Default for InFlight<K> {
    fn default() -> Self {
        Self {
            keys: Mutex::new(HashSet::new()),
        }
    }
}

impl<K: Eq + Hash + Clone> InFlight<K> {
    /// Marks `key` as on its way for as long as the guard lives; `None`
    /// while it already is.
    pub fn start(&self, key: K) -> Option<Sending<'_, K>> {
        let inserted = self
            .keys
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(key.clone());
        // A guard made for a key already on its way would let it go when
        // dropped, so none is made.
        if inserted {
            Some(Sending { set: self, key })
        } else {
            None
        }
    }
}

/// One send on its way; dropping it lets the key go again.
#[must_use = "the send is only guarded while this lives"]
pub struct Sending<'a, K: Eq + Hash> {
    set: &'a InFlight<K>,
    key: K,
}

impl<K: Eq + Hash> Drop for Sending<'_, K> {
    fn drop(&mut self) {
        self.set
            .keys
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_goes_once_at_a_time_and_again_after() {
        let in_flight = InFlight::default();
        let first = in_flight.start("answer");
        assert!(first.is_some());
        assert!(in_flight.start("answer").is_none());
        assert!(in_flight.start("other").is_some());

        drop(first);

        assert!(in_flight.start("answer").is_some());
    }
}
