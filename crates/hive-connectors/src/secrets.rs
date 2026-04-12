//! OS keyring storage for connector secrets.
//!
//! Delegates to `hive_core::secret_store`, which manages per-key entries
//! in the OS keyring.  Each connector field is stored under the
//! key `"connector:{id}:{field}"`.

use hive_core::secret_store;

/// All known secret field names for connectors.
const ALL_FIELDS: &[&str] = &[
    "password",
    "client_secret",
    "access_token",
    "refresh_token",
    "bot_token",
    "app_token",
    "client_id_override",
    "custom_client_secret",
    "oauth_error",
];

/// Global-store key for a connector secret field.
fn store_key(connector_id: &str, field: &str) -> String {
    format!("connector:{connector_id}:{field}")
}

/// Store a secret in the OS keyring.
pub fn save(connector_id: &str, field: &str, value: &str) {
    secret_store::save(&store_key(connector_id, field), value);
}

/// Load a secret from the OS keyring.  Returns `None` if not found.
pub fn load(connector_id: &str, field: &str) -> Option<String> {
    secret_store::load(&store_key(connector_id, field))
}

/// Delete a single secret from the OS keyring.
pub fn delete(connector_id: &str, field: &str) {
    secret_store::delete(&store_key(connector_id, field));
}

/// Delete all known secrets for a connector.
pub fn delete_all(connector_id: &str) {
    for field in ALL_FIELDS {
        secret_store::delete(&store_key(connector_id, field));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_key_format() {
        assert_eq!(store_key("work-email", "password"), "connector:work-email:password");
    }
}
