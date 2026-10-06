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

/// A reader's checked access to a host that builds providers (#193 phase 6).
///
/// The host stays private. The only way to reach it from here is `reach::<F>()`,
/// which names the fact being asked for and compiles only when that fact's
/// producer is strictly below the reader (`F::Producer: Below<L>`): the same
/// sealed relation `Facts::get` uses. A service holding a `Wired<L2, _>` therefore
/// cannot build the provider of an L4 fact, and a same-level fact is a plain call
/// inside the service, not a provider lookup.
pub struct Wired<'a, L: Reader, H: ?Sized> {
    host: &'a H,
    reader: PhantomData<L>,
}
impl<'a, L: Reader, H: ?Sized> Wired<'a, L, H> {
    pub fn new(host: &'a H) -> Self {
        Self { host, reader: PhantomData }
    }
    /// The host, for building the provider of `F`. Only a producer below `L` is allowed.
    pub fn reach<F: Fact>(&self) -> &'a H
    where
        F::Producer: Below<L>,
    {
        self.host
    }
}
impl<L: Reader, H: ?Sized> Clone for Wired<'_, L, H> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<L: Reader, H: ?Sized> Copy for Wired<'_, L, H> {}

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
