//! `DynService` implementation for Coinbase trading operations.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use hive_classification::ChannelClass;
use hive_contracts::ToolApproval;

use crate::service_registry::{DynService, OperationSchema, ServiceDescriptor};

use super::api::CoinbaseClient;

// ---------------------------------------------------------------------------
// CoinbaseTradingService
// ---------------------------------------------------------------------------

pub struct CoinbaseTradingService {
    client: Arc<CoinbaseClient>,
}

impl CoinbaseTradingService {
    pub fn new(client: Arc<CoinbaseClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl DynService for CoinbaseTradingService {
    fn descriptor(&self) -> ServiceDescriptor {
        ServiceDescriptor {
            service_type: "trading".into(),
            display_name: "Coinbase Trading".into(),
            description: "Crypto trading via Coinbase — view balances, prices, buy/sell, \
                          send crypto, and transaction history."
                .into(),
            is_standard: false,
        }
    }

    fn operations(&self) -> Vec<OperationSchema> {
        vec![
            OperationSchema {
                name: "list_accounts".into(),
                description: "List all Coinbase accounts with balances.".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }),
                output_schema: None,
                side_effects: false,
                approval: ToolApproval::Auto,
                channel_class: ChannelClass::Internal,
            },
            OperationSchema {
                name: "get_account".into(),
                description: "Get details for a specific Coinbase account by UUID.".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "account_id": {
                            "type": "string",
                            "description": "The UUID of the account to retrieve."
                        }
                    },
                    "required": ["account_id"],
                    "additionalProperties": false
                }),
                output_schema: None,
                side_effects: false,
                approval: ToolApproval::Auto,
                channel_class: ChannelClass::Internal,
            },
            OperationSchema {
                name: "get_price".into(),
                description: "Get the current price for a trading pair (e.g. BTC-USD).".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "product_id": {
                            "type": "string",
                            "description": "Trading pair identifier, e.g. 'BTC-USD'."
                        }
                    },
                    "required": ["product_id"],
                    "additionalProperties": false
                }),
                output_schema: None,
                side_effects: false,
                approval: ToolApproval::Auto,
                channel_class: ChannelClass::Public,
            },
            OperationSchema {
                name: "list_transactions".into(),
                description: "List transactions for a specific account.".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "account_id": {
                            "type": "string",
                            "description": "The UUID of the account."
                        }
                    },
                    "required": ["account_id"],
                    "additionalProperties": false
                }),
                output_schema: None,
                side_effects: false,
                approval: ToolApproval::Auto,
                channel_class: ChannelClass::Internal,
            },
            OperationSchema {
                name: "create_order".into(),
                description: "Place a market order to buy or sell cryptocurrency. \
                              Specify either quote_size (amount in quote currency, e.g. USD) \
                              or base_size (amount in base currency, e.g. BTC)."
                    .into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "product_id": {
                            "type": "string",
                            "description": "Trading pair, e.g. 'BTC-USD'."
                        },
                        "side": {
                            "type": "string",
                            "enum": ["BUY", "SELL"],
                            "description": "Order side."
                        },
                        "quote_size": {
                            "type": "string",
                            "description": "Amount in quote currency (e.g. '100' for $100). Mutually exclusive with base_size."
                        },
                        "base_size": {
                            "type": "string",
                            "description": "Amount in base currency (e.g. '0.01' for 0.01 BTC). Mutually exclusive with quote_size."
                        }
                    },
                    "required": ["product_id", "side"],
                    "additionalProperties": false
                }),
                output_schema: None,
                side_effects: true,
                approval: ToolApproval::Ask,
                channel_class: ChannelClass::Internal,
            },
            OperationSchema {
                name: "send_crypto".into(),
                description: "Send cryptocurrency from an account to an external address or \
                              email."
                    .into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "account_id": {
                            "type": "string",
                            "description": "The UUID of the source account."
                        },
                        "to": {
                            "type": "string",
                            "description": "Destination address or email."
                        },
                        "amount": {
                            "type": "string",
                            "description": "Amount to send."
                        },
                        "currency": {
                            "type": "string",
                            "description": "Currency code, e.g. 'BTC'."
                        }
                    },
                    "required": ["account_id", "to", "amount", "currency"],
                    "additionalProperties": false
                }),
                output_schema: None,
                side_effects: true,
                approval: ToolApproval::Ask,
                channel_class: ChannelClass::Internal,
            },
            OperationSchema {
                name: "list_orders".into(),
                description: "List historical orders.".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }),
                output_schema: None,
                side_effects: false,
                approval: ToolApproval::Auto,
                channel_class: ChannelClass::Internal,
            },
        ]
    }

    async fn execute(&self, operation: &str, input: Value) -> anyhow::Result<Value> {
        match operation {
            "list_accounts" => {
                let resp = self.client.list_accounts().await?;
                Ok(serde_json::to_value(resp.accounts)?)
            }
            "get_account" => {
                let account_id = input["account_id"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("missing required field: account_id"))?;
                let acct = self.client.get_account(account_id).await?;
                Ok(serde_json::to_value(acct)?)
            }
            "get_price" => {
                let product_id = input["product_id"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("missing required field: product_id"))?;
                let product = self.client.get_product(product_id).await?;
                Ok(serde_json::to_value(product)?)
            }
            "list_transactions" => {
                let account_id = input["account_id"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("missing required field: account_id"))?;
                let txns = self.client.list_transactions(account_id).await?;
                Ok(serde_json::to_value(txns)?)
            }
            "create_order" => {
                let product_id = input["product_id"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("missing required field: product_id"))?;
                let side = input["side"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("missing required field: side"))?;
                let quote_size = input["quote_size"].as_str();
                let base_size = input["base_size"].as_str();
                let resp =
                    self.client.create_order(product_id, side, quote_size, base_size).await?;
                Ok(serde_json::to_value(resp)?)
            }
            "send_crypto" => {
                let account_id = input["account_id"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("missing required field: account_id"))?;
                let to = input["to"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("missing required field: to"))?;
                let amount = input["amount"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("missing required field: amount"))?;
                let currency = input["currency"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("missing required field: currency"))?;
                let tx = self.client.send_money(account_id, to, amount, currency).await?;
                Ok(serde_json::to_value(tx)?)
            }
            "list_orders" => {
                let resp = self.client.list_orders().await?;
                Ok(serde_json::to_value(resp.orders)?)
            }
            _ => anyhow::bail!("unknown Coinbase trading operation: {operation}"),
        }
    }

    async fn test_connection(&self) -> anyhow::Result<()> {
        self.client.list_accounts().await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A test EC P-256 private key in PKCS#8 format (for unit tests only).
    const TEST_EC_PEM: &str = "-----BEGIN PRIVATE KEY-----\n\
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgl0V43geGdE1aUifF\n\
Yl9SkTFxl51Pzhxf1ceo4TTX4x2hRANCAAS0d4TxS/dVRfp8uugFXbSD2oFKKxdz\n\
WFqar8wj03nITtVkHqWT5oLXHtnpcrCFMnCUrr7BH7gJwUpeGedSgKV/\n\
-----END PRIVATE KEY-----";

    fn test_client() -> Arc<CoinbaseClient> {
        Arc::new(CoinbaseClient::new("test", "orgs/1/apiKeys/2", TEST_EC_PEM, true).unwrap())
    }

    #[test]
    fn descriptor_is_non_standard() {
        let svc = CoinbaseTradingService::new(test_client());
        let desc = svc.descriptor();
        assert_eq!(desc.service_type, "trading");
        assert!(!desc.is_standard);
    }

    #[test]
    fn operations_schema_count() {
        let svc = CoinbaseTradingService::new(test_client());
        let ops = svc.operations();
        assert_eq!(ops.len(), 7);
    }

    #[test]
    fn mutating_operations_require_approval() {
        let svc = CoinbaseTradingService::new(test_client());
        let ops = svc.operations();
        for op in &ops {
            if op.side_effects {
                assert_eq!(
                    op.approval,
                    ToolApproval::Ask,
                    "operation '{}' has side effects but does not require approval",
                    op.name
                );
            }
        }
    }

    #[test]
    fn read_only_operations_are_auto() {
        let svc = CoinbaseTradingService::new(test_client());
        let ops = svc.operations();
        for op in &ops {
            if !op.side_effects {
                assert_eq!(
                    op.approval,
                    ToolApproval::Auto,
                    "read-only operation '{}' should be auto-approved",
                    op.name
                );
            }
        }
    }

    #[tokio::test]
    async fn unknown_operation_returns_error() {
        let svc = CoinbaseTradingService::new(test_client());
        let result = svc.execute("nonexistent", json!({})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown"));
    }
}
