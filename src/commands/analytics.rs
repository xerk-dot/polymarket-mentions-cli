use anyhow::Result;
use clap::{Args, Subcommand};
use polymarket_client_sdk::gamma::{self, types::request::SearchRequest};

use crate::output::{print_json, OutputFormat};

// Broad search to find potential mentions markets
const SEARCH_PATTERNS: &[&str] = &[
    "will say",
    "will post",
    "be said",
    "say \"",
    "post \"",
];

// Filter: mentions markets - "Will X say/post Y" or "What will be said"
fn is_mentions_market(question: &str) -> bool {
    let q = question.to_lowercase();

    // Must have quotes (the word/phrase being predicted)
    if !q.contains("\"") {
        return false;
    }

    // Find first quote position
    let quote_pos = match q.find("\"") {
        Some(pos) => pos,
        None => return false,
    };

    // Check for "say", "post", or "said" BEFORE the quote
    let verbs = [" say ", " post ", " said "];
    for verb in verbs {
        if let Some(verb_pos) = q.find(verb) {
            if verb_pos < quote_pos {
                return true;
            }
        }
    }

    false
}

#[derive(Args)]
pub struct AnalyticsArgs {
    #[command(subcommand)]
    pub command: AnalyticsCommand,
}

#[derive(Subcommand)]
pub enum AnalyticsCommand {
    /// Analyze mentions markets that resolved to "No"
    MentionsNo {
        /// Max results per search pattern
        #[arg(long, default_value = "50")]
        limit: i32,
    },
}

#[derive(serde::Serialize)]
struct MentionsNoResult {
    total_mentions_markets: usize,
    resolved_yes: usize,
    resolved_no: usize,
    still_open: usize,
    no_percentage: f64,
    markets_resolved_no: Vec<MarketSummary>,
}

#[derive(serde::Serialize)]
struct MarketSummary {
    question: String,
    volume: String,
    closed_time: Option<String>,
}


pub async fn execute(
    client: &gamma::Client,
    args: AnalyticsArgs,
    output: OutputFormat,
) -> Result<()> {
    match args.command {
        AnalyticsCommand::MentionsNo { limit } => {
            mentions_no(client, limit, output).await
        }
    }
}

async fn mentions_no(client: &gamma::Client, limit: i32, output: OutputFormat) -> Result<()> {
    let mut all_markets = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();

    // Search with each pattern
    for pattern in SEARCH_PATTERNS {
        let request = SearchRequest::builder()
            .q(pattern.to_string())
            .limit_per_type(limit)
            .build();

        if let Ok(results) = client.search(&request).await {
            let markets = results
                .events
                .unwrap_or_default()
                .into_iter()
                .flat_map(|e| e.markets.unwrap_or_default());

            for market in markets {
                if !seen_ids.contains(&market.id) {
                    // Filter to actual mentions markets
                    let question = market.question.clone().unwrap_or_default();
                    if is_mentions_market(&question) {
                        seen_ids.insert(market.id.clone());
                        all_markets.push(market);
                    }
                }
            }
        }
    }

    // Categorize markets
    let mut resolved_yes = 0;
    let mut resolved_no = 0;
    let mut still_open = 0;
    let mut no_markets = Vec::new();

    for market in &all_markets {
        if !market.closed.unwrap_or(false) {
            still_open += 1;
            continue;
        }

        // Serialize to JSON and re-parse to get raw outcomePrices string
        let json = serde_json::to_value(market).unwrap_or_default();
        let prices_str = json.get("outcomePrices")
            .and_then(|v| v.as_str());

        if let Some(prices_str) = prices_str {
            if let Ok(prices) = serde_json::from_str::<Vec<String>>(prices_str) {
                if prices.len() >= 2 {
                    let yes_price: f64 = prices[0].parse().unwrap_or(0.0);
                    let no_price: f64 = prices[1].parse().unwrap_or(0.0);

                    if no_price > 0.99 {
                        resolved_no += 1;
                        no_markets.push(MarketSummary {
                            question: market.question.clone().unwrap_or_default(),
                            volume: market.volume.map(|v| v.to_string()).unwrap_or_default(),
                            closed_time: market.closed_time.clone(),
                        });
                    } else if yes_price > 0.99 {
                        resolved_yes += 1;
                    } else {
                        still_open += 1;
                    }
                    continue;
                }
            }
        }
        still_open += 1;
    }

    let total_resolved = resolved_yes + resolved_no;
    let no_percentage = if total_resolved > 0 {
        (resolved_no as f64 / total_resolved as f64) * 100.0
    } else {
        0.0
    };

    let result = MentionsNoResult {
        total_mentions_markets: all_markets.len(),
        resolved_yes,
        resolved_no,
        still_open,
        no_percentage,
        markets_resolved_no: no_markets,
    };

    match output {
        OutputFormat::Json => print_json(&result)?,
        OutputFormat::Table => {
            println!("=== Mentions Markets Analysis ===\n");
            println!("Total mentions markets found: {}", result.total_mentions_markets);
            println!("Resolved YES: {}", result.resolved_yes);
            println!("Resolved NO:  {}", result.resolved_no);
            println!("Still open:   {}", result.still_open);
            println!("\nNO win rate (of resolved): {:.1}%", result.no_percentage);

            if !result.markets_resolved_no.is_empty() {
                println!("\n--- Markets that resolved NO ---\n");
                for (i, m) in result.markets_resolved_no.iter().enumerate() {
                    println!("{}. {}", i + 1, m.question);
                    println!("   Volume: ${}", m.volume);
                    if let Some(t) = &m.closed_time {
                        println!("   Closed: {}", t);
                    }
                    println!();
                }
            }
        }
    }

    Ok(())
}
