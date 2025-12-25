use anyhow::Result;
use tantivy::collector::TopDocs as TopDocsStruct;
use tantivy::query::QueryParser;
use tantivy::schema::*;
use tantivy::{Index, IndexWriter};

pub struct MarketIndex {
    index: Index,
    writer: IndexWriter,
    // Fields
    title: Field,
    description: Field,
    tags: Field,
    id: Field,
    resolution_date: Field,
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

        Ok(Self {
            index,
            writer,
            title,
            description,
            tags,
            id,
            resolution_date,
        })
    }

    pub fn add_market(
        &mut self,
        market_id: &str,
        title_text: &str,
        desc_text: &str,
        tags_text: &str,
        res_date: Option<i64>,
    ) -> Result<()> {
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

        let query_parser =
            QueryParser::for_index(&self.index, vec![self.title, self.description, self.tags]);
        let query = query_parser.parse_query(&query_str)?;

        let collector = TopDocsStruct::with_limit(limit);
        let top_docs = searcher.search(&query, &collector)?;

        let mut results = Vec::new();
        for (score, doc_address) in top_docs {
            let retrieved_doc: TantivyDocument = searcher.doc(doc_address)?;

            let id_val = retrieved_doc
                .get_first(self.id)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let title_val = retrieved_doc
                .get_first(self.title)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let desc_val = retrieved_doc
                .get_first(self.description)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let tags_val = retrieved_doc
                .get_first(self.tags)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let res_date_val = retrieved_doc
                .get_first(self.resolution_date)
                .and_then(|v| v.as_i64());

            let tags_vec: Vec<String> = tags_val.split(',').map(|s| s.trim().to_string()).collect();

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
}
