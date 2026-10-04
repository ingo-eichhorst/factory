//! Compiler-checked adjacent command edges, separate from upward fact reads.
use crate::{Level, L1, L2, L3, L4, L5, L6};
use std::marker::PhantomData;

mod sealed {
    pub trait DirectlyBelow<R> {}
}
/// Exactly five command edges. This relation cannot be extended by a consumer.
pub trait DirectlyBelow<R: Level>: Level + sealed::DirectlyBelow<R> {}
macro_rules! edge {
    ($lower:ty, $upper:ty) => {
        impl sealed::DirectlyBelow<$upper> for $lower {}
        impl DirectlyBelow<$upper> for $lower {}
    };
}
edge!(L1, L2);
edge!(L2, L3);
edge!(L3, L4);
edge!(L4, L5);
edge!(L5, L6);
/// A level-owned command port. Domain traits return acknowledgements and ids.
pub trait CommandPort {
    type Level: Level;
}
/// A service may hold only its directly lower level's command port.
pub struct Commands<R: Level, P: CommandPort>
where
    P::Level: DirectlyBelow<R>,
{
    port: P,
    caller: PhantomData<R>,
}
impl<R: Level, P: CommandPort> Commands<R, P>
where
    P::Level: DirectlyBelow<R>,
{
    pub fn new(port: P) -> Self {
        Self {
            port,
            caller: PhantomData,
        }
    }
    pub fn port(&self) -> &P {
        &self.port
    }
}
impl<P: CommandPort + ?Sized> CommandPort for &P {
    type Level = P::Level;
}
/// Acknowledgement only: no task state, status, configuration or report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskReceipt {
    pub id: String,
}
