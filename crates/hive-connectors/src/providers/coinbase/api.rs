//! HTTP client for the Coinbase Advanced Trade API (v3) and legacy v2
//! transaction endpoints.
//!
//! Authenticates via **CDP API Key**: per-request ES256-signed JWT.

use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Base URLs
// ---------------------------------------------------------------------------

const PRODUCTION_BASE_URL: &str = "https://api.coinbase.com";
const SANDBOX_BASE_URL: &str = "https://api-sandbox.coinbase.com";

// ---------------------------------------------------------------------------
// API response types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub uuid: String,
    pub name: String,
    pub currency: String,
    #[serde(default)]
    pub available_balance: Balance,
    #[serde(default)]
    pub hold: Balance,
    #[serde(default)]
    pub r#type: String,
    #[serde(default)]
    pub active: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Balance {
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub currency: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountsResponse {
    pub accounts: Vec<Account>,
    #[serde(default)]
    pub has_next: bool,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Product {
    pub product_id: String,
    pub price: String,
    #[serde(default)]
    pub base_currency_id: String,
    #[serde(default)]
    pub quote_currency_id: String,
    #[serde(default)]
    pub base_display_symbol: String,
    #[serde(default)]
    pub quote_display_symbol: String,
    #[serde(default)]
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProductsResponse {
    pub products: Vec<Product>,
    #[serde(default)]
    pub num_products: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderResponse {
    pub success: bool,
    #[serde(default)]
    pub order_id: String,
    #[serde(default)]
    pub failure_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    pub order_id: String,
    pub product_id: String,
    pub side: String,
    pub status: String,
    #[serde(default)]
    pub created_time: String,
    #[serde(default)]
    pub completion_percentage: String,
    #[serde(default)]
    pub filled_size: Option<String>,
    #[serde(default)]
    pub average_filled_price: Option<String>,
    #[serde(default)]
    pub total_value_after_fees: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrdersResponse {
    pub orders: Vec<Order>,
    #[serde(default)]
    pub has_next: bool,
    #[serde(default)]
    pub cursor: Option<String>,
}

// -- V2 transaction types ---------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    pub id: String,
    pub r#type: String,
    pub status: String,
    #[serde(default)]
    pub amount: MoneyHash,
    #[serde(default)]
    pub native_amount: MoneyHash,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MoneyHash {
    #[serde(default)]
    pub amount: String,
    #[serde(default)]
    pub currency: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionsWrapper {
    pub data: Vec<Transaction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMoneyResponse {
    pub data: Transaction,
}

// ---------------------------------------------------------------------------
// CoinbaseClient
// ---------------------------------------------------------------------------

/// Normalize a PEM key that may have been pasted with literal `\n` escape
/// sequences, `\r\n` line endings, or missing final newline.
/// Also converts SEC1 (`BEGIN EC PRIVATE KEY`) to PKCS#8 (`BEGIN PRIVATE KEY`)
/// because `jsonwebtoken` v9 only supports PKCS#8 for EC keys.
fn normalize_pem(pem: &str) -> Result<String> {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;

    // Replace literal two-char `\n` sequences with actual newlines
    let s = pem.replace("\\n", "\n");
    // Normalize \r\n → \n
    let s = s.replace("\r\n", "\n");
    // Trim outer whitespace
    let s = s.trim().to_string();

    // If the key is already PKCS#8, return as-is (with trailing newline)
    if s.contains("BEGIN PRIVATE KEY") {
        return Ok(if s.ends_with('\n') { s } else { format!("{s}\n") });
    }

    // Convert SEC1 (BEGIN EC PRIVATE KEY) → PKCS#8 (BEGIN PRIVATE KEY).
    // jsonwebtoken v9's from_ec_pem only accepts PKCS#8.
    if s.contains("BEGIN EC PRIVATE KEY") {
        let b64: String =
            s.lines().filter(|l| !l.starts_with("-----")).collect::<Vec<_>>().join("");
        let sec1_der =
            STANDARD.decode(&b64).context("failed to base64-decode EC PRIVATE KEY body")?;

        // Parse the SEC1 ECPrivateKey to strip the optional [0] parameters
        // (curve OID), since those move into the PKCS#8 AlgorithmIdentifier.
        //
        // SEC1 ECPrivateKey ::= SEQUENCE {
        //   version      INTEGER (1),
        //   privateKey   OCTET STRING,
        //   parameters   [0] EXPLICIT ECParameters OPTIONAL,  ← strip
        //   publicKey    [1] EXPLICIT BIT STRING OPTIONAL      ← keep
        // }
        //
        // We rebuild an inner ECPrivateKey without the [0] parameters.
        anyhow::ensure!(
            sec1_der.len() >= 2 && sec1_der[0] == 0x30,
            "SEC1 DER does not start with SEQUENCE"
        );

        // Skip outer SEQUENCE tag + length to get to the contents
        let (_, outer_contents) =
            skip_der_tag_length(&sec1_der).context("failed to parse SEC1 outer SEQUENCE")?;

        // Walk through the SEC1 contents, collecting everything except [0]
        let mut inner_parts: Vec<u8> = Vec::new();
        let mut pos = 0;
        while pos < outer_contents.len() {
            let tag = outer_contents[pos];
            let (elem_total, _) = skip_der_tag_length(&outer_contents[pos..])
                .context("failed to parse SEC1 element")?;
            if tag == 0xa0 {
                // [0] EXPLICIT parameters — skip (curve OID is in PKCS#8 AlgorithmIdentifier)
            } else {
                inner_parts.extend_from_slice(&outer_contents[pos..pos + elem_total]);
            }
            pos += elem_total;
        }

        // Wrap the cleaned ECPrivateKey in a new SEQUENCE
        let inner_seq = wrap_der_sequence(&inner_parts);

        // Build PKCS#8:
        //   SEQUENCE {
        //     INTEGER 0  (version)
        //     SEQUENCE { OID id-ecPublicKey, OID secp256r1 }  (AlgorithmIdentifier)
        //     OCTET STRING { inner_seq }
        //   }
        let alg_id: &[u8] = &[
            0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02,
            0x01, // OID id-ecPublicKey
            0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, // OID secp256r1
        ];
        let octet_string = wrap_der_octet_string(&inner_seq);

        let mut body = Vec::new();
        body.extend_from_slice(&[0x02, 0x01, 0x00]); // INTEGER 0
        body.extend_from_slice(alg_id);
        body.extend_from_slice(&octet_string);

        let pkcs8 = wrap_der_sequence(&body);

        let b64_out = STANDARD.encode(&pkcs8);
        let mut pem_out = String::from("-----BEGIN PRIVATE KEY-----\n");
        for chunk in b64_out.as_bytes().chunks(64) {
            pem_out.push_str(std::str::from_utf8(chunk).unwrap());
            pem_out.push('\n');
        }
        pem_out.push_str("-----END PRIVATE KEY-----\n");
        return Ok(pem_out);
    }

    // Unknown format — return as-is and let from_ec_pem report the error
    Ok(if s.ends_with('\n') { s } else { format!("{s}\n") })
}

/// Parse a DER tag + length and return (total_element_bytes, contents_slice).
fn skip_der_tag_length(data: &[u8]) -> Option<(usize, &[u8])> {
    if data.is_empty() {
        return None;
    }
    let _tag = data[0];
    if data.len() < 2 {
        return None;
    }
    let (content_len, header_len) = if data[1] < 0x80 {
        (data[1] as usize, 2)
    } else if data[1] == 0x81 {
        if data.len() < 3 {
            return None;
        }
        (data[2] as usize, 3)
    } else if data[1] == 0x82 {
        if data.len() < 4 {
            return None;
        }
        (((data[2] as usize) << 8) | (data[3] as usize), 4)
    } else {
        return None;
    };
    let total = header_len + content_len;
    if data.len() < total {
        return None;
    }
    Some((total, &data[header_len..total]))
}

/// Encode a DER length.
fn encode_der_length(len: usize) -> Vec<u8> {
    if len < 0x80 {
        vec![len as u8]
    } else if len < 0x100 {
        vec![0x81, len as u8]
    } else {
        vec![0x82, (len >> 8) as u8, (len & 0xff) as u8]
    }
}

/// Wrap content bytes in a DER SEQUENCE (tag 0x30).
fn wrap_der_sequence(content: &[u8]) -> Vec<u8> {
    let mut out = vec![0x30];
    out.extend_from_slice(&encode_der_length(content.len()));
    out.extend_from_slice(content);
    out
}

/// Wrap content bytes in a DER OCTET STRING (tag 0x04).
fn wrap_der_octet_string(content: &[u8]) -> Vec<u8> {
    let mut out = vec![0x04];
    out.extend_from_slice(&encode_der_length(content.len()));
    out.extend_from_slice(content);
    out
}

pub struct CoinbaseClient {
    http: Client,
    base_url: String,
    host: String,
    connector_id: String,
    key_name: String,
    encoding_key: jsonwebtoken::EncodingKey,
}

impl CoinbaseClient {
    /// Create a client using a **CDP API Key** (ES256 JWT signing).
    pub fn new(
        connector_id: &str,
        key_name: &str,
        private_key_pem: &str,
        sandbox: bool,
    ) -> Result<Self> {
        let normalized_pem = normalize_pem(private_key_pem)?;
        let encoding_key = jsonwebtoken::EncodingKey::from_ec_pem(normalized_pem.as_bytes())
            .context("invalid EC private key PEM for Coinbase CDP API key")?;
        let base_url =
            if sandbox { SANDBOX_BASE_URL.to_string() } else { PRODUCTION_BASE_URL.to_string() };
        let host = if sandbox { "api-sandbox.coinbase.com" } else { "api.coinbase.com" };
        Ok(Self {
            http: Client::new(),
            base_url,
            host: host.to_string(),
            connector_id: connector_id.to_string(),
            key_name: key_name.to_string(),
            encoding_key,
        })
    }

    /// The connector ID this client belongs to.
    pub fn connector_id(&self) -> &str {
        &self.connector_id
    }

    // -- CDP API Key JWT generation -----------------------------------------

    fn build_cdp_jwt(&self, method: &str, path: &str) -> Result<String> {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

        let now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();

        // Matches the official Coinbase Advanced Trade Python SDK exactly:
        //   sub  = API key name
        //   iss  = "cdp" (literal)
        //   uri  = "METHOD host/path"
        //   nonce = random hex in JWT header
        let uri = format!("{} {}{}", method.to_uppercase(), self.host, path);

        let nonce = uuid::Uuid::new_v4().to_string().replace('-', "");

        let header = serde_json::json!({
            "alg": "ES256",
            "typ": "JWT",
            "kid": self.key_name,
            "nonce": nonce,
        });
        let claims = serde_json::json!({
            "sub": self.key_name,
            "iss": "cdp",
            "nbf": now,
            "exp": now + 120,
            "uri": uri,
        });

        let h = URL_SAFE_NO_PAD.encode(header.to_string().as_bytes());
        let c = URL_SAFE_NO_PAD.encode(claims.to_string().as_bytes());
        let message = format!("{}.{}", h, c);

        let sig = self.encoding_key_sign(&message).context("failed to sign Coinbase CDP JWT")?;

        Ok(format!("{}.{}", message, sig))
    }

    /// Sign a JWT message string with the ES256 key and return the
    /// base64url-encoded signature.
    fn encoding_key_sign(&self, message: &str) -> Result<String> {
        let sig = jsonwebtoken::crypto::sign(
            message.as_bytes(),
            &self.encoding_key,
            jsonwebtoken::Algorithm::ES256,
        )?;
        Ok(sig)
    }

    // -- Authenticated request helpers --------------------------------------

    /// Execute a GET request with CDP API Key auth.
    async fn get(&self, path: &str) -> Result<reqwest::Response> {
        let url = format!("{}{}", self.base_url, path);
        let token = self.build_cdp_jwt("GET", path)?;

        self.http.get(&url).bearer_auth(&token).send().await.context("Coinbase API GET")
    }

    /// Execute a POST request with CDP API Key auth.
    async fn post(&self, path: &str, body: &impl Serialize) -> Result<reqwest::Response> {
        let url = format!("{}{}", self.base_url, path);
        let token = self.build_cdp_jwt("POST", path)?;

        self.http
            .post(&url)
            .bearer_auth(&token)
            .json(body)
            .send()
            .await
            .context("Coinbase API POST")
    }

    // -- Advanced Trade API v3 endpoints ------------------------------------

    /// List all accounts.
    pub async fn list_accounts(&self) -> Result<AccountsResponse> {
        let resp = self.get("/api/v3/brokerage/accounts").await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            bail!("list_accounts failed ({}): {}", status, body);
        }
        resp.json().await.context("parsing accounts response")
    }

    /// Get a single account by UUID.
    pub async fn get_account(&self, account_id: &str) -> Result<Account> {
        let resp = self.get(&format!("/api/v3/brokerage/accounts/{account_id}")).await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            bail!("get_account failed ({}): {}", status, body);
        }
        #[derive(Deserialize)]
        struct Wrapper {
            account: Account,
        }
        let w: Wrapper = resp.json().await.context("parsing account response")?;
        Ok(w.account)
    }

    /// List available trading products (with prices).
    pub async fn list_products(&self) -> Result<ProductsResponse> {
        let resp = self.get("/api/v3/brokerage/products").await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            bail!("list_products failed ({}): {}", status, body);
        }
        resp.json().await.context("parsing products response")
    }

    /// Get a single product by ID (e.g. "BTC-USD").
    pub async fn get_product(&self, product_id: &str) -> Result<Product> {
        let resp = self.get(&format!("/api/v3/brokerage/products/{product_id}")).await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            bail!("get_product failed ({}): {}", status, body);
        }
        resp.json().await.context("parsing product response")
    }

    /// Create a market order (buy or sell).
    pub async fn create_order(
        &self,
        product_id: &str,
        side: &str,
        quote_size: Option<&str>,
        base_size: Option<&str>,
    ) -> Result<OrderResponse> {
        let client_order_id = uuid::Uuid::new_v4().to_string();
        let mut order_config = serde_json::json!({});
        if let Some(qs) = quote_size {
            order_config = serde_json::json!({ "market_market_ioc": { "quote_size": qs } });
        } else if let Some(bs) = base_size {
            order_config = serde_json::json!({ "market_market_ioc": { "base_size": bs } });
        }
        let body = serde_json::json!({
            "client_order_id": client_order_id,
            "product_id": product_id,
            "side": side,
            "order_configuration": order_config,
        });
        let resp = self.post("/api/v3/brokerage/orders", &body).await?;
        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            bail!("create_order failed ({}): {}", status, body_text);
        }
        resp.json().await.context("parsing order response")
    }

    /// List historical orders.
    pub async fn list_orders(&self) -> Result<OrdersResponse> {
        let resp = self.get("/api/v3/brokerage/orders/historical/batch").await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            bail!("list_orders failed ({}): {}", status, body);
        }
        resp.json().await.context("parsing orders response")
    }

    // -- Legacy v2 transaction endpoints ------------------------------------

    /// List transactions for an account.
    pub async fn list_transactions(&self, account_id: &str) -> Result<Vec<Transaction>> {
        let resp = self.get(&format!("/v2/accounts/{account_id}/transactions")).await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            bail!("list_transactions failed ({}): {}", status, body);
        }
        let wrapper: TransactionsWrapper =
            resp.json().await.context("parsing transactions response")?;
        Ok(wrapper.data)
    }

    /// Send cryptocurrency from an account.
    pub async fn send_money(
        &self,
        account_id: &str,
        to: &str,
        amount: &str,
        currency: &str,
    ) -> Result<Transaction> {
        let body = serde_json::json!({
            "type": "send",
            "to": to,
            "amount": amount,
            "currency": currency,
        });
        let resp = self.post(&format!("/v2/accounts/{account_id}/transactions"), &body).await?;
        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            bail!("send_money failed ({}): {}", status, body_text);
        }
        let wrapper: SendMoneyResponse =
            resp.json().await.context("parsing send_money response")?;
        Ok(wrapper.data)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_deserializes() {
        let json = serde_json::json!({
            "uuid": "abc-123",
            "name": "BTC Wallet",
            "currency": "BTC",
            "available_balance": { "value": "1.5", "currency": "BTC" },
            "hold": { "value": "0.0", "currency": "BTC" },
            "type": "ACCOUNT_TYPE_CRYPTO",
            "active": true
        });
        let acct: Account = serde_json::from_value(json).unwrap();
        assert_eq!(acct.uuid, "abc-123");
        assert_eq!(acct.available_balance.value, "1.5");
        assert!(acct.active);
    }

    #[test]
    fn product_deserializes() {
        let json = serde_json::json!({
            "product_id": "BTC-USD",
            "price": "67000.00",
            "base_currency_id": "BTC",
            "quote_currency_id": "USD",
            "base_display_symbol": "BTC",
            "quote_display_symbol": "USD",
            "status": "ONLINE"
        });
        let p: Product = serde_json::from_value(json).unwrap();
        assert_eq!(p.product_id, "BTC-USD");
        assert_eq!(p.price, "67000.00");
    }

    #[test]
    fn order_response_deserializes() {
        let json = serde_json::json!({
            "success": true,
            "order_id": "order-xyz",
            "failure_reason": null
        });
        let r: OrderResponse = serde_json::from_value(json).unwrap();
        assert!(r.success);
        assert_eq!(r.order_id, "order-xyz");
    }

    #[test]
    fn transaction_deserializes() {
        let json = serde_json::json!({
            "id": "tx-1",
            "type": "send",
            "status": "completed",
            "amount": { "amount": "-0.01", "currency": "BTC" },
            "native_amount": { "amount": "-500.00", "currency": "USD" },
            "description": "test send",
            "created_at": "2025-01-01T00:00:00Z",
            "updated_at": "2025-01-01T00:00:00Z"
        });
        let tx: Transaction = serde_json::from_value(json).unwrap();
        assert_eq!(tx.id, "tx-1");
        assert_eq!(tx.amount.amount, "-0.01");
    }

    /// A test EC P-256 private key in PKCS#8 format (NOT a real key — for unit tests only).
    const TEST_EC_PEM: &str = "-----BEGIN PRIVATE KEY-----\n\
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgl0V43geGdE1aUifF\n\
Yl9SkTFxl51Pzhxf1ceo4TTX4x2hRANCAAS0d4TxS/dVRfp8uugFXbSD2oFKKxdz\n\
WFqar8wj03nITtVkHqWT5oLXHtnpcrCFMnCUrr7BH7gJwUpeGedSgKV/\n\
-----END PRIVATE KEY-----";

    #[test]
    fn sandbox_base_url() {
        let client = CoinbaseClient::new("test-cb", "orgs/1/apiKeys/2", TEST_EC_PEM, true).unwrap();
        assert_eq!(client.base_url, SANDBOX_BASE_URL);
        assert_eq!(client.host, "api-sandbox.coinbase.com");
    }

    #[test]
    fn production_base_url() {
        let client =
            CoinbaseClient::new("test-cb", "orgs/1/apiKeys/2", TEST_EC_PEM, false).unwrap();
        assert_eq!(client.base_url, PRODUCTION_BASE_URL);
        assert_eq!(client.host, "api.coinbase.com");
    }

    #[test]
    fn api_key_jwt_has_expected_claims() {
        let client = CoinbaseClient::new(
            "test-cb",
            "organizations/org1/apiKeys/key-uuid-123",
            TEST_EC_PEM,
            false,
        )
        .unwrap();
        let jwt = client.build_cdp_jwt("GET", "/api/v3/brokerage/accounts").unwrap();

        // Decode without validation (we don't have the public key handy)
        let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::ES256);
        validation.insecure_disable_signature_validation();
        validation.set_required_spec_claims::<&str>(&[]);
        validation.validate_aud = false;

        let data = jsonwebtoken::decode::<serde_json::Value>(
            &jwt,
            &jsonwebtoken::DecodingKey::from_secret(b"unused"),
            &validation,
        )
        .unwrap();

        // sub = API key name, iss = "cdp", uri = "METHOD host/path"
        assert_eq!(data.claims["sub"], "organizations/org1/apiKeys/key-uuid-123");
        assert_eq!(data.claims["iss"], "cdp");
        assert!(data.claims.get("aud").is_none(), "no aud claim expected");
        assert_eq!(data.claims["uri"], "GET api.coinbase.com/api/v3/brokerage/accounts");
        assert!(data.claims["nbf"].as_u64().is_some());
        assert!(data.claims["exp"].as_u64().unwrap() > data.claims["nbf"].as_u64().unwrap());
        assert_eq!(data.header.kid.as_deref(), Some("organizations/org1/apiKeys/key-uuid-123"));
    }

    #[test]
    fn invalid_pem_returns_error() {
        let result = CoinbaseClient::new("test-cb", "key", "not-a-pem", false);
        assert!(result.is_err());
    }

    #[test]
    fn normalize_pem_handles_escaped_newlines() {
        // Simulate a PEM pasted with literal \n characters
        let escaped = TEST_EC_PEM.replace('\n', "\\n");
        let client = CoinbaseClient::new("test-cb", "orgs/1/apiKeys/2", &escaped, false);
        assert!(client.is_ok(), "escaped \\n PEM should parse after normalization");
    }

    #[test]
    fn normalize_pem_handles_crlf() {
        let crlf = TEST_EC_PEM.replace('\n', "\r\n");
        let client = CoinbaseClient::new("test-cb", "orgs/1/apiKeys/2", &crlf, false);
        assert!(client.is_ok(), "CRLF PEM should parse after normalization");
    }

    /// SEC1 format key (`BEGIN EC PRIVATE KEY`) as provided by Coinbase CDP.
    const TEST_SEC1_PEM: &str = "-----BEGIN EC PRIVATE KEY-----\n\
MHcCAQEEIPpqjjRuv+xkPgouyjxDFd22Gp5E05jmxwU4M2q5ZVDNoAoGCCqGSM49\n\
AwEHoUQDQgAEYeiOYtO5tCzzIj2fLelBj1leLoDKiTsfD5WRY7K9wp5pVfEwHMDG\n\
DrkNyxX60wvpTzaiJA3RpQQo0gu+vXTmUw==\n\
-----END EC PRIVATE KEY-----";

    #[test]
    fn sec1_pem_is_auto_converted_to_pkcs8() {
        let client = CoinbaseClient::new("test-cb", "orgs/1/apiKeys/2", TEST_SEC1_PEM, false);
        assert!(client.is_ok(), "SEC1 PEM should be auto-converted to PKCS#8: {:?}", client.err());
    }

    #[test]
    fn sec1_pem_with_escaped_newlines() {
        let escaped = TEST_SEC1_PEM.replace('\n', "\\n");
        let client = CoinbaseClient::new("test-cb", "orgs/1/apiKeys/2", &escaped, false);
        assert!(client.is_ok(), "SEC1 PEM with escaped newlines should work: {:?}", client.err());
    }

    #[test]
    fn sec1_jwt_produces_valid_token() {
        let client =
            CoinbaseClient::new("test-cb", "orgs/1/apiKeys/2", TEST_SEC1_PEM, false).unwrap();
        let jwt = client.build_cdp_jwt("GET", "/api/v3/brokerage/accounts");
        assert!(jwt.is_ok(), "JWT signing with converted SEC1 key should succeed");
    }

    #[test]
    fn sec1_to_pkcs8_matches_openssl() {
        use base64::engine::general_purpose::STANDARD;
        use base64::Engine;

        // This is the PKCS#8 output from:
        //   openssl pkcs8 -topk8 -nocrypt -in <TEST_SEC1_PEM>
        let expected_pkcs8_pem = "-----BEGIN PRIVATE KEY-----\n\
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQg+mqONG6/7GQ+Ci7K\n\
PEMV3bYankTTmObHBTgzarllUM2hRANCAARh6I5i07m0LPMiPZ8t6UGPWV4ugMqJ\n\
Ox8PlZFjsr3CnmlV8TAcwMYOuQ3LFfrTC+lPNqIkDdGlBCjSC769dOZT\n\
-----END PRIVATE KEY-----\n";

        let our_pkcs8 = normalize_pem(TEST_SEC1_PEM).unwrap();

        // Compare DER bytes (ignore PEM line-wrapping differences)
        let extract_der = |pem: &str| -> Vec<u8> {
            let b64: String =
                pem.lines().filter(|l| !l.starts_with("-----")).collect::<Vec<_>>().join("");
            STANDARD.decode(&b64).unwrap()
        };

        let expected_der = extract_der(expected_pkcs8_pem);
        let our_der = extract_der(&our_pkcs8);

        assert_eq!(
            our_der,
            expected_der,
            "Our PKCS#8 DER should match OpenSSL's output.\n  ours:   {}\n  expect: {}",
            our_der.iter().map(|b| format!("{:02x}", b)).collect::<String>(),
            expected_der.iter().map(|b| format!("{:02x}", b)).collect::<String>(),
        );
    }
}
