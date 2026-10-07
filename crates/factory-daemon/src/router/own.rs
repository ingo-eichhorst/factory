//! Requests served by the entry point itself (no level owns these). The arms are moved verbatim from the single
//! `dispatch_request` match; `Request::level` decides which file a request lands in.
use crate::engine::*;
use super::misrouted;

impl Engine {
    pub(super) async fn route_own(
        &self,
        _caller: &crate::access::Caller,
        req: Request,
    ) -> Result<Payload> {
        match req {
            Request::Status => Ok(Payload::Status {
                status: self.status().await?,
            }),
            Request::Adapters => Ok(self.shared.registry.list().into()),

            Request::Subscribe => Err(FactoryError::BadRequest(
                "this interface does not stream events on the request channel".into(),
            )),
            other => Err(misrouted(other.level())),
        }
    }
}
