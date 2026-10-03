//! Typed pull ports. Providers and queries belong to the producing level;
//! L0 knows only the fact and the allowed shapes of a response.
use crate::{Fact, Level};
use async_trait::async_trait;
use std::collections::BTreeMap;

mod sealed {
    use super::*;
    pub trait Value<F: Fact> {}
    impl<F: Fact> Value<F> for F {}
    impl<F: Fact> Value<F> for Option<F> {}
    impl<F: Fact> Value<F> for Vec<F> {}
    impl<F: Fact> Value<F> for BTreeMap<String, F> {}
    impl<F: Fact> Value<F> for BTreeMap<String, Vec<F>> {}
}

/// A fact or a collection of that exact fact, never an arbitrary DTO.
///
/// ```compile_fail
/// use factory_kernel::{DaemonConfigFact, FactValue};
/// fn response<T: FactValue<DaemonConfigFact>>() {}
/// response::<String>();
/// ```
pub trait FactValue<F: Fact>: sealed::Value<F> + Send {}
impl<F: Fact, T: sealed::Value<F> + Send> FactValue<F> for T {}

/// Identifies the level that owns a provider implementation.
pub trait FactProvider {
    type Level: Level;
}

/// A producing level's live read. Query parameters preserve selective
/// names, scope and time windows; the response remains typed fact data.
/// Errors are supplied by the host until the shared error vocabulary moves
/// to L0. No cache, persistence or provider registry lives here.
///
/// A provider from the wrong level cannot implement a fact's port:
/// ```compile_fail
/// use factory_kernel::{FactProvider, Provide, DaemonConfigFact, L2};
/// struct Wrong;
/// impl FactProvider for Wrong { type Level = L2; }
/// #[async_trait::async_trait]
/// impl Provide<DaemonConfigFact> for Wrong {
///     type Query = (); type Value = DaemonConfigFact; type Error = ();
///     async fn get(&self, _: &()) -> Result<DaemonConfigFact, ()> {
///         Ok(DaemonConfigFact::default())
///     }
/// }
/// ```
#[async_trait]
pub trait Provide<F: Fact>: FactProvider<Level = F::Producer> + Send + Sync {
    type Query: Sync;
    type Value: FactValue<F>;
    type Error;
    async fn get(&self, query: &Self::Query) -> Result<Self::Value, Self::Error>;
}
