use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReferenceEnvironment {
    pub macos: String,
    pub architecture: String,
    pub xcode: String,
    pub runtime: String,
    pub device_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExperimentManifest {
    pub schema_version: u32,
    pub experiment_id: String,
    pub candidate_id: String,
    pub source_revision: String,
    pub mx_sha256: String,
    pub catalog_sha256: String,
    pub app_build_identity: String,
    pub task_script_sha256: String,
    pub sample_count: usize,
    pub cycle_count: usize,
    pub environment: ReferenceEnvironment,
}

impl ExperimentManifest {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != SCHEMA_VERSION {
            bail!(
                "unsupported experiment schema version {}",
                self.schema_version
            );
        }
        for (name, value) in [
            ("experiment_id", self.experiment_id.as_str()),
            ("candidate_id", self.candidate_id.as_str()),
            ("source_revision", self.source_revision.as_str()),
            ("mx_sha256", self.mx_sha256.as_str()),
            ("catalog_sha256", self.catalog_sha256.as_str()),
            ("app_build_identity", self.app_build_identity.as_str()),
            ("task_script_sha256", self.task_script_sha256.as_str()),
        ] {
            if value.trim().is_empty() {
                bail!("{name} must not be empty");
            }
        }
        if self.sample_count != 30 {
            bail!("the measurement contract requires 30 samples per state");
        }
        if self.cycle_count != 3 {
            bail!("the measurement contract requires three A/B/A cycles");
        }
        Ok(())
    }

    pub fn matches_environment(&self, actual: &ReferenceEnvironment) -> Result<()> {
        if &self.environment != actual {
            bail!("reference environment does not match the experiment manifest");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Distribution {
    pub count: usize,
    pub minimum: f64,
    pub median: f64,
    pub p95_nearest_rank: f64,
    pub maximum: f64,
}

impl Distribution {
    pub fn from_values(values: &[f64]) -> Result<Self> {
        if values.is_empty() {
            bail!("cannot summarize an empty sample set");
        }
        if values.iter().any(|value| !value.is_finite()) {
            bail!("sample values must be finite");
        }
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        let count = sorted.len();
        let median = if count.is_multiple_of(2) {
            (sorted[count / 2 - 1] + sorted[count / 2]) / 2.0
        } else {
            sorted[count / 2]
        };
        let p95_index = (count * 95).div_ceil(100).saturating_sub(1);
        Ok(Self {
            count,
            minimum: sorted[0],
            median,
            p95_nearest_rank: sorted[p95_index],
            maximum: sorted[count - 1],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_nearest_rank_for_p95_and_standard_median() {
        let values = (1..=30).map(f64::from).collect::<Vec<_>>();
        let summary = Distribution::from_values(&values).unwrap();
        assert_eq!(summary.median, 15.5);
        assert_eq!(summary.p95_nearest_rank, 29.0);
    }

    #[test]
    fn rejects_non_finite_samples() {
        assert!(Distribution::from_values(&[f64::NAN]).is_err());
    }
}
