//! The brain on its own: documents by source type, searched per source or
//! across all of them, and erased one source at a time.
//!
//! Run with:
//!
//! ```sh
//! cargo run -p tinymemory-tools --example brain
//! ```

use std::sync::Arc;

use tinymemory_api::conformance::ReferenceEngine;
use tinymemory_tools::{BackgroundJob, Brain, BrainDocument, BrainSource, MemoryLayout};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // A layout rooted below a team node keeps one tenant's brain apart.
    let layout = MemoryLayout::new("team:acme".parse()?)?;
    let brain = Brain::new(Arc::new(ReferenceEngine::new()), layout.clone());

    for (source, title, text) in [
        (
            BrainSource::Pdf,
            "pricing.pdf",
            "The Pro plan costs 20 dollars a month.",
        ),
        (
            BrainSource::Markdown,
            "deploys.md",
            "Deploys run on weekdays only.",
        ),
        (
            BrainSource::Notion,
            "Pricing FAQ",
            "Annual Pro plans get two months free.",
        ),
        (
            BrainSource::Github,
            "README",
            "Run cargo test before every pull request.",
        ),
    ] {
        let ingested = brain
            .ingest(BrainDocument::new(source.clone(), text).titled(title))
            .await?;
        println!(
            "{:<9} -> {}  (then: {})",
            source.to_string(),
            layout.brain(&source)?,
            ingested.job.as_ref().map_or("nothing", BackgroundJob::name)
        );
    }

    let everywhere = brain.search("pro plan pricing", None, 5).await?;
    println!(
        "\n'pro plan pricing' across the brain: {} hits",
        everywhere.len()
    );
    for hit in &everywhere {
        println!("  {} | {}", hit.meta.namespace, hit.text.replace('\n', " "));
    }

    let notion_only = brain
        .search("pro plan pricing", Some(&BrainSource::Notion), 5)
        .await?;
    println!("in notion only: {} hit", notion_only.len());

    let forgotten = brain.forget(&BrainSource::Pdf).await?;
    println!("\nforgot {} pdf document(s)", forgotten.forgotten);
    let left = brain.search("pro plan pricing", None, 5).await?;
    assert!(
        left.iter()
            .all(|hit| !hit.meta.namespace.to_string().contains("pdf"))
    );
    println!("{} hits remain across the brain", left.len());
    Ok(())
}
