use std::sync::{Mutex, MutexGuard, PoisonError};

/// Lock a mutex, ignoring poisoning: a fake must keep working after a test thread panicked.
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}
