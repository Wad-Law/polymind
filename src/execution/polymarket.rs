use crate::config::config::PolyCfg;
use crate::core::types::{Execution, Order as CoreOrder, Side};
use anyhow::Result;
use ethers::abi::Address;
use ethers::prelude::*;
use ethers::signers::{LocalWallet, Signer};
use ethers::types::U256;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{error, info};

#[derive(Clone)]
pub struct PolyExecutionClient {
    client: Client,
    cfg: PolyCfg,
    wallet: Option<LocalWallet>,
}

// EIP-712 Order Struct for Polymarket CTF Exchange
// Domain:
// name: "Polymarket CTF Exchange"
// version: "1"
// chainId: 137
// verifyingContract: "0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E"
#[allow(non_snake_case)]
#[derive(Debug, Clone, Eip712, EthAbiType, Serialize)]
#[eip712(
    name = "Polymarket CTF Exchange",
    version = "1",
    chain_id = 137,
    verifying_contract = "0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E"
)]
pub struct Order {
    pub salt: U256,
    pub maker: Address,
    pub signer: Address,
    pub taker: Address,
    pub tokenId: U256,
    pub makerAmount: U256,
    pub takerAmount: U256,
    pub expiration: U256,
    pub nonce: U256,
    pub feeRateBps: U256,
    pub side: U256,          // 0 for BUY, 1 for SELL
    pub signatureType: U256, // 0 for EOA, 1 for POLY_PROXY, etc.
}

#[derive(Debug, Serialize)]
struct CreateOrderRequest {
    order: Order,
    owner: Address,
    signature: String,
    #[serde(rename = "orderType")]
    order_type: String, // "GTC", "FOK", "GTD"
}

#[derive(Debug, Deserialize)]
struct CreateOrderResponse {
    #[serde(rename = "orderID")]
    order_id: String,
    // other fields...
}

impl PolyExecutionClient {
    pub fn new(cfg: PolyCfg, client: Client) -> Self {
        let wallet = if !cfg.api_key.is_empty() && !cfg.api_secret.is_empty() {
            // In a real implementation, we would derive the wallet from the private key (api_secret or similar)
            // For Polymarket, the API interaction often involves signing EIP-712 messages with a proxy wallet key.
            // Here we assume api_secret is the private key for simplicity in this "real" implementation attempt.
            // WARNING: In production, handle keys securely!
            match LocalWallet::from_str(&cfg.api_secret) {
                Ok(w) => Some(w.with_chain_id(137u64)),
                Err(e) => {
                    error!("Failed to create wallet from api_secret: {}", e);
                    None
                }
            }
        } else {
            None
        };

        Self {
            client,
            cfg,
            wallet,
        }
    }

    pub async fn create_order(&self, order: &CoreOrder) -> Result<Execution> {
        if self.wallet.is_none() {
            anyhow::bail!("No wallet configured for signing orders");
        }
        let wallet = self.wallet.as_ref().unwrap();
        let maker_address = wallet.address();

        // 1. Construct the EIP-712 Order
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let expiration = now + 300; // 5 minutes expiration
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();

        // Conversion logic (Simplified for demonstration)
        // Polymarket uses token units (e.g. 10^6 for USDC).
        // Assuming price is 0.0-1.0 and size is number of shares.
        // For a BUY:
        // makerAmount = size * price (USDC)
        // takerAmount = size (Outcome Tokens)
        // side = 0 (BUY)

        // Actually size in Polymarket API is usually raw units.
        // NOTE: This is a simplified conversion. Real Polymarket logic requires precise token decimals.
        // Assuming USDC has 6 decimals.
        // Use configured token decimals for scaling
        let decimals = self.cfg.token_decimals;
        let scale = 10u64.pow(decimals) as f64;

        // Convert price and size to U256
        // Price is usually per unit, Size is number of units.
        // For USDC (6 decimals), price 0.50 -> 500000
        let price_f64 = order.price as f64;
        let size_f64 = order.size as f64;

        // makerAmount and takerAmount depend on side
        // BUY: maker = USDC (price * size), taker = Asset (size)
        // SELL: maker = Asset (size), taker = USDC (price * size)

        // NOTE: This assumes the asset also has the same decimals as the collateral (USDC).
        // If they differ, we need separate configs. For now, we use token_decimals for both.

        let (maker_amount_val, taker_amount_val) = match order.side {
            Side::Buy => (price_f64 * size_f64 * scale, size_f64 * scale), // Paying USDC, getting Asset
            Side::Sell => (size_f64 * scale, price_f64 * size_f64 * scale), // Paying Asset, getting USDC
        };

        // We need to be careful with the math here.
        // If price_val is scaled, and size is raw units?
        // Let's stick to the previous logic but use the dynamic scale.
        // Previous logic:
        // maker_amount = price * size * 1e6 (if buying) -> wait.
        // If I buy 10 units at 0.5 USDC:
        // Cost = 5 USDC = 5 * 1e6 = 5,000,000.
        // Taker amount = 10 units = 10 * 1e6 = 10,000,000 (if asset has 6 decimals).

        // Let's recalculate carefully.
        // price_f64 = 0.5
        // size_f64 = 10.0
        // scale = 1_000_000.0

        // BUY:
        // makerAmount (USDC) = price_f64 * size_f64 * scale = 0.5 * 10.0 * 1e6 = 5,000,000. Correct.
        // takerAmount (Asset) = size_f64 * scale = 10.0 * 1e6 = 10,000,000. Correct.

        // SELL:
        // makerAmount (Asset) = size_f64 * scale = 10,000,000. Correct.
        // takerAmount (USDC) = price_f64 * size_f64 * scale = 5,000,000. Correct.

        let maker_amount = U256::from((maker_amount_val).round() as u64);
        let taker_amount = U256::from((taker_amount_val).round() as u64);

        let poly_order = Order {
            salt: U256::from(nonce), // Using nonce as salt for uniqueness
            maker: maker_address,
            signer: maker_address,
            taker: Address::zero(), // Open order
            tokenId: U256::from_dec_str(&order.market_id).unwrap_or(U256::zero()), // Market ID must be numeric token ID
            makerAmount: maker_amount,
            takerAmount: taker_amount,
            expiration: U256::from(expiration),
            nonce: U256::from(nonce),
            feeRateBps: U256::zero(),
            side: U256::from(match order.side {
                Side::Buy => 0,
                Side::Sell => 1,
            }),
            signatureType: U256::zero(), // 0 = EOA
        };

        // 2. Sign the order
        let signature = wallet.sign_typed_data(&poly_order).await?;
        let signature_str = format!("0x{}", hex::encode(signature.to_vec()));

        // 3. Send to CLOB API
        let req = CreateOrderRequest {
            order: poly_order.clone(),
            owner: maker_address,
            signature: signature_str,
            order_type: "FOK".to_string(), // Fill-Or-Kill for simplicity
        };

        info!("Sending order to Polymarket: {:?}", req);

        // Real Network Call
        let url = format!("{}/order", self.cfg.base_url);
        let res = self
            .client
            .post(&url)
            .header("Poly-Api-Key", &self.cfg.api_key)
            .header("Poly-Passphrase", &self.cfg.passphrase)
            .header("Poly-Api-Secret", &self.cfg.api_secret) // Some APIs need this header too
            .json(&req)
            .send()
            .await?;

        if !res.status().is_success() {
            let error_text = res.text().await?;
            error!("Polymarket API error: {}", error_text);
            anyhow::bail!("Polymarket API error: {}", error_text);
        }

        let resp_json: CreateOrderResponse = res.json().await?;
        info!("Order placed successfully: {:?}", resp_json);

        let execution = Execution {
            client_order_id: order.client_order_id.clone(),
            market_id: order.market_id.clone(),
            avg_px: order.price,
            filled: order.size,
            fee: 0.0,
            ts_ms: chrono::Utc::now().timestamp_millis(),
        };

        Ok(execution)
    }
}
