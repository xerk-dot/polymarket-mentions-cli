use anyhow::Result;
use clap::{Args, Subcommand};
use polymarket_client_sdk::gamma::{self, types::request::SearchRequest};

use crate::output::{print_json, OutputFormat};

const MENTION_PATTERNS: &[&str] = &[
    "will trump say",
    "will biden say",
    "will elon say",
    "trump mention",
    "biden mention",
    "will say",
    "speech mention",
    "state of the union",
];

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
    unresolved: usize,
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
    for pattern in MENTION_PATTERNS {
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
                    seen_ids.insert(market.id.clone());
                    all_markets.push(market);
                }
            }
        }
    }

    // Categorize markets
    let mut resolved_yes = 0;
    let mut resolved_no = 0;
    let mut unresolved = 0;
    let mut no_markets = Vec::new();

    for market in &all_markets {
        if !market.closed.unwrap_or(false) {
            unresolved += 1;
            continue;
        }

        // Parse outcome prices to determine winner
        // Format: [YES_price, NO_price] as Vec<Decimal>
        // If YES_price is "1" or close to 1, YES won
        // If NO_price is "1" or close to 1, NO won
        if let Some(prices) = &market.outcome_prices {
            if prices.len() >= 2 {
                use rust_decimal::prelude::ToPrimitive;
                let yes_price: f64 = prices[0].to_f64().unwrap_or(0.0);
                let no_price: f64 = prices[1].to_f64().unwrap_or(0.0);

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
                    unresolved += 1;
                }
                continue;
            }
        }
        unresolved += 1;
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
        unresolved,
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
            println!("Unresolved:   {}", result.unresolved);
            println!("\nNO win rate: {:.1}%", result.no_percentage);

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
