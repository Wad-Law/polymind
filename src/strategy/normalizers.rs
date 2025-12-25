use deunicode::deunicode;
use regex::Regex;
use crate::core::types::RawNews;

pub fn normalize_news_item_dedup_stage(news: &RawNews) -> String {
    lazy_static::lazy_static! {
        static ref URL_RE: Regex = Regex::new(r"https?://\S+").unwrap(); // strip urls
        static ref PUNCT_RE: Regex = Regex::new(r"[^\w\s]").unwrap(); // strip any char not a word char or whitespace (emoji, punc, ..etc)
    }

    // tune what you combine here
    let combined = format!("{} {}", news.title, news.description);

    let lower = combined.to_lowercase();
    let no_url = URL_RE.replace_all(&lower, "");
    let ascii = deunicode(&no_url); // Converts accented / unicode characters into ASCII equivalents: ñ -> n
    let no_punct = PUNCT_RE.replace_all(&ascii, "");

    // collaps tabs/new lines/ multiple spaces into a single space
    let collapsed = no_punct
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    collapsed.trim().to_string()
}


pub fn normalize_for_matching(news: &RawNews) -> String {
    lazy_static::lazy_static! {
        // Remove URLs
        static ref URL_RE: Regex = Regex::new(r"https?://\S+").unwrap();
        // You can keep punctuation for now; we will mostly rely on tokenization
    }

    // Combine title + description (you can add source, snippet, etc. if useful)
    let combined = format!("{} {}", news.title, news.description);

    // 1) lowercase for consistency
    let lower = combined.to_lowercase();

    // 2) remove URLs (they are noise)
    let no_url = URL_RE.replace_all(&lower, "");

    // 3) deunicode (é -> e, ñ -> n, …)
    let ascii = deunicode(&no_url);

    // 4) collapse multiple whitespace (spaces, tabs, newlines...)
    let collapsed = ascii
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    // 5) trim edges
    collapsed.trim().to_string()
}