//! Canonical physical stores: no shared table selector across the ladder.
pub(crate) use factory_infrastructure::expiry_store::ObservationStore as InfrastructureExpiryStore;
pub(crate) use factory_environment::expiry_store::ObservationStore as CredentialExpiryStore;
pub(crate) use factory_direction::renewal_store::AlertStore;
