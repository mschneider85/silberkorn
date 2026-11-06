//! Computes the embeddings of the keyword list (`models/keywords.txt`) into `models/keywords.bin`,
//! after the list changed: `keyword_vectors` (needs the text model of the content search).

fn main() -> anyhow::Result<()> {
    let encoder = sk_ai::content::TextEncoder::load()?;
    let file = sk_ai::content::keyword_vectors_file(&encoder)?;
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("models/keywords.bin");
    std::fs::write(&path, &file)?;
    println!("{} ({} Bytes)", path.display(), file.len());
    Ok(())
}
