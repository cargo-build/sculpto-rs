//! NIF string table: allocate each string once, keep insertion order.
//!
//! Index 0 is the empty string, as in the reference layout. Shapes that share a
//! name share one index; the table never reorders an existing entry.

use std::collections::HashMap;

/// Insertion-ordered, deduplicating string table.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StringTable {
    entries: Vec<String>,
    lookup: HashMap<String, u32>,
}

impl StringTable {
    /// An empty table with the conventional empty string at index 0.
    pub fn new() -> Self {
        let mut table = Self::default();
        table.intern("");
        table
    }

    /// Return the index for `value`, inserting it at the end when new.
    pub fn intern(&mut self, value: &str) -> u32 {
        if let Some(index) = self.lookup.get(value) {
            return *index;
        }
        let index = u32::try_from(self.entries.len()).expect("string table fits in u32");
        self.entries.push(value.to_string());
        self.lookup.insert(value.to_string(), index);
        index
    }

    /// Look up an entry.
    pub fn get(&self, index: u32) -> Option<&str> {
        self.entries.get(index as usize).map(String::as_str)
    }

    /// Number of entries, including the empty string.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the table has no entries. A fresh table always has index 0.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Entries in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_names_share_one_entry() {
        let mut table = StringTable::new();
        let body = table.intern("Body");
        let again = table.intern("Body");
        assert_eq!(body, again);
        assert_eq!(table.len(), 2);
        assert_eq!(table.get(0), Some(""));
        assert_eq!(table.get(1), Some("Body"));
    }

    #[test]
    fn insertion_order_is_stable() {
        let mut table = StringTable::new();
        table.intern("first");
        table.intern("second");
        table.intern("first");
        assert_eq!(table.iter().collect::<Vec<_>>(), ["", "first", "second"]);
    }

    #[test]
    fn empty_string_is_index_zero() {
        let mut table = StringTable::new();
        assert_eq!(table.intern(""), 0);
        assert_eq!(table.get(0), Some(""));
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn out_of_range_is_none() {
        let table = StringTable::new();
        assert_eq!(table.get(1), None);
        assert!(!table.is_empty());
    }
}
