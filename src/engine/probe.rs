use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Probe {
    Build,
    Install,
    Launch,
    SemanticInspection,
    UnicodeInput,
    Assertion,
    Screenshot,
    Logs,
    Networking,
}

impl Probe {
    pub const REQUIRED: [Self; 9] = [
        Self::Build,
        Self::Install,
        Self::Launch,
        Self::SemanticInspection,
        Self::UnicodeInput,
        Self::Assertion,
        Self::Screenshot,
        Self::Logs,
        Self::Networking,
    ];
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProbeResult {
    pub passed: bool,
    pub detail: Option<String>,
    pub artifact_ids: Vec<String>,
}

pub fn validate_required(results: &BTreeMap<Probe, ProbeResult>) -> Result<()> {
    for probe in Probe::REQUIRED {
        let Some(result) = results.get(&probe) else {
            bail!("missing required {probe:?} probe");
        };
        if !result.passed {
            bail!("required {probe:?} probe failed");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_every_probe_to_pass() {
        let mut results = BTreeMap::new();
        for probe in Probe::REQUIRED {
            results.insert(
                probe,
                ProbeResult {
                    passed: true,
                    detail: None,
                    artifact_ids: Vec::new(),
                },
            );
        }
        validate_required(&results).unwrap();
        results.remove(&Probe::Networking);
        assert!(
            validate_required(&results)
                .unwrap_err()
                .to_string()
                .contains("Networking")
        );
    }
}
