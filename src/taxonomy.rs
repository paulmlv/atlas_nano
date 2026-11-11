//! The label lists and rules, read from the taxonomy page built into the binary.

use sha2::{Digest, Sha256};

const PAGE: &str = include_str!("../docs/TAXONOMY.md");

/// One label with the group or domain it rolls up to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub group: String,
    pub label: String,
    pub definition: String,
}

/// The labeling rules, the two label lists, and the pull request rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Taxonomy {
    pub rules: String,
    pub work: Vec<Entry>,
    pub topics: Vec<Entry>,
    pub pull_request_rules: String,
}

/// Return the body of the section whose heading starts with `## <title>`.
fn section<'a>(page: &'a str, title: &str) -> Result<&'a str, String> {
    let heading = format!("\n## {title}");
    let start = page
        .find(&heading)
        .ok_or(format!("the taxonomy has no `{title}` section"))?;
    let body = &page[start + heading.len()..];
    let body = &body[body.find('\n').unwrap_or(body.len())..];
    Ok(body[..body.find("\n## ").unwrap_or(body.len())].trim())
}

/// Read a three-column Markdown table of group, label, and definition.
fn entries(table: &str) -> Result<Vec<Entry>, String> {
    let mut entries = Vec::new();
    for line in table.lines().filter(|line| line.starts_with('|')).skip(2) {
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        let [group, label, definition] = cells[..] else {
            return Err(format!(
                "a taxonomy row does not have three columns: {line}"
            ));
        };
        entries.push(Entry {
            group: group.to_string(),
            label: label.to_string(),
            definition: definition.to_string(),
        });
    }
    if entries.is_empty() {
        Err("a taxonomy table has no rows".to_string())
    } else {
        Ok(entries)
    }
}

impl Taxonomy {
    /// The taxonomy built into the binary.
    pub fn built_in() -> Taxonomy {
        Taxonomy::decode(PAGE).expect("the built-in taxonomy is valid")
    }

    fn decode(page: &str) -> Result<Taxonomy, String> {
        Ok(Taxonomy {
            rules: section(page, "Rules")?.to_string(),
            work: entries(section(page, "Type of work")?)?,
            topics: entries(section(page, "Topic")?)?,
            pull_request_rules: section(page, "Pull request mapping")?.to_string(),
        })
    }

    /// A digest of everything a label depends on, so a changed taxonomy invalidates stored labels.
    pub fn version(&self) -> String {
        let mut digest = Sha256::new();
        for part in [&self.rules, &self.pull_request_rules] {
            digest.update(part.as_bytes());
        }
        for entry in self.work.iter().chain(&self.topics) {
            digest.update(format!(
                "{}|{}|{}\n",
                entry.group, entry.label, entry.definition
            ));
        }
        digest
            .finalize()
            .iter()
            .take(8)
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// The group of a type of work, or the domain of a topic.
    pub fn group<'a>(entries: &'a [Entry], label: &str) -> Option<&'a str> {
        entries
            .iter()
            .find(|entry| entry.label == label)
            .map(|entry| entry.group.as_str())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn the_built_in_taxonomy_matches_its_headings() {
        let taxonomy = Taxonomy::built_in();
        let count = |title: &str| -> usize {
            let heading = PAGE
                .lines()
                .find(|line| line.starts_with(&format!("## {title}")))
                .unwrap();
            let digits: String = heading
                .rsplit(' ')
                .nth(1)
                .unwrap()
                .chars()
                .filter(char::is_ascii_digit)
                .collect();
            digits.parse().unwrap()
        };
        assert_eq!(taxonomy.work.len(), count("Type of work"));
        assert_eq!(taxonomy.topics.len(), count("Topic"));
        for entries in [&taxonomy.work, &taxonomy.topics] {
            let labels: HashSet<&str> = entries.iter().map(|entry| entry.label.as_str()).collect();
            assert_eq!(labels.len(), entries.len(), "labels are unique");
            assert!(entries.iter().all(|entry| !entry.definition.is_empty()));
        }
        assert!(taxonomy.rules.starts_with("- "));
        assert!(taxonomy.pull_request_rules.contains("Contributed to"));
        assert_eq!(
            Taxonomy::group(&taxonomy.work, "Debugging"),
            Some("Software")
        );
        assert_eq!(taxonomy.version().len(), 16);
    }

    #[test]
    fn a_changed_definition_changes_the_version() {
        let page = "\n## Rules\n\n- One rule.\n\n## Type of work (1 labels)\n\n\
            | Group | Label | Definition |\n| --- | --- | --- |\n| G | L | First |\n\n\
            ## Topic (1 topics)\n\n| Domain | Topic | Definition |\n| --- | --- | --- |\n| D | T | X |\n\n\
            ## Pull request mapping (experimental)\n\n- Rule.\n";
        let first = Taxonomy::decode(page).unwrap();
        let second = Taxonomy::decode(&page.replace("First", "Second")).unwrap();
        assert_eq!(first.work[0].label, "L");
        assert_ne!(first.version(), second.version());
        assert!(Taxonomy::decode("\n## Rules\n\n- One rule.\n").is_err());
    }
}
