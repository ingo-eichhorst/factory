//! Typed pull ports. Providers and queries belong to the producing level;
//! L0 knows only the fact and the allowed shapes of a response.
use crate::{Fact, Level, L1, L2, L3, L4, L5, L6};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::marker::PhantomData;

/// A fact reader: one of the levels, or the interfaces beside the ladder.
pub trait Reader {}
impl<L: Level> Reader for L {}

/// Page/API composition is outside the ladder, not an invented L7 or an
/// exemption for an internal level. It may read any producer's facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct People;
impl Reader for People {}

/// A producer strictly below the reading level. Same-level calls stay
/// inside their service; downward fact reads are forbidden.
///
/// The relation is sealed: consumers cannot grant themselves another edge.
/// There are exactly fifteen level-to-level edges, plus six people-side reads.
pub trait Below<R: Reader>: Level + sealed::Below<R> {}

macro_rules! below {
    ($producer:ty => $($reader:ty),+ $(,)?) => {
        $(impl sealed::Below<$reader> for $producer {}
          impl Below<$reader> for $producer {})+
    };
}
below!(L1 => L2, L3, L4, L5, L6, People);
below!(L2 => L3, L4, L5, L6, People);
below!(L3 => L4, L5, L6, People);
below!(L4 => L5, L6, People);
below!(L5 => L6, People);
below!(L6 => People);

/// The compiler-checked read boundary, independent of a provider registry
/// or host. The host supplies the producer; this handle neither gathers nor
/// persists evidence and cannot change who owns the fact.
pub struct Facts<R: Reader> {
    reader: PhantomData<R>,
}
impl<R: Reader> Default for Facts<R> {
    fn default() -> Self {
        Self { reader: PhantomData }
    }
}
impl<R: Reader> Facts<R> {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn get<F, P>(&self, provider: &P, query: &P::Query) -> Result<P::Value, P::Error>
    where
        F: Fact,
        F::Producer: Below<R>,
        P: Provide<F>,
    {
        provider.get(query).await
    }
}

mod sealed {
    use super::*;
    pub trait Below<R: Reader> {}
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
