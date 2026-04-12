//! Apple Contacts service via the Contacts framework.

use std::ptr::NonNull;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use objc2::rc::Retained;
use objc2::runtime::{Bool, ProtocolObject};
use objc2_contacts::{
    CNAuthorizationStatus, CNContact, CNContactFetchRequest, CNContactStore, CNEntityType,
    CNKeyDescriptor,
};
use objc2_foundation::{NSArray, NSError, NSString};

use hive_classification::DataClass;
use hive_contracts::connectors::Contact;

use super::bridge::{self, SendRetained};
use crate::services::ContactsService;

/// Build the list of CNContact keys to fetch, cast to CNKeyDescriptor protocol objects.
fn fetch_keys() -> Retained<NSArray<ProtocolObject<dyn CNKeyDescriptor>>> {
    let keys: Vec<Retained<NSString>> = vec![
        NSString::from_str("givenName"),
        NSString::from_str("familyName"),
        NSString::from_str("organizationName"),
        NSString::from_str("jobTitle"),
        NSString::from_str("emailAddresses"),
        NSString::from_str("phoneNumbers"),
        NSString::from_str("identifier"),
    ];

    // NSString conforms to CNKeyDescriptor so this cast is safe.
    let protocol_keys: Vec<&ProtocolObject<dyn CNKeyDescriptor>> = keys
        .iter()
        .map(|k| {
            let ns_ref: &NSString = k;
            ProtocolObject::from_ref(ns_ref)
        })
        .collect();
    NSArray::from_slice(&protocol_keys)
}

/// Apple Contacts service backed by the Contacts framework.
pub struct AppleContacts {
    connector_id: String,
    store: SendRetained<CNContactStore>,
    /// Serialize all CNContactStore operations for thread safety.
    lock: std::sync::Arc<std::sync::Mutex<()>>,
}

impl AppleContacts {
    pub fn new(connector_id: &str) -> Self {
        let store = unsafe { CNContactStore::new() };
        Self {
            connector_id: connector_id.to_string(),
            store: SendRetained(store),
            lock: std::sync::Arc::new(std::sync::Mutex::new(())),
        }
    }
}

fn request_contacts_access(store: &CNContactStore) -> Result<()> {
    let status =
        unsafe { CNContactStore::authorizationStatusForEntityType(CNEntityType::Contacts) };
    // CNAuthorizationStatusAuthorized = full access.
    // CNAuthorizationStatusLimited (macOS 15+) = partial access to selected contacts.
    // Both are sufficient to proceed.
    if status == CNAuthorizationStatus::Authorized || status == CNAuthorizationStatus::Limited {
        return Ok(());
    }

    if status == CNAuthorizationStatus::Denied {
        anyhow::bail!(
            "Contacts access has not been granted. \
             Please open System Settings → Privacy & Security → Contacts \
             and enable access for HiveMind OS, then try again."
        );
    }

    let (tx, rx) = std::sync::mpsc::channel::<Result<()>>();
    let block: block2::RcBlock<dyn Fn(Bool, *mut NSError)> =
        block2::RcBlock::new(move |granted: Bool, error: *mut NSError| {
            if granted.as_bool() {
                let _ = tx.send(Ok(()));
            } else if !error.is_null() {
                let err_ref = unsafe { &*error };
                let desc = err_ref.localizedDescription();
                let _ = tx.send(Err(anyhow!("{}", bridge::nsstring_to_string(&desc))));
            } else {
                let _ = tx.send(Err(anyhow!(
                    "Contacts access was not granted. \
                     Please open System Settings → Privacy & Security → Contacts \
                     and enable access for HiveMind OS, then try again."
                )));
            }
        });

    unsafe {
        store.requestAccessForEntityType_completionHandler(CNEntityType::Contacts, &block);
    }

    rx.recv().map_err(|_| anyhow!("Contacts access request channel closed"))?
}

/// Convert a CNContact to our Contact contract type.
fn cncontact_to_contact(contact: &CNContact, connector_id: &str) -> Contact {
    let id = bridge::nsstring_to_string(unsafe { &contact.identifier() });

    let given_str = bridge::nsstring_to_string(unsafe { &contact.givenName() });
    let family_str = bridge::nsstring_to_string(unsafe { &contact.familyName() });
    let display_name = format!("{} {}", given_str, family_str).trim().to_string();

    let company = {
        let s = bridge::nsstring_to_string(unsafe { &contact.organizationName() });
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    };

    let job_title = {
        let s = bridge::nsstring_to_string(unsafe { &contact.jobTitle() });
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    };

    let email_addresses = {
        let emails = unsafe { contact.emailAddresses() };
        emails
            .to_vec()
            .into_iter()
            .filter_map(|labeled| {
                let value = unsafe { labeled.value() };
                let s = bridge::nsstring_to_string(&value);
                if s.is_empty() {
                    None
                } else {
                    Some(s)
                }
            })
            .collect()
    };

    let phone_numbers = {
        let phones = unsafe { contact.phoneNumbers() };
        phones
            .to_vec()
            .into_iter()
            .filter_map(|labeled| {
                let phone = unsafe { labeled.value() };
                let s = bridge::nsstring_to_string(unsafe { &phone.stringValue() });
                if s.is_empty() {
                    None
                } else {
                    Some(s)
                }
            })
            .collect()
    };

    Contact {
        id,
        connector_id: connector_id.to_string(),
        display_name,
        email_addresses,
        phone_numbers,
        company,
        job_title,
        data_class: DataClass::Internal,
    }
}

#[async_trait]
impl ContactsService for AppleContacts {
    fn name(&self) -> &str {
        "Apple Contacts"
    }

    async fn test_connection(&self) -> Result<()> {
        let store = self.store.clone();
        let lock = self.lock.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = lock.lock().map_err(|e| anyhow!("lock poisoned: {e}"))?;
            request_contacts_access(&store)
        })
        .await
        .context("Contacts test_connection task panicked")?
    }

    async fn list_contacts(&self, limit: usize, offset: usize) -> Result<Vec<Contact>> {
        let store = self.store.clone();
        let lock = self.lock.clone();
        let connector_id = self.connector_id.clone();

        tokio::task::spawn_blocking(move || {
            let _guard = lock.lock().map_err(|e| anyhow!("lock poisoned: {e}"))?;
            request_contacts_access(&store)?;

            let keys = fetch_keys();
            let request = unsafe {
                let req = CNContactFetchRequest::new();
                req.setKeysToFetch(&keys);
                req
            };

            let target_count = offset + limit;
            let mut all_contacts: Vec<Contact> = Vec::new();
            let connector_id_ref = &connector_id;

            // We use a raw pointer to the Vec so the block can append to it.
            // SAFETY: The block runs synchronously within `enumerateContacts…`
            // and does not outlive `all_contacts`.
            let contacts_ptr = &mut all_contacts as *mut Vec<Contact>;

            let block =
                block2::RcBlock::new(move |contact: NonNull<CNContact>, stop: NonNull<Bool>| {
                    let contact_ref = unsafe { contact.as_ref() };
                    let vec = unsafe { &mut *contacts_ptr };
                    vec.push(cncontact_to_contact(contact_ref, connector_id_ref));
                    if vec.len() >= target_count {
                        unsafe { *stop.as_ptr() = Bool::YES };
                    }
                });

            let mut err: Option<Retained<NSError>> = None;
            let ok = unsafe {
                store.enumerateContactsWithFetchRequest_error_usingBlock(
                    &request,
                    Some(&mut err),
                    &block,
                )
            };

            if !ok {
                if let Some(e) = err {
                    return Err(bridge::retained_nserror_to_anyhow(&e));
                }
                return Err(anyhow!("Failed to enumerate contacts"));
            }

            Ok(all_contacts.into_iter().skip(offset).take(limit).collect())
        })
        .await
        .context("Contacts list_contacts task panicked")?
    }

    async fn search_contacts(&self, query: &str, limit: usize) -> Result<Vec<Contact>> {
        let store = self.store.clone();
        let lock = self.lock.clone();
        let query_str = query.to_string();
        let connector_id = self.connector_id.clone();

        tokio::task::spawn_blocking(move || {
            let _guard = lock.lock().map_err(|e| anyhow!("lock poisoned: {e}"))?;
            request_contacts_access(&store)?;

            let keys = fetch_keys();
            let ns_query = bridge::string_to_nsstring(&query_str);
            let predicate = unsafe { CNContact::predicateForContactsMatchingName(&ns_query) };

            let contacts = unsafe {
                store.unifiedContactsMatchingPredicate_keysToFetch_error(&predicate, &keys)
            }
            .map_err(|e| bridge::retained_nserror_to_anyhow(&e))?;

            let mut result = Vec::new();
            for contact in contacts.to_vec().into_iter().take(limit) {
                result.push(cncontact_to_contact(&contact, &connector_id));
            }
            Ok(result)
        })
        .await
        .context("Contacts search_contacts task panicked")?
    }

    async fn get_contact(&self, contact_id: &str) -> Result<Contact> {
        let store = self.store.clone();
        let lock = self.lock.clone();
        let cid = contact_id.to_string();
        let connector_id = self.connector_id.clone();

        tokio::task::spawn_blocking(move || {
            let _guard = lock.lock().map_err(|e| anyhow!("lock poisoned: {e}"))?;
            request_contacts_access(&store)?;

            let keys = fetch_keys();
            let ns_id = bridge::string_to_nsstring(&cid);

            let contact =
                unsafe { store.unifiedContactWithIdentifier_keysToFetch_error(&ns_id, &keys) }
                    .map_err(|e| bridge::retained_nserror_to_anyhow(&e))?;

            Ok(cncontact_to_contact(&contact, &connector_id))
        })
        .await
        .context("Contacts get_contact task panicked")?
    }
}
