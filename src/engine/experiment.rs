use super::{
    evidence::{Distribution, ExperimentManifest},
    probe::{Probe, ProbeResult, validate_required},
};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum State {
    BaselineBefore,
    CandidateIdle,
    CandidateActive,
    CandidatePostInteraction,
    BaselineRestored,
}

impl State {
    pub const ORDER: [Self; 5] = [
        Self::BaselineBefore,
        Self::CandidateIdle,
        Self::CandidateActive,
        Self::CandidatePostInteraction,
        Self::BaselineRestored,
    ];
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MemorySample {
    pub physical_bytes: u64,
    pub process_count: usize,
    pub compressor_bytes: u64,
    pub swap_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StateMeasurement {
    pub state: State,
    pub samples: Vec<MemorySample>,
}

impl StateMeasurement {
    pub fn physical_distribution(&self) -> Result<Distribution> {
        Distribution::from_values(
            &self
                .samples
                .iter()
                .map(|sample| sample.physical_bytes as f64)
                .collect::<Vec<_>>(),
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Cycle {
    pub index: usize,
    pub measurements: Vec<StateMeasurement>,
    pub probes: BTreeMap<Probe, ProbeResult>,
    pub boot_latency_ms: f64,
    pub launch_latency_ms: f64,
    pub action_latency_ms: f64,
}

impl Cycle {
    pub fn validate(&self, expected_samples: usize) -> Result<()> {
        if self.index == 0 {
            bail!("cycle indexes start at one");
        }
        let order = self
            .measurements
            .iter()
            .map(|measurement| measurement.state)
            .collect::<Vec<_>>();
        if order != State::ORDER {
            bail!(
                "cycle {} does not follow the required A/B/A state order",
                self.index
            );
        }
        for measurement in &self.measurements {
            if measurement.samples.len() != expected_samples {
                bail!(
                    "cycle {} {:?} has {} samples; expected {expected_samples}",
                    self.index,
                    measurement.state,
                    measurement.samples.len()
                );
            }
        }
        if [
            self.boot_latency_ms,
            self.launch_latency_ms,
            self.action_latency_ms,
        ]
        .iter()
        .any(|value| !value.is_finite() || *value < 0.0)
        {
            bail!("cycle latencies must be finite and non-negative");
        }
        validate_required(&self.probes)
    }

    pub fn restoration_delta(&self) -> Result<f64> {
        let before = self.measurements[0].physical_distribution()?.median;
        let restored = self.measurements[4].physical_distribution()?.median;
        if before == 0.0 {
            return Ok(if restored == 0.0 { 0.0 } else { f64::INFINITY });
        }
        Ok((restored - before).abs() / before)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExperimentRun {
    pub manifest: ExperimentManifest,
    pub cycles: Vec<Cycle>,
}

impl ExperimentRun {
    pub fn validate(&self) -> Result<()> {
        self.manifest.validate()?;
        if self.cycles.len() != self.manifest.cycle_count {
            bail!("experiment has an incomplete cycle set");
        }
        for (position, cycle) in self.cycles.iter().enumerate() {
            if cycle.index != position + 1 {
                bail!("experiment cycle indexes must be contiguous");
            }
            cycle.validate(self.manifest.sample_count)?;
            if cycle.restoration_delta()? > 0.05 {
                bail!(
                    "cycle {} restored baseline differs by more than 5%",
                    cycle.index
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::evidence::{ReferenceEnvironment, SCHEMA_VERSION};

    fn samples(value: u64) -> Vec<MemorySample> {
        (0..30)
            .map(|_| MemorySample {
                physical_bytes: value,
                process_count: 10,
                compressor_bytes: 0,
                swap_bytes: 0,
            })
            .collect()
    }

    fn probes() -> BTreeMap<Probe, ProbeResult> {
        Probe::REQUIRED
            .into_iter()
            .map(|probe| {
                (
                    probe,
                    ProbeResult {
                        passed: true,
                        detail: None,
                        artifact_ids: Vec::new(),
                    },
                )
            })
            .collect()
    }

    fn cycle(index: usize, restored: u64) -> Cycle {
        Cycle {
            index,
            measurements: State::ORDER
                .into_iter()
                .map(|state| StateMeasurement {
                    state,
                    samples: samples(if state == State::BaselineRestored {
                        restored
                    } else {
                        100
                    }),
                })
                .collect(),
            probes: probes(),
            boot_latency_ms: 1.0,
            launch_latency_ms: 1.0,
            action_latency_ms: 1.0,
        }
    }

    fn manifest() -> ExperimentManifest {
        ExperimentManifest {
            schema_version: SCHEMA_VERSION,
            experiment_id: "ram".into(),
            candidate_id: "candidate".into(),
            source_revision: "revision".into(),
            mx_sha256: "mx".into(),
            catalog_sha256: "catalog".into(),
            app_build_identity: "app".into(),
            task_script_sha256: "task".into(),
            sample_count: 30,
            cycle_count: 3,
            environment: ReferenceEnvironment {
                macos: "macOS".into(),
                architecture: "arm64".into(),
                xcode: "26.5".into(),
                runtime: "iOS 26.5".into(),
                device_type: "iPhone 17 Pro".into(),
            },
        }
    }

    #[test]
    fn accepts_complete_restorable_experiment() {
        ExperimentRun {
            manifest: manifest(),
            cycles: vec![cycle(1, 100), cycle(2, 103), cycle(3, 95)],
        }
        .validate()
        .unwrap();
    }

    #[test]
    fn rejects_out_of_order_and_incomplete_cycles() {
        let mut run = ExperimentRun {
            manifest: manifest(),
            cycles: vec![cycle(1, 100), cycle(2, 100)],
        };
        assert!(
            run.validate()
                .unwrap_err()
                .to_string()
                .contains("incomplete")
        );
        run.cycles.push(cycle(3, 100));
        run.cycles[0].measurements.swap(0, 1);
        assert!(
            run.validate()
                .unwrap_err()
                .to_string()
                .contains("state order")
        );
    }

    #[test]
    fn rejects_restoration_outside_five_percent() {
        let run = ExperimentRun {
            manifest: manifest(),
            cycles: vec![cycle(1, 106), cycle(2, 100), cycle(3, 100)],
        };
        assert!(
            run.validate()
                .unwrap_err()
                .to_string()
                .contains("more than 5%")
        );
    }
}
