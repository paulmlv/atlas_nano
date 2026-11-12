//! API-equivalent prices of recorded model requests, from the built-in `prices.toml`.

use std::collections::HashMap;

use rust_decimal::Decimal;
use serde::Deserialize;

use crate::model::{Client, Usage};

const PRICES: &str = include_str!("prices.toml");

/// USD per million tokens of each billed category. A missing cache-write rate falls back to the
/// input rate, and a missing one-hour rate to the five-minute one.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Rates {
    input: Decimal,
    cached: Decimal,
    cache_write: Option<Decimal>,
    cache_write_1h: Option<Decimal>,
    output: Decimal,
}

impl Rates {
    fn cost(&self, usage: &Usage) -> Decimal {
        let cache_write = self.cache_write.unwrap_or(self.input);
        let tokens = Decimal::from(usage.input) * self.input
            + Decimal::from(usage.cached) * self.cached
            + Decimal::from(usage.cache_write) * cache_write
            + Decimal::from(usage.cache_write_1h) * self.cache_write_1h.unwrap_or(cache_write)
            + Decimal::from(usage.output) * self.output;
        tokens / Decimal::from(1_000_000)
    }
}

/// A model's rates up to and above its long-context boundary on total input tokens.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelPrice {
    long_context_above: u64,
    short: Rates,
    long: Option<Rates>,
}

/// Rates by `client/model`, and the date they were checked.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriceTable {
    models: HashMap<String, ModelPrice>,
    #[serde(default)]
    priced_as: HashMap<String, String>,
    pub checked: toml::value::Datetime,
}

impl PriceTable {
    /// The price table built into the binary.
    pub fn built_in() -> PriceTable {
        PriceTable::decode(PRICES).expect("the built-in price table is valid")
    }

    fn decode(text: &str) -> Result<PriceTable, String> {
        let table: PriceTable = toml::from_str(text).map_err(|error| error.to_string())?;
        if let Some(target) = table
            .priced_as
            .values()
            .find(|t| !table.models.contains_key(*t))
        {
            return Err(format!("priced_as names `{target}`, which has no rates"));
        }
        Ok(table)
    }

    /// Return one request's cost, or None when the table cannot price it.
    ///
    /// Fast mode, a model without rates, and a request above a boundary without long rates are
    /// unpriced. A request that reports no tokens costs nothing, whatever its model.
    pub fn cost(&self, client: Client, usage: &Usage) -> Option<Decimal> {
        if usage.tokens() == 0 {
            return Some(Decimal::ZERO);
        }
        let name = format!("{}/{}", client.name(), usage.model);
        let price = self
            .models
            .get(self.priced_as.get(&name).unwrap_or(&name))?;
        if usage.fast {
            return None;
        }
        let rates = if usage.prompt() <= price.long_context_above {
            Some(price.short)
        } else {
            price.long
        };
        rates.map(|rates| rates.cost(usage))
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr as _;

    use chrono::Utc;

    use super::*;

    fn usage(model: &str, input: u64) -> Usage {
        Usage {
            id: String::new(),
            owner: None,
            at: Utc::now(),
            model: model.to_string(),
            fast: false,
            input,
            cached: 1_000_000,
            cache_write: 1_000_000,
            cache_write_1h: 1_000_000,
            output: 1_000_000,
        }
    }

    const TABLE: &str = r#"
        checked = 2026-01-01
        [priced_as]
        "codex/alias" = "codex/plain"
        [models."codex/plain"]
        long_context_above = 5_000_000
        short = { input = 2.00, cached = 0.10, output = 10.00 }
        [models."claude/tiered"]
        long_context_above = 5_000_000
        short = { input = 1.00, cached = 0.10, cache_write = 1.25, cache_write_1h = 2.00, output = 5.00 }
        long = { input = 2.00, cached = 0.20, cache_write = 2.50, cache_write_1h = 4.00, output = 7.50 }
    "#;

    #[test]
    fn prices_each_category_at_its_own_rate() {
        let table = PriceTable::decode(TABLE).unwrap();
        let cost = |client, model, input| table.cost(client, &usage(model, input));
        // 1.00 + 0.10 + 1.25 + 2.00 + 5.00
        assert_eq!(
            cost(Client::Claude, "tiered", 1_000_000),
            Decimal::from_str("9.35").ok()
        );
        // Above the boundary every token takes the long rates: 6.00 + 0.20 + 2.50 + 4.00 + 7.50.
        assert_eq!(
            cost(Client::Claude, "tiered", 3_000_000),
            Decimal::from_str("20.20").ok()
        );
        // Missing cache-write rates fall back to the input rate: 2.00 + 0.10 + 2.00 + 2.00 + 10.00.
        assert_eq!(
            cost(Client::Codex, "alias", 1_000_000),
            Decimal::from_str("16.10").ok()
        );
        assert_eq!(table.checked.to_string(), "2026-01-01");
    }

    #[test]
    fn never_guesses_a_price() {
        let table = PriceTable::decode(TABLE).unwrap();
        assert_eq!(table.cost(Client::Claude, &usage("unknown", 1)), None);
        assert_eq!(table.cost(Client::Codex, &usage("plain", 3_000_000)), None);
        let fast = Usage {
            fast: true,
            ..usage("tiered", 1)
        };
        assert_eq!(table.cost(Client::Claude, &fast), None);
        let empty = Usage {
            cached: 0,
            cache_write: 0,
            cache_write_1h: 0,
            output: 0,
            ..usage("unknown", 0)
        };
        assert_eq!(table.cost(Client::Claude, &empty), Some(Decimal::ZERO));
    }

    #[test]
    fn rejects_unknown_fields_and_dangling_aliases() {
        assert!(PriceTable::decode("checked = 2026-01-01\n[extra]").is_err());
        let alias = "checked = 2026-01-01\n[priced_as]\n\"a/b\" = \"a/missing\"";
        assert!(PriceTable::decode(alias).is_err());
        let rate = TABLE.replace(
            "cached = 0.10, output = 10.00",
            "cached = 0.10, bonus = 1.0, output = 10.00",
        );
        assert!(PriceTable::decode(&rate).is_err());
    }

    #[test]
    fn the_built_in_table_prices_terra_and_long_gpt_5_4_requests() {
        let table = PriceTable::built_in();
        for (model, input, cache_write, expected) in [
            ("gpt-6.1-terra", 122_000, 50_000, "0.2395"),
            ("gpt-6.1-terra", 122_001, 50_000, "0.454002"),
            ("gpt-5.6-terra", 122_000, 50_000, "0.509"),
            ("gpt-5.6-terra", 122_001, 50_000, "0.958004"),
            ("gpt-5.4", 172_000, 0, "0.605"),
            ("gpt-5.4", 172_001, 0, "1.135005"),
            ("codex-auto-review", 172_001, 0, "1.135005"),
        ] {
            let request = Usage {
                cached: 100_000,
                cache_write,
                cache_write_1h: 0,
                output: 10_000,
                ..usage(model, input)
            };
            assert_eq!(
                table.cost(Client::Codex, &request),
                Some(Decimal::from_str(expected).unwrap()),
                "{model} with {} prompt tokens",
                request.prompt()
            );
        }
    }

    #[test]
    fn the_built_in_table_prices_the_current_claude_models() {
        let table = PriceTable::built_in();
        // 100,000 tokens each of input, cached, five-minute write, one-hour write, and output.
        let cost = |model| {
            let each = 100_000;
            let usage = Usage {
                cached: each,
                cache_write: each,
                cache_write_1h: each,
                output: each,
                ..usage(model, each)
            };
            table.cost(Client::Claude, &usage)
        };
        assert_eq!(cost("claude-fable-5-2"), Decimal::from_str("7.42").ok());
        assert_eq!(cost("claude-sonnet-5-5"), Decimal::from_str("1.86").ok());
    }
}
