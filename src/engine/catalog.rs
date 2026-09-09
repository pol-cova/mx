use super::mask::{MAX_SERVICES, ServiceMask};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::OnceLock};

#[derive(Debug, Deserialize, Serialize)]
pub struct Catalog {
    pub catalog_id: String,
    pub tested_runtime: String,
    pub categories: Vec<Category>,
    pub features: Vec<Feature>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Category {
    pub id: String,
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downside: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Feature {
    pub id: String,
    pub labels: Vec<String>,
}

pub struct Registry {
    catalog: Catalog,
    labels: Vec<String>,
    indexes: HashMap<String, usize>,
    categories: HashMap<String, ServiceMask>,
    features: HashMap<String, ServiceMask>,
    all: ServiceMask,
}

impl Registry {
    fn load() -> Self {
        let catalog: Catalog =
            serde_json::from_str(include_str!("../../data/mx-runtime-ios-26.5.json"))
                .expect("embedded Mx runtime catalog must be valid JSON");
        let mut labels: Vec<_> = catalog
            .categories
            .iter()
            .flat_map(|category| category.labels.iter().cloned())
            .collect();
        labels.sort();
        labels.dedup();
        assert!(
            labels.len() <= MAX_SERVICES,
            "Mx runtime catalog exceeds fixed service-mask capacity"
        );
        let indexes: HashMap<_, _> = labels
            .iter()
            .enumerate()
            .map(|(index, label)| (label.clone(), index))
            .collect();
        let mask = |services: &[String]| {
            let mut mask = ServiceMask::default();
            for label in services {
                mask.insert(
                    *indexes
                        .get(label)
                        .expect("capability references service outside Mx catalog"),
                );
            }
            mask
        };
        let categories = catalog
            .categories
            .iter()
            .map(|category| (category.id.clone(), mask(&category.labels)))
            .collect();
        let features = catalog
            .features
            .iter()
            .map(|feature| (feature.id.clone(), mask(&feature.labels)))
            .collect();
        let mut all = ServiceMask::default();
        for index in 0..labels.len() {
            all.insert(index);
        }
        Self {
            catalog,
            labels,
            indexes,
            categories,
            features,
            all,
        }
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    pub fn service_count(&self) -> usize {
        self.labels.len()
    }

    pub(super) fn all(&self) -> ServiceMask {
        self.all
    }

    pub(super) fn mask_for_labels(&self, labels: &[&str]) -> ServiceMask {
        let mut mask = ServiceMask::default();
        for label in labels {
            if let Some(index) = self.indexes.get(*label) {
                mask.insert(*index);
            }
        }
        mask
    }

    pub(super) fn retained(&self, keep: &[String]) -> anyhow::Result<ServiceMask> {
        let mut retained = ServiceMask::default();
        for capability in keep {
            if capability == "network" {
                continue;
            }
            let selected = self
                .categories
                .get(capability)
                .or_else(|| self.features.get(capability))
                .copied()
                .ok_or_else(|| anyhow::anyhow!("Unknown capability {capability}"))?;
            retained = retained.union(selected);
        }
        Ok(retained)
    }

    pub(super) fn labels(&self, mask: ServiceMask) -> Vec<String> {
        self.labels
            .iter()
            .enumerate()
            .filter(|(index, _)| mask.contains(*index))
            .map(|(_, label)| label.clone())
            .collect()
    }
}

pub fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(Registry::load)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_catalog_is_initialized_once_and_has_stable_ids() {
        let first = registry();
        let second = registry();
        assert!(std::ptr::eq(first, second));
        assert_eq!(first.catalog().catalog_id, "mx-runtime-ios-26.5-v1");
        assert_eq!(first.service_count(), 172);
    }
}
