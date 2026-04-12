//! Adapters that bridge existing typed service traits into the generic
//! [`DynService`] interface.
//!
//! These allow the four built-in service archetypes (communication, calendar,
//! drive, contacts) to participate in the dynamic service registry without
//! rewriting any provider logic.

mod calendar_adapter;
mod communication_adapter;
mod contacts_adapter;
mod drive_adapter;

pub use calendar_adapter::CalendarServiceAdapter;
pub use communication_adapter::CommunicationServiceAdapter;
pub use contacts_adapter::ContactsServiceAdapter;
pub use drive_adapter::DriveServiceAdapter;
