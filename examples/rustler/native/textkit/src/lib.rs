use std::collections::HashMap;

use rustler::{Atom, Binary};
use sha2::{Digest, Sha256};

mod atoms {
    rustler::atoms! { invalid }
}

#[rustler::nif]
fn slugify(text: &str) -> String {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

#[rustler::nif]
fn word_count(text: &str) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for word in text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()) {
        *counts.entry(word.to_lowercase()).or_insert(0) += 1;
    }
    counts
}

#[rustler::nif(schedule = "DirtyCpu")]
fn sha256_hex(data: Binary) -> String {
    Sha256::digest(data.as_slice()).iter().map(|b| format!("{b:02x}")).collect()
}

// A Result becomes {:ok, value} or {:error, reason}. Errors are values, not panics: on wasm a panic
// aborts the whole VM rather than raising.
#[rustler::nif]
fn parse_int(text: &str) -> Result<i64, Atom> {
    text.trim().parse().map_err(|_| atoms::invalid())
}

rustler::init!("Elixir.TextKit.Native");
