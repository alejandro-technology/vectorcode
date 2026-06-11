/// A simple key-value store with thread-safe access.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Error types for the store.
#[derive(Debug, Clone, PartialEq)]
pub enum StoreError {
    KeyNotFound(String),
    SerializationError(String),
}

/// A thread-safe key-value store.
pub struct Store {
    data: Arc<RwLock<HashMap<String, String>>>,
}

impl Store {
    /// Create a new empty store.
    pub fn new() -> Self {
        Self {
            data: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Insert a key-value pair into the store.
    pub fn insert(&self, key: &str, value: &str) -> Result<(), StoreError> {
        let mut data = self.data.write().map_err(|e| {
            StoreError::SerializationError(format!("Lock poisoned: {}", e))
        })?;
        data.insert(key.to_string(), value.to_string());
        Ok(())
    }

    /// Get a value by key.
    pub fn get(&self, key: &str) -> Result<String, StoreError> {
        let data = self.data.read().map_err(|e| {
            StoreError::SerializationError(format!("Lock poisoned: {}", e))
        })?;
        data.get(key)
            .cloned()
            .ok_or_else(|| StoreError::KeyNotFound(key.to_string()))
    }

    /// Remove a key from the store.
    pub fn remove(&self, key: &str) -> Result<String, StoreError> {
        let mut data = self.data.write().map_err(|e| {
            StoreError::SerializationError(format!("Lock poisoned: {}", e))
        })?;
        data.remove(key)
            .ok_or_else(|| StoreError::KeyNotFound(key.to_string()))
    }

    /// Check if a key exists.
    pub fn contains(&self, key: &str) -> bool {
        self.data
            .read()
            .map(|data| data.contains_key(key))
            .unwrap_or(false)
    }

    /// Get the number of entries in the store.
    pub fn len(&self) -> usize {
        self.data
            .read()
            .map(|data| data.len())
            .unwrap_or(0)
    }

    /// Check if the store is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for Store {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for Store {
    fn clone(&self) -> Self {
        Self {
            data: Arc::clone(&self.data),
        }
    }
}
