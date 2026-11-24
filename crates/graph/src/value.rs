//! Port values: what flows along an edge.

use std::any::Any;
use std::fmt;
use std::sync::Arc;

use crate::PortType;

/// A value on a port: a [`PortType`] tag plus a type-erased, shared payload.
///
/// Type erasure (rather than an enum of payloads) because the v0.1 payload types live
/// in crates that don't exist yet or sit beside `graph` (`mol` arena, polars tables,
/// `chem` ligand sets), and plugin nodes will bring their own. `graph` only moves
/// values around; the node that produces and the node that consumes agree on the
/// concrete Rust type for a `PortType` (a convention the `nodes` crate owns).
///
/// The payload sits behind an `Arc`: fanning one output out to several consumers, and
/// keeping every intermediate inspectable, costs a refcount, never a copy.
#[derive(Clone)]
pub struct Value {
    ty: PortType,
    data: Arc<dyn Any + Send + Sync>,
}

impl Value {
    pub fn new<T: Any + Send + Sync>(ty: PortType, data: T) -> Self {
        Self {
            ty,
            data: Arc::new(data),
        }
    }

    pub fn from_arc<T: Any + Send + Sync>(ty: PortType, data: Arc<T>) -> Self {
        Self { ty, data }
    }

    pub fn ty(&self) -> PortType {
        self.ty
    }

    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        self.data.downcast_ref::<T>()
    }

    pub fn downcast_arc<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        self.data.clone().downcast::<T>().ok()
    }

    /// Whether two values share the same payload allocation.
    pub fn ptr_eq(&self, other: &Value) -> bool {
        Arc::ptr_eq(&self.data, &other.data)
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Value({:?})", self.ty)
    }
}
