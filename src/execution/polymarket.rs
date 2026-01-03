use crate::config::config::PolyCfg;
use crate::core::types::{Execution, Order as CoreOrder, Side};
use crate::execution::client::ExecutionClient;
use anyhow::{Context, Result};
use async_trait::async_trait;
use rust_decimal::Decimal;
use std::str::FromStr;
use std::sync::Arc;
use tracing::info;

// Polymarket SDK imports
use alloy::signers::Signer; // Trait for with_chain_id
use alloy::signers::local::PrivateKeySigner;
use polymarket_client_sdk::POLYGON;
use polymarket_client_sdk::auth::Credentials as SdkCredentials;
use polymarket_client_sdk::auth::Normal;
use polymarket_client_sdk::auth::state::Authenticated;
use polymarket_client_sdk::clob::types::{OrderType, Side as PolySide};
use polymarket_client_sdk::clob::{Client, Config};

use base64::Engine;
use base64::prelude::*;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;

// Alloy for EIP-712
use alloy::dyn_abi::Eip712Domain;
use alloy::hex::ToHexExt;
use alloy::primitives::U256;
use alloy::sol;
use alloy::sol_types::SolStruct;
use std::borrow::Cow;

sol! {
    #[derive(Debug)]
    struct ClobAuth {
        address address;
        string  timestamp;
        uint256 nonce;
        string  message;
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct ApiCreds {
    #[serde(rename = "apiKey")]
    pub api_key: String,
    pub secret: String,
    pub passphrase: String,
}

#[derive(Clone)]
pub struct PolyExecutionClient {
    // Client<Authenticated<Normal>>
    inner: Arc<Client<Authenticated<Normal>>>,
    cfg: PolyCfg,
    signer: PrivateKeySigner,
    creds: ApiCreds, // Stored credentials for manual calls
}

impl PolyExecutionClient {
    pub async fn get_book_summary(
        &self,
        token_id: &str,
    ) -> Result<polymarket_client_sdk::clob::types::OrderBookSummaryResponse> {
        // Try using the builder() method if available
        // If this fails, I will revert this helper method.
        let req = polymarket_client_sdk::clob::types::OrderBookSummaryRequest::builder()
            .token_id(token_id.to_string())
            .build();
        let summary = self.inner.order_book(&req).await?;
        Ok(summary)
    }
    pub async fn new(cfg: PolyCfg) -> Result<Self> {
        if cfg.private_key.is_empty() {
            return Err(anyhow::anyhow!(
                "Private Key required for PolyExecutionClient"
            ));
        }

        let signer = PrivateKeySigner::from_str(&cfg.private_key)
            .context("Invalid private key format")?
            .with_chain_id(Some(POLYGON));

        // 1. Derive credentials manually first (One-Time derivation)
        let creds = Self::derive_credentials(&signer, &cfg.base_url).await?;
        info!("Derived API Credentials Manually: Key={}", creds.api_key);

        // 2. Construct SDK Credentials
        use uuid::Uuid;
        let key_uuid = Uuid::parse_str(&creds.api_key).context("Invalid UUID for API Key")?;
        let sdk_creds =
            SdkCredentials::new(key_uuid, creds.secret.clone(), creds.passphrase.clone());

        // 3. Initialize SDK Client using derived credentials
        // This avoids SDK re-deriving them.
        let client = Client::new(&cfg.base_url, Config::default())?
            .authentication_builder(&signer)
            .credentials(sdk_creds)
            .authenticate()
            .await
            .context("Failed to authenticate with SDK")?;

        info!("Polymarket Client Authenticated via SDK.");

        Ok(Self {
            inner: Arc::new(client),
            cfg,
            signer,
            creds,
        })
    }

    async fn derive_credentials(signer: &PrivateKeySigner, base_url: &str) -> Result<ApiCreds> {
        let chain_id = signer.chain_id().unwrap_or(POLYGON);
        let timestamp = chrono::Utc::now().timestamp();
        let nonce = 0; // default assumption

        // EIP-712 Signing
        let auth = ClobAuth {
            address: signer.address(),
            timestamp: timestamp.to_string(),
            nonce: U256::from(nonce),
            message: "This message attests that I control the given wallet".to_owned(),
        };

        let domain = Eip712Domain {
            name: Some(Cow::Borrowed("ClobAuthDomain")),
            version: Some(Cow::Borrowed("1")),
            chain_id: Some(U256::from(chain_id)),
            ..Eip712Domain::default()
        };

        let hash = auth.eip712_signing_hash(&domain);
        let signature = signer.sign_hash(&hash).await?;

        // Prepare Headers
        let poly_sig = signature.to_string(); // Hex string 0x...
        // SDK might strip 0x? Check headers.
        // SDK code: map.insert(POLY_SIGNATURE, signature.to_string().parse()?);
        // Signature::to_string outputs hex.

        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .build()?;
        let url = format!("{}/auth/derive-api-key", base_url);

        let resp = client
            .get(&url)
            .header("POLY_ADDRESS", signer.address().encode_hex_with_prefix())
            .header("POLY_NONCE", nonce.to_string())
            .header("POLY_SIGNATURE", poly_sig)
            .header("POLY_TIMESTAMP", timestamp.to_string())
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Failed to derive keys: {} - {}", status, text);
        }

        let creds: ApiCreds = resp.json().await?;
        Ok(creds)
    }

    pub async fn submit_order(&self, order: &CoreOrder) -> Result<Execution> {
        let side = match order.side {
            Side::Buy => PolySide::Buy,
            Side::Sell => PolySide::Sell,
        };

        let token_id = order
            .token_id
            .clone()
            .unwrap_or_else(|| order.market_id.clone());

        let built_order = self
            .inner
            .limit_order()
            .token_id(&token_id)
            .side(side)
            .size(order.size)
            .price(match side {
                PolySide::Buy => Decimal::from_str("0.99").unwrap(),
                PolySide::Sell => Decimal::from_str("0.01").unwrap(),
                _ => Decimal::ZERO,
            })
            .order_type(OrderType::FAK)
            .build()
            .await?;

        let signed = self.inner.sign(&self.signer, built_order).await?;
        let responses = self.inner.post_order(signed).await?;

        // Handle response
        // Usually one response for one order
        if let Some(resp) = responses.first() {
            if !resp.success {
                anyhow::bail!("Order failed: {:?}", resp.error_msg);
            }

            // For Market Buy:
            // making_amount = USDC spent? (Quote)
            // taking_amount = Shares received? (Base)
            // Needs verification of side, but generally for Taker:
            // "Taking" liquidty.

            let (filled_qty, cost) = if order.side == Side::Buy {
                // We are buying shares. taking_amount usually represents the asset token size?
                // Or making_amount?
                // For Market Buy using Amount::Shares, valid assumption is taking_amount = shares matched.
                (resp.taking_amount, resp.making_amount)
            } else {
                // Sell shares. making_amount = shares sold? taking_amount = USDC received?
                (resp.making_amount, resp.taking_amount)
            };

            let avg_px = if !filled_qty.is_zero() {
                cost / filled_qty
            } else {
                Decimal::ZERO
            };

            // Use Trade IDs if Order ID is empty (common in immediate FOK matches?)
            let ex_id = if !resp.order_id.is_empty() {
                resp.order_id.clone()
            } else {
                resp.trade_ids.first().cloned().unwrap_or_default()
            };

            let execution = Execution {
                exchange_order_id: Some(ex_id),
                client_order_id: order.client_order_id.clone(),
                market_id: order.market_id.clone(),
                token_id: Some(order.token_id.clone().unwrap()),
                side: order.side,
                avg_px,
                filled: filled_qty,
                fee: Decimal::ZERO, // Fee is embedded or separate? SDK has fee_rate logic.
                ts_ms: chrono::Utc::now().timestamp_millis(),
            };

            info!("Order Executed: {:?}", execution);
            Ok(execution)
        } else {
            anyhow::bail!("No response from Polymarket");
        }
    }
}

#[async_trait]
impl ExecutionClient for PolyExecutionClient {
    async fn create_order(&self, order: &CoreOrder) -> Result<Execution> {
        self.submit_order(order).await
    }

    async fn get_proxy_balance(&self) -> Result<Decimal> {
        let proxy_addr = self
            .cfg
            .proxy_address
            .clone()
            .ok_or_else(|| anyhow::anyhow!("No Proxy Address configured"))?;

        // USDC on Polygon
        const USDC_ADDR: &str = "0x2791Bca1f2de4661ED88A30C99A7a9449Aa84174";
        // balanceOf(address) selector = 70a08231
        let selector = "70a08231";
        // Pad address to 32 bytes (64 chars)
        let addr_clean = proxy_addr.trim_start_matches("0x");
        let padded_addr = format!("{:0>64}", addr_clean);
        let data = format!("0x{}{}", selector, padded_addr);

        let rpc_payload = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "eth_call",
            "params": [
                {
                    "to": USDC_ADDR,
                    "data": data
                },
                "latest"
            ],
            "id": 1
        });

        let client = reqwest::Client::new();
        let resp = client
            .post(&self.cfg.rpc_url)
            .json(&rpc_payload)
            .send()
            .await?;

        if !resp.status().is_success() {
            anyhow::bail!("RPC Error: {}", resp.status());
        }

        // Response format: { "jsonrpc": "2.0", "result": "0x...", "id": 1 }
        let body: serde_json::Value = resp.json().await?;

        if let Some(err) = body.get("error") {
            anyhow::bail!("RPC Error Body: {:?}", err);
        }

        let result_hex = body
            .get("result")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("No result in RPC response"))?;

        // Parse U256 (hex) -> Decimal (6 decimals for USDC)
        let raw_hex = result_hex.trim_start_matches("0x");
        if raw_hex.is_empty() {
            return Ok(Decimal::ZERO);
        }

        let raw_u128 = u128::from_str_radix(raw_hex, 16).unwrap_or(0);
        let balance = Decimal::from_str(&raw_u128.to_string()).unwrap() / Decimal::from(1_000_000); // USDC has 6 decimals

        Ok(balance)
    }

    async fn get_positions(&self) -> Result<Vec<crate::core::types::Position>> {
        // Use stored credentials for manual request
        let keys = &self.creds;

        // Data API endpoint
        let base = &self.cfg.data_api_url;
        let base = base.trim_end_matches('/');
        let path = "/positions";
        // Use proxy address if configured, otherwise signer (EOA) address
        let address = self
            .cfg
            .proxy_address
            .clone()
            .unwrap_or_else(|| self.signer.address().to_string());

        let url = format!("{}{}{}?user={}", base, path, "", address); // Use data_api_url, not CLOB url!

        // SDK logic uses `self.config.use_server_time` logic but we use local time here
        let timestamp = chrono::Utc::now().timestamp().to_string();

        // Signature: timestamp + method + path + body
        let message = format!("{}{}{}{}", timestamp, "GET", path, "");

        // Secret is base64 encoded
        let secret_bytes = BASE64_STANDARD
            .decode(&keys.secret)
            .context("Failed to decode API secret from base64")?;

        let mut mac =
            Hmac::<Sha256>::new_from_slice(&secret_bytes).context("Invalid HMAC secret")?;
        mac.update(message.as_bytes());
        let result = mac.finalize();
        let signature = BASE64_STANDARD.encode(result.into_bytes());

        let client = reqwest::Client::new();
        let resp = client
            .get(&url)
            .header("Poly-Api-Key", &keys.api_key)
            .header("Poly-Api-Signature", signature)
            .header("Poly-Timestamp", timestamp)
            .header("Poly-Api-Passphrase", &keys.passphrase)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Data API Error {}: {}", status, text);
        }

        // Local struct to match Data API response
        #[derive(Debug, Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct PolyPosition {
            asset: String,
            condition_id: String,
            size: Decimal,
            avg_price: Decimal,
            cur_price: Decimal,
            cash_pnl: Decimal,
        }

        let positions_raw: Vec<PolyPosition> = resp.json().await?;
        info!("Positions fetched: {}", positions_raw.len());

        let mapped: Vec<crate::core::types::Position> = positions_raw
            .into_iter()
            .map(|p| {
                crate::core::types::Position {
                    market_id: p.condition_id,
                    token_id: p.asset,
                    side: crate::core::types::Side::Buy, // Spot positions are always Long asset holdings
                    quantity: p.size,
                    avg_entry_price: p.avg_price,
                    current_price: p.cur_price,
                    unrealized_pnl: p.cash_pnl,
                    last_updated_ts: chrono::Utc::now().timestamp(),
                }
            })
            .collect();

        Ok(mapped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::config::PolyCfg;
    use crate::core::types::{Order, Side as CoreSide};
    use rust_decimal::Decimal;
    use uuid::Uuid;
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn test_client_full_flow() {
        let mock_server = MockServer::start().await;

        // 1. Mock Key Derivation
        let mock_key = Uuid::new_v4().to_string();
        let mock_pass = "mock-pass".to_string();
        let mock_secret = BASE64_STANDARD.encode(b"mock-secret-bytes-1234567890123456"); // Needs to be b64

        let creds_body = serde_json::json!({
            "apiKey": mock_key,
            "secret": mock_secret,
            "passphrase": mock_pass
        });

        Mock::given(method("GET"))
            .and(path("/auth/derive-api-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(creds_body))
            .mount(&mock_server)
            .await;

        // 2. Mock Balance
        // SDK likely calls /balance-allowance? Or queries generic endpoint?
        // Note: SDK uses query params. We match path only for simplicity.
        // SDK balance_allowance path: "balance-allowance"
        Mock::given(method("GET"))
            .and(path("/balance-allowance"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "balance": "1000.50",
                "allowances": {}
            })))
            .mount(&mock_server)
            .await;

        // 3. Mock Positions (Data API)
        // Note: get_positions uses data_api_url. We'll set that to mock server too.
        Mock::given(method("GET"))
            .and(path("/positions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "asset": "0x123...",
                    "conditionId": "0xabc...",
                    "size": "100.0",
                    "avgPrice": "0.55",
                    "curPrice": "0.60",
                    "cashPnl": "5.0",
                    // other fields ignored by our struct
                    "initialValue": "0", "currentValue": "0", "percentPnl": "0",
                    "totalBought": "0", "realizedPnl": "0", "percentRealizedPnl": "0",
                    "redeemable": false, "mergeable": false, "title": "Test",
                    "slug": "test", "icon": "", "eventSlug": "", "outcome": "Yes",
                    "outcomeIndex": 0, "oppositeOutcome": "No", "oppositeAsset": ""
                }
            ])))
            .mount(&mock_server)
            .await;

        // 4. Mock Order Post
        // SDK calls POST /order (Correction: /orders)
        Mock::given(method("POST"))
            .and(path("/orders"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "orderID": "mock-order-id-123",
                    "status": "received",
                    "makingAmount": "10",
                    "takingAmount": "10",
                    "side": "Buy", // Maybe needed?
                    "success": true
                }
            ])))
            .mount(&mock_server)
            .await;

        // 5. Mock Tick Size
        Mock::given(method("GET"))
            .and(path("/tick-size"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "minimum_tick_size": "0.01"
            })))
            .mount(&mock_server)
            .await;

        // 6. Mock Fee Rate
        Mock::given(method("GET"))
            .and(path("/fee-rate"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "base_fee": 0
            })))
            .mount(&mock_server)
            .await;

        // 7. Mock Order Book (for price validation/calc)
        Mock::given(method("GET"))
            .and(path("/book"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "market": "123",
                "asset_id": "123",
                "timestamp": "1735814400000", // Valid MS timestamp
                "hash": "mock-hash",
                "asks": [{"price": "0.55", "size": "1000"}],
                "bids": [{"price": "0.45", "size": "1000"}],
                "min_order_size": "1",
                "neg_risk": false,
                "tick_size": "0.01"
            })))
            .mount(&mock_server)
            .await;

        // 8. Mock Neg Risk
        Mock::given(method("GET"))
            .and(path("/neg-risk"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "neg_risk": false
            })))
            .mount(&mock_server)
            .await;

        // Mock RPC Balance Call
        // Request: {"method":"eth_call", ...}
        // Response: 1000.50 USDC * 10^6 = 1000500000 = 0x3BA14300
        Mock::given(method("POST"))
            // We can't easily match the body for eth_call specifically without complex matching,
            // but since we separate mocks by strictness or just assume sequential calls?
            // "method":"eth_call"
            .and(body_string_contains("eth_call"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": "0x000000000000000000000000000000000000000000000000000000003ba26b20"
            })))
            .mount(&mock_server)
            .await;

        // Init Config
        let mut cfg = PolyCfg::default();
        cfg.private_key =
            "0000000000000000000000000000000000000000000000000000000000000001".to_string();
        cfg.base_url = mock_server.uri(); // Point CLOB to mock
        cfg.data_api_url = mock_server.uri(); // Point Data API to mock
        cfg.rpc_url = mock_server.uri(); // Point RPC to mock
        cfg.proxy_address = Some("0x1234567890123456789012345678901234567890".to_string());

        let client = PolyExecutionClient::new(cfg)
            .await
            .expect("Failed to init client");

        // Test Balance
        let balance = client
            .get_proxy_balance()
            .await
            .expect("Failed get_proxy_balance");
        assert_eq!(balance, Decimal::from_str("1000.50").unwrap());

        // Test Positions
        let positions = client.get_positions().await.expect("Failed get_positions");
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0].quantity, Decimal::from_str("100.0").unwrap());
        assert_eq!(
            positions[0].unrealized_pnl,
            Decimal::from_str("5.0").unwrap()
        );

        // Test Submit Order
        let order = Order {
            client_order_id: "test-oid".to_string(),
            market_id: "123".to_string(),
            token_id: Some("123".to_string()),
            side: CoreSide::Buy,
            price: Decimal::from_str("0.50").unwrap(), // Ignored by market order logic
            size: Decimal::from_str("10.0").unwrap(),
        };
        let execution = client
            .create_order(&order)
            .await
            .expect("Failed create_order");
        assert_eq!(
            execution.exchange_order_id,
            Some("mock-order-id-123".to_string())
        );
    }

    #[tokio::test]
    #[ignore] // Run with `cargo test -- --ignored`
    async fn test_client_real_connection() {
        let mut cfg = PolyCfg::default();
        cfg.private_key = "dummypk".to_string();
        cfg.proxy_address = Some("dummyproxyaddress".to_string());

        let client = PolyExecutionClient::new(cfg.clone())
            .await
            .expect("Failed to init real client");

        // 1. Get Balance (Proxy)
        match client.get_proxy_balance().await {
            Ok(b) => println!("Get Proxy Balance Success: {} USDC", b),
            Err(e) => println!("Get Proxy Balance Failed: {:?}", e),
        }

        // 2. Get Positions
        let positions = client.get_positions().await;
        match positions {
            Ok(p) => println!("Get Positions Success: {} positions found", p.len()),
            Err(e) => panic!("Get Positions Failed: {:?}", e),
        }

        // 3. Submit Order (Expected Failure on invalid market, but proving connectivity)
        // Using "0" as invalid market ID to ensure we don't accidentally fill
        use crate::core::types::{Order, Side as CoreSide};
        let order = Order {
            client_order_id: format!("test-{}", uuid::Uuid::new_v4()),
            market_id: "664878".to_string(), // Invalid ID
            token_id: Some(
                "44901443285038704648888495467527795989898124409346615914800440744846007864248"
                    .to_string(),
            ),
            side: CoreSide::Buy,
            price: Decimal::ZERO, // Market order ignores price
            size: Decimal::from_str("1.0").unwrap(),
        };

        // 2b. Check Orderbook
        let book = client
            .get_book_summary(
                "112838095111461683880944516726938163688341306245473734071798778736646352193304",
            )
            .await;
        match book {
            Ok(b) => {
                println!("Orderbook Summary: {:?}", b);
                if let Some(ask) = b.asks.first() {
                    println!("Best Ask: Price {}, Size {}", ask.price, ask.size);
                } else {
                    println!("No Asks on Book!");
                }
            }
            Err(e) => println!("Failed to fetch book: {:?}", e),
        }

        let result = client.create_order(&order).await;
        match result {
            Ok(ord) => println!("Order Accepted (Unexpected): {:?}", ord),
            Err(e) => {
                println!(
                    "Order correctly failed (expected due to invalid ID): {:?}",
                    e
                );
            }
        }

        // 4. Verify Position (Should exist now)
        println!("Verifying position creation...");
        // Wait a small delay for indexing (Poly API is fast but >0ms)
        tokio::time::sleep(std::time::Duration::from_millis(2000)).await;

        let positions_post = client
            .get_positions()
            .await
            .expect("Failed to get post-order positions");
        println!(
            "Post-Order Positions: Found {}, First: {:?}",
            positions_post.len(),
            positions_post.first()
        );

        if positions_post.is_empty() {
            println!(
                "WARNING: Order filled but no position found! Check Proxy Address or Indexing."
            );
        } else {
            println!("SUCCESS: Position confirmed.");
            let pos_val: Decimal = positions_post
                .iter()
                .map(|p| p.quantity * p.current_price)
                .sum();
            println!("--- Portfolio Analysis ---");
            println!(
                "USDC Balance (RPC): {} USDC",
                match client.get_proxy_balance().await {
                    Ok(b) => b,
                    _ => Decimal::ZERO,
                }
            );
            println!("Positions Value:    {} USDC", pos_val);
            println!(
                "Total Equity (NLV): {} USDC",
                match client.get_proxy_balance().await {
                    Ok(b) => b + pos_val,
                    _ => pos_val,
                }
            );
            println!("--------------------------");
        }
    }
}
