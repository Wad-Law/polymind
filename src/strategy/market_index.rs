use anyhow::Result;
use tantivy::collector::TopDocs as TopDocsStruct;
use tantivy::query::QueryParser;
use tantivy::schema::*;
use tantivy::{Index, IndexWriter};

use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use std::collections::HashMap;

pub struct MarketIndex {
    index: Index,
    writer: IndexWriter,
    // Fields
    title: Field,
    description: Field,
    tags: Field,
    id: Field,
    resolution_date: Field,
    // Semantic Search
    embedding_model: TextEmbedding,
    vectors: HashMap<String, Vec<f32>>,
}

impl MarketIndex {
    pub fn new() -> Result<Self> {
        let mut schema_builder = Schema::builder();

        // Text fields are indexed and stored (if needed for retrieval, though we mostly need ID)
        // For BM25, we need them indexed.
        let title = schema_builder.add_text_field("title", TEXT | STORED);
        let description = schema_builder.add_text_field("description", TEXT);
        let tags = schema_builder.add_text_field("tags", TEXT);

        // ID is stored so we can map back to the market
        let id = schema_builder.add_text_field("id", STRING | STORED);

        // Resolution date (timestamp in seconds) - stored and indexed (FAST) for range queries
        let resolution_date = schema_builder.add_i64_field("resolution_date", INDEXED | STORED);

        let schema = schema_builder.build();

        // Create in-memory index for now (fast, ephemeral)
        // In production, you might want a directory.
        let index = Index::create_in_ram(schema.clone());

        // 50MB buffer for indexing
        let writer = index.writer(50_000_000)?;

        // Initialize Embedding Model
        let embedding_model = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::AllMiniLML6V2).with_show_download_progress(true),
        )?;

        Ok(Self {
            index,
            writer,
            title,
            description,
            tags,
            id,
            resolution_date,
            embedding_model,
            vectors: HashMap::new(),
        })
    }

    pub fn contains(&self, market_id: &str) -> bool {
        self.vectors.contains_key(market_id)
    }

    pub fn delete_market(&mut self, market_id: &str) -> Result<()> {
        let term = Term::from_field_text(self.id, market_id);
        self.writer.delete_term(term);
        self.writer.commit()?;

        // Remove from semantic cache
        self.vectors.remove(market_id);
        Ok(())
    }

    pub fn add_market(
        &mut self,
        market_id: &str,
        title_text: &str,
        desc_text: &str,
        tags_text: &str,
        res_date: Option<i64>,
    ) -> Result<()> {
        // First delete to ensure update/replace
        let term = Term::from_field_text(self.id, market_id);
        self.writer.delete_term(term);

        let mut doc = TantivyDocument::default();
        doc.add_text(self.title, title_text);
        doc.add_text(self.description, desc_text);
        doc.add_text(self.tags, tags_text);
        doc.add_text(self.id, market_id);

        if let Some(d) = res_date {
            doc.add_i64(self.resolution_date, d);
        }

        self.writer.add_document(doc)?;
        // In a real high-throughput system, you wouldn't commit every time.
        // But for low-frequency market additions, it's fine.
        self.writer.commit()?;

        // Generate and store embedding
        // Combine title and description for better context
        let text_to_embed = format!("{} {}", title_text, desc_text);
        let embeddings = self.embedding_model.embed(vec![text_to_embed], None)?;
        if let Some(embedding) = embeddings.first() {
            self.vectors
                .insert(market_id.to_string(), embedding.clone());
        }

        Ok(())
    }

    pub fn search(
        &self,
        query_tokens: &[String],
        limit: usize,
    ) -> Result<Vec<crate::strategy::types::RawCandidate>> {
        let reader = self.index.reader()?;
        let searcher = reader.searcher();

        // Construct a query from tokens
        // Simple approach: OR query of all tokens
        let query_str = query_tokens.join(" ");

        let mut query_parser =
            QueryParser::for_index(&self.index, vec![self.title, self.description, self.tags]);
        query_parser.set_conjunction_by_default();
        let query = query_parser.parse_query(&query_str)?;

        let collector = TopDocsStruct::with_limit(limit);
        let top_docs = searcher.search(&query, &collector)?;

        let mut results = Vec::new();
        for (score, doc_address) in top_docs {
            let retrieved_doc: TantivyDocument = searcher.doc(doc_address)?;
            let id_val = self.extract_id(&retrieved_doc);
            let (title_val, desc_val, tags_vec, res_date_val) =
                self.extract_metadata(&retrieved_doc);

            results.push(crate::strategy::types::RawCandidate {
                market_id: id_val,
                bm25_score: score,
                title: title_val,
                description: desc_val,
                tags: tags_vec,
                resolution_date: res_date_val,
            });
        }

        Ok(results)
    }

    pub fn search_semantic(
        &mut self,
        query_text: &str,
        limit: usize,
    ) -> Result<Vec<crate::strategy::types::RawCandidate>> {
        let embeddings = self.embedding_model.embed(vec![query_text], None)?;
        let query_embedding = match embeddings.first() {
            Some(e) => e,
            None => return Ok(Vec::new()),
        };

        // Brute-force Cosine Similarity (fast enough for <10k items)
        let mut scored_markets: Vec<(String, f32)> = self
            .vectors
            .iter()
            .map(|(id, vec)| {
                let score = cosine_similarity(query_embedding, vec);
                (id.clone(), score)
            })
            .collect();

        // Sort by score descending
        scored_markets.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Filter by threshold (e.g., 0.35) to avoid irrelevant matches
        let threshold = 0.35;
        let top_k = scored_markets
            .into_iter()
            .filter(|(_, score)| *score >= threshold)
            .take(limit);

        // Retrieve metadata from Tantivy (or we could store it in memory too, but Tantivy is fine)
        let reader = self.index.reader()?;
        let searcher = reader.searcher();
        let term_query_parser = QueryParser::for_index(&self.index, vec![self.id]);

        let mut results = Vec::new();
        for (id, score) in top_k {
            // We need to find the doc by ID to get metadata
            // This is a bit inefficient (querying by ID), but fine for small K.
            // Optimization: Store metadata in memory or use a DocId map.
            let query = term_query_parser.parse_query(&format!("id:\"{}\"", id))?;
            let top_docs = searcher.search(&query, &TopDocsStruct::with_limit(1))?;

            if let Some((_, doc_addr)) = top_docs.first() {
                let retrieved_doc: TantivyDocument = searcher.doc(*doc_addr)?;
                let (title_val, desc_val, tags_vec, res_date_val) =
                    self.extract_metadata(&retrieved_doc);

                results.push(crate::strategy::types::RawCandidate {
                    market_id: id,
                    bm25_score: score, // Using cosine similarity as "score"
                    title: title_val,
                    description: desc_val,
                    tags: tags_vec,
                    resolution_date: res_date_val,
                });
            }
        }

        Ok(results)
    }

    fn extract_id(&self, doc: &TantivyDocument) -> String {
        doc.get_first(self.id)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    }

    fn extract_metadata(
        &self,
        doc: &TantivyDocument,
    ) -> (String, String, Vec<String>, Option<i64>) {
        let title_val = doc
            .get_first(self.title)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let desc_val = doc
            .get_first(self.description)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let tags_val = doc
            .get_first(self.tags)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let res_date_val = doc.get_first(self.resolution_date).and_then(|v| v.as_i64());
        let tags_vec: Vec<String> = tags_val.split(',').map(|s| s.trim().to_string()).collect();
        (title_val, desc_val, tags_vec, res_date_val)
    }
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot_product: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot_product / (norm_a * norm_b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_market_index_basic() -> Result<()> {
        let mut index: MarketIndex = MarketIndex::new()?;

        index.add_market(
            "1",
            "Fed rates decision",
            "Will the Fed hike rates?",
            "fed, rates, macro",
            None,
        )?;
        index.add_market(
            "2",
            "Bitcoin price",
            "Will BTC hit 100k?",
            "crypto, btc",
            None,
        )?;
        index.add_market(
            "3",
            "US Election",
            "Who will win?",
            "politics, us",
            Some(1700000000),
        )?;

        // Search for "Fed"
        let results = index.search(&vec!["Fed".to_string()], 10)?;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].market_id, "1");

        // Search for "rates"
        let results = index.search(&vec!["rates".to_string()], 10)?;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].market_id, "1");

        // Search for "crypto"
        let results = index.search(&vec!["crypto".to_string()], 10)?;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].market_id, "2");

        Ok(())
    }

    #[test]
    fn test_semantic_search() -> Result<()> {
        let mut index: MarketIndex = MarketIndex::new()?;

        index.add_market(
            "1",
            "Fed rates decision",
            "Will the Fed hike rates?",
            "fed, rates, macro",
            None,
        )?;
        index.add_market(
            "2",
            "Bitcoin price",
            "Will BTC hit 100k?",
            "crypto, btc",
            None,
        )?;

        // Semantic search for "interest rate increase" (not in text, but semantically related to Fed rates)
        let results = index.search_semantic("interest rate increase", 1)?;
        assert!(!results.is_empty());
        assert_eq!(results[0].market_id, "1");

        // Semantic search for "cryptocurrency surge"
        let results = index.search_semantic("cryptocurrency surge", 1)?;
        assert!(!results.is_empty());
        assert_eq!(results[0].market_id, "2");

        Ok(())
    }
}
