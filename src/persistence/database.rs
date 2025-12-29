use crate::core::types::{Execution, Order, Position, RawNews, Side};
use anyhow::Result;
use rust_decimal::Decimal;
use sqlx::Row;
use sqlx::postgres::{PgPool, PgPoolOptions};
use tracing::info;

#[derive(Clone)]
pub struct Database {
    pub pool: PgPool,
}

impl Database {
    pub async fn new(connection_string: &str) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(connection_string)
            .await?;

        let db = Self { pool };
        db.init().await?;
        Ok(db)
    }

    pub async fn init(&self) -> Result<()> {
        // Create tables if valid
        // Postgres syntax:
        // - BIGSERIAL for auto-increment IDs usually, or GENERATED ALWAYS AS IDENTITY
        // - TIMESTAMPTZ for timestamps (recommended over TIMESTAMP)
        // - TEXT is fine
        // - JSONB for raw_json structure if we wanted, but TEXT is okay too.

        // Events
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS events (
                id BIGSERIAL PRIMARY KEY,
                url TEXT NOT NULL UNIQUE,
                title TEXT NOT NULL,
                description TEXT,
                source TEXT,
                published_at TIMESTAMPTZ,
                created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS markets (
                id TEXT PRIMARY KEY,
                question TEXT NOT NULL,
                description TEXT,
                start_date TEXT,
                end_date TEXT,
                active BOOLEAN,
                closed BOOLEAN,
                archived BOOLEAN,
                tokens JSONB, -- Store tokens as JSONB array
                created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Signals
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS signals (
                id BIGSERIAL PRIMARY KEY,
                event_id BIGINT NOT NULL REFERENCES events(id),
                market_id TEXT NOT NULL,
                sentiment TEXT NOT NULL,
                confidence FLOAT NOT NULL,
                raw_json TEXT NOT NULL,
                prompt TEXT, -- Add prompt for auditability
                model TEXT,  -- Add model name for auditability
                created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Orders
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS orders (
                id BIGSERIAL PRIMARY KEY,
                client_order_id TEXT NOT NULL UNIQUE,
                market_id TEXT NOT NULL,
                token_id TEXT,
                side TEXT NOT NULL,
                price TEXT NOT NULL, -- Decimal stored as text
                size TEXT NOT NULL,  -- Decimal stored as text
                status TEXT NOT NULL DEFAULT 'New',
                created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Executions
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS executions (
                id BIGSERIAL PRIMARY KEY,
                exchange_order_id TEXT,
                client_order_id TEXT NOT NULL,
                market_id TEXT NOT NULL,
                token_id TEXT,
                side TEXT NOT NULL,
                price TEXT NOT NULL,
                size TEXT NOT NULL,
                fee TEXT DEFAULT '0',
                created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Positions
        // Using upsert logic later, so need unique constraints
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS positions (
                market_id TEXT NOT NULL,
                token_id TEXT NOT NULL,
                side TEXT NOT NULL,
                quantity TEXT NOT NULL,
                avg_entry_price TEXT NOT NULL,
                current_price TEXT NOT NULL,
                unrealized_pnl TEXT NOT NULL,
                last_updated_ts TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (market_id, token_id)
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS decisions (
                id BIGSERIAL PRIMARY KEY,
                event_id BIGINT, -- Optional if we can't always link
                market_id TEXT NOT NULL,
                side TEXT NOT NULL,
                score TEXT NOT NULL,
                probability TEXT NOT NULL,
                market_price TEXT NOT NULL,
                edge TEXT NOT NULL,
                kelly_fraction TEXT NOT NULL,
                size_fraction TEXT NOT NULL,
                created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Retrieval Logs
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS retrieval_logs (
                id BIGSERIAL PRIMARY KEY,
                event_id BIGINT NOT NULL,
                market_id TEXT NOT NULL,
                created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Verify tables exist
        let tables: Vec<(String,)> = sqlx::query_as(
            "SELECT table_name FROM information_schema.tables WHERE table_schema = 'public'",
        )
        .fetch_all(&self.pool)
        .await?;

        info!(
            "Database tables initialized (Postgres). Found tables: {:?}",
            tables.iter().map(|t| &t.0).collect::<Vec<_>>()
        );
        Ok(())
    }

    // --- Persist Methods ---

    pub async fn save_decision(
        &self,
        event_id: Option<i64>,
        decision: &crate::strategy::types::SizedDecision,
    ) -> Result<()> {
        let start = std::time::Instant::now();
        let side = match &decision.side {
            crate::strategy::types::TradeSide::Buy(outcome) => outcome.as_str(),
        };
        // Decimal to string
        let score = decision.candidate.score.to_string();
        let prob = decision.candidate.probability.to_string();
        let mkt_price = decision.candidate.market_price.to_string();
        let edge = decision.candidate.edge.to_string();
        let kelly = decision.kelly_fraction.to_string();
        let size_frac = decision.size_fraction.to_string();
        let mkt_id = &decision.candidate.candidate.market_id;

        let res = sqlx::query(
            r#"
            INSERT INTO decisions (event_id, market_id, side, score, probability, market_price, edge, kelly_fraction, size_fraction)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
        )
        .bind(event_id)
        .bind(mkt_id)
        .bind(side)
        .bind(score)
        .bind(prob)
        .bind(mkt_price)
        .bind(edge)
        .bind(kelly)
        .bind(size_frac)
        .execute(&self.pool)
        .await;

        match res {
            Ok(_) => {
                metrics::counter!("database_queries_total", "table" => "decisions", "op" => "insert", "status" => "success").increment(1);
            }
            Err(_) => {
                metrics::counter!("database_queries_total", "table" => "decisions", "op" => "insert", "status" => "error").increment(1);
            }
        }
        res?;
        metrics::histogram!("database_query_duration_seconds", "table" => "decisions", "op" => "insert").record(start.elapsed().as_secs_f64());
        Ok(())
    }

    pub async fn save_retrieval_log(&self, event_id: i64, market_id: &str) -> Result<()> {
        let start = std::time::Instant::now();
        let res = sqlx::query(
            r#"
            INSERT INTO retrieval_logs (event_id, market_id)
            VALUES ($1, $2)
            "#,
        )
        .bind(event_id)
        .bind(market_id)
        .execute(&self.pool)
        .await;

        match res {
            Ok(_) => {
                metrics::counter!("database_queries_total", "table" => "retrieval_logs", "op" => "insert", "status" => "success").increment(1);
            }
            Err(_) => {
                metrics::counter!("database_queries_total", "table" => "retrieval_logs", "op" => "insert", "status" => "error").increment(1);
            }
        }
        res?;
        metrics::histogram!("database_query_duration_seconds", "table" => "retrieval_logs", "op" => "insert").record(start.elapsed().as_secs_f64());
        Ok(())
    }

    pub async fn save_event(&self, news: &RawNews) -> Result<i64> {
        let start = std::time::Instant::now();
        // RETURNING id in Postgres
        let rec = sqlx::query(
            r#"
            INSERT INTO events (url, title, description, source, published_at)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (url) DO UPDATE SET title = EXCLUDED.title -- Simple no-op or update
            RETURNING id
            "#,
        )
        .bind(&news.url)
        .bind(&news.title)
        .bind(&news.description)
        .bind(&news.feed)
        .bind(news.published)
        .fetch_one(&self.pool)
        .await;

        match &rec {
             Ok(_) => metrics::counter!("database_queries_total", "table" => "events", "op" => "insert", "status" => "success").increment(1),
             Err(_) => metrics::counter!("database_queries_total", "table" => "events", "op" => "insert", "status" => "error").increment(1),
        }

        metrics::histogram!("database_query_duration_seconds", "table" => "events", "op" => "insert").record(start.elapsed().as_secs_f64());

        Ok(rec?.get("id"))
    }

    pub async fn save_market(&self, market: &crate::core::types::PolyMarketMarket) -> Result<()> {
        let start = std::time::Instant::now();
        let tokens_vec = market.get_tokens();
        let tokens_json = serde_json::to_value(&tokens_vec).unwrap_or(serde_json::Value::Null);

        let res = sqlx::query(
            r#"
            INSERT INTO markets (id, question, description, start_date, end_date, active, closed, archived, tokens)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            ON CONFLICT (id) DO UPDATE SET
                question = EXCLUDED.question,
                description = EXCLUDED.description,
                active = EXCLUDED.active,
                closed = EXCLUDED.closed,
                archived = EXCLUDED.archived
            "#,
        )
        .bind(&market.id)
        .bind(&market.question)
        .bind(&market.description)
        .bind(&market.start_date)
        .bind(&market.end_date)
        .bind(market.active)
        .bind(market.closed)
        .bind(market.archived)
        .bind(tokens_json)
        .execute(&self.pool)
        .await;

        match res {
            Ok(_) => {
                metrics::counter!("database_queries_total", "table" => "markets", "op" => "upsert", "status" => "success").increment(1);
            }
            Err(_) => {
                metrics::counter!("database_queries_total", "table" => "markets", "op" => "upsert", "status" => "error").increment(1);
            }
        }
        res?;
        metrics::histogram!("database_query_duration_seconds", "table" => "markets", "op" => "upsert").record(start.elapsed().as_secs_f64());
        Ok(())
    }

    pub async fn save_signal(
        &self,
        event_id: i64,
        market_id: &str,
        signal: &crate::llm::SignalResponse,
        prompt: &str,
        model: &str,
    ) -> Result<()> {
        let start = std::time::Instant::now();
        let json = serde_json::to_string(signal)?;
        let res = sqlx::query(
            r#"
            INSERT INTO signals (event_id, market_id, sentiment, confidence, raw_json, prompt, model)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(event_id)
        .bind(market_id)
        .bind(&signal.sentiment)
        .bind(signal.confidence)
        .bind(json)
        .bind(prompt)
        .bind(model)
        .execute(&self.pool)
        .await;

        match res {
            Ok(_) => {
                metrics::counter!("database_queries_total", "table" => "signals", "op" => "insert", "status" => "success").increment(1);
            }
            Err(_) => {
                metrics::counter!("database_queries_total", "table" => "signals", "op" => "insert", "status" => "error").increment(1);
            }
        }
        res?;
        metrics::histogram!("database_query_duration_seconds", "table" => "signals", "op" => "insert").record(start.elapsed().as_secs_f64());
        Ok(())
    }

    pub async fn save_order(&self, order: &Order) -> Result<()> {
        let start = std::time::Instant::now();
        let side = match order.side {
            Side::Buy => "Buy",
            Side::Sell => "Sell",
        };
        // Decimal to string
        let price_str = order.price.to_string();
        let size_str = order.size.to_string();

        let res = sqlx::query(
            r#"
            INSERT INTO orders (client_order_id, market_id, token_id, side, price, size)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(&order.client_order_id)
        .bind(&order.market_id)
        .bind(&order.token_id)
        .bind(side)
        .bind(price_str)
        .bind(size_str)
        .execute(&self.pool)
        .await;

        match res {
            Ok(_) => {
                metrics::counter!("database_queries_total", "table" => "orders", "op" => "insert", "status" => "success").increment(1);
            }
            Err(_) => {
                metrics::counter!("database_queries_total", "table" => "orders", "op" => "insert", "status" => "error").increment(1);
            }
        }
        res?;
        metrics::histogram!("database_query_duration_seconds", "table" => "orders", "op" => "insert").record(start.elapsed().as_secs_f64());
        Ok(())
    }

    pub async fn save_execution(&self, exec: &Execution) -> Result<()> {
        let start = std::time::Instant::now();
        let side = match exec.side {
            Side::Buy => "Buy",
            Side::Sell => "Sell",
        };
        let price_str = exec.avg_px.to_string(); // mapped from avg_px
        let size_str = exec.filled.to_string(); // mapped from filled
        let fee_str = exec.fee.to_string();

        let res = sqlx::query(
            r#"
            INSERT INTO executions (exchange_order_id, client_order_id, market_id, token_id, side, price, size, fee)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(&exec.exchange_order_id)
        .bind(&exec.client_order_id)
        .bind(&exec.market_id)
        .bind(&exec.token_id)
        .bind(side)
        .bind(price_str)
        .bind(size_str)
        .bind(fee_str)
        .execute(&self.pool)
        .await;

        match res {
            Ok(_) => {
                metrics::counter!("database_queries_total", "table" => "executions", "op" => "insert", "status" => "success").increment(1);
            }
            Err(_) => {
                metrics::counter!("database_queries_total", "table" => "executions", "op" => "insert", "status" => "error").increment(1);
            }
        }
        res?;
        metrics::histogram!("database_query_duration_seconds", "table" => "executions", "op" => "insert").record(start.elapsed().as_secs_f64());
        Ok(())
    }

    pub async fn upsert_position(&self, pos: &Position) -> Result<()> {
        let start = std::time::Instant::now();
        let side = match pos.side {
            Side::Buy => "Buy",
            Side::Sell => "Sell",
        };
        let qty = pos.quantity.to_string();
        let avg = pos.avg_entry_price.to_string();
        let curr = pos.current_price.to_string();
        let pnl = pos.unrealized_pnl.to_string();

        let res = sqlx::query(
            r#"
            INSERT INTO positions (market_id, token_id, side, quantity, avg_entry_price, current_price, unrealized_pnl)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (market_id, token_id) DO UPDATE SET
                side = EXCLUDED.side,
                quantity = EXCLUDED.quantity,
                avg_entry_price = EXCLUDED.avg_entry_price,
                current_price = EXCLUDED.current_price,
                unrealized_pnl = EXCLUDED.unrealized_pnl,
                last_updated_ts = CURRENT_TIMESTAMP
            "#,
        )
        .bind(&pos.market_id)
        .bind(&pos.token_id)
        .bind(side)
        .bind(qty)
        .bind(avg)
        .bind(curr)
        .bind(pnl)
        .execute(&self.pool)
        .await;

        match res {
            Ok(_) => {
                metrics::counter!("database_queries_total", "table" => "positions", "op" => "upsert", "status" => "success").increment(1);
            }
            Err(_) => {
                metrics::counter!("database_queries_total", "table" => "positions", "op" => "upsert", "status" => "error").increment(1);
            }
        }
        res?;
        metrics::histogram!("database_query_duration_seconds", "table" => "positions", "op" => "upsert").record(start.elapsed().as_secs_f64());
        Ok(())
    }

    pub async fn load_positions(&self) -> Result<Vec<Position>> {
        let start = std::time::Instant::now();
        let rows = sqlx::query(
            r#"
            SELECT market_id, token_id, side, quantity, avg_entry_price, current_price, unrealized_pnl, last_updated_ts
            FROM positions
            "#,
        )
        .fetch_all(&self.pool)
        .await;

        match &rows {
            Ok(_) => {
                metrics::counter!("database_queries_total", "table" => "positions", "op" => "select", "status" => "success").increment(1);
            }
            Err(_) => {
                metrics::counter!("database_queries_total", "table" => "positions", "op" => "select", "status" => "error").increment(1);
            }
        }
        let rows = rows?;
        metrics::histogram!("database_query_duration_seconds", "table" => "positions", "op" => "select").record(start.elapsed().as_secs_f64());

        let mut positions = Vec::new();
        for row in rows {
            let market_id: String = row.get("market_id");
            let token_id: String = row.get("token_id");
            let side_str: String = row.get("side");
            let qty_str: String = row.get("quantity");
            let avg_str: String = row.get("avg_entry_price");
            let curr_str: String = row.get("current_price");
            let pnl_str: String = row.get("unrealized_pnl");
            let last_updated_ts_pg: chrono::DateTime<chrono::Utc> = row.get("last_updated_ts");

            let side = if side_str == "Buy" {
                Side::Buy
            } else {
                Side::Sell
            };
            let quantity = Decimal::from_str_exact(&qty_str).unwrap_or(Decimal::ZERO);
            let avg_entry_price = Decimal::from_str_exact(&avg_str).unwrap_or(Decimal::ZERO);
            let current_price = Decimal::from_str_exact(&curr_str).unwrap_or(Decimal::ZERO);
            let unrealized_pnl = Decimal::from_str_exact(&pnl_str).unwrap_or(Decimal::ZERO);

            positions.push(Position {
                market_id,
                token_id,
                side,
                quantity,
                avg_entry_price,
                current_price,
                unrealized_pnl,
                last_updated_ts: last_updated_ts_pg.timestamp_millis(),
            });
        }
        Ok(positions)
    }

    pub async fn load_recent_events(&self, limit: i64) -> Result<Vec<(String, String)>> {
        let start = std::time::Instant::now();
        let rows = sqlx::query(
            r#"
            SELECT title, description
            FROM events
            ORDER BY created_at DESC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await;

        match &rows {
            Ok(_) => {
                metrics::counter!("database_queries_total", "table" => "events", "op" => "select", "status" => "success").increment(1);
            }
            Err(_) => {
                metrics::counter!("database_queries_total", "table" => "events", "op" => "select", "status" => "error").increment(1);
            }
        }
        let rows = rows?;
        metrics::histogram!("database_query_duration_seconds", "table" => "events", "op" => "select").record(start.elapsed().as_secs_f64());

        let mut events = Vec::new();
        for row in rows {
            let title: String = row.get("title");
            let description: Option<String> = row.get("description");
            events.push((title, description.unwrap_or_default()));
        }
        Ok(events)
    }

    pub async fn load_active_markets(&self) -> Result<Vec<(String, String, String)>> {
        let rows = sqlx::query(
            r#"
            SELECT id, question, description
            FROM markets
            WHERE active = true AND closed = false
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let mut markets = Vec::new();
        for row in rows {
            let id: String = row.get("id");
            let question: String = row.get("question");
            let description: Option<String> = row.get("description");
            let description = description.unwrap_or_default();
            markets.push((id, question, description));
        }
        Ok(markets)
    }
}
