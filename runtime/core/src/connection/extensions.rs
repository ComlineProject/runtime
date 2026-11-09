use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use core::any::{Any, TypeId};
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::contract::ConnectionId;

/// Typed data kept on a connection, one value per type, like
/// `http::Extensions`: what the application knows about the peer (a session,
/// a user id) or keeps for it (a rate-limit bucket). Values are `Send`, so
/// an async connection's handlers on any thread can reach them.
#[derive(Default)]
pub struct Extensions {
    values: BTreeMap<TypeId, Box<dyn Any + Send>>,
}

impl Extensions {
    /// Store `value`, returning the one of its type stored before.
    pub fn insert<T: Send + 'static>(&mut self, value: T) -> Option<T> {
        self.values
            .insert(TypeId::of::<T>(), Box::new(value))
            .and_then(|old| old.downcast().ok().map(|old| *old))
    }

    pub fn get<T: 'static>(&self) -> Option<&T> {
        self.values.get(&TypeId::of::<T>())?.downcast_ref()
    }

    pub fn get_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.values.get_mut(&TypeId::of::<T>())?.downcast_mut()
    }

    pub fn remove<T: 'static>(&mut self) -> Option<T> {
        self.values
            .remove(&TypeId::of::<T>())
            .and_then(|old| old.downcast().ok().map(|old| *old))
    }
}

impl core::fmt::Debug for Extensions {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Extensions")
            .field("len", &self.values.len())
            .finish()
    }
}

pub(crate) fn next_connection_id() -> ConnectionId {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    ConnectionId(NEXT.fetch_add(1, Ordering::Relaxed) as u64)
}
