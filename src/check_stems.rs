use rust_stemmers::{Algorithm, Stemmer};

fn main() {
    let stemmer = Stemmer::create(Algorithm::English);
    println!("digital -> {}", stemmer.stem("digital"));
    println!("currency -> {}", stemmer.stem("currency"));
}
