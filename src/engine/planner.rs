use super::catalog::registry;

const BALANCED_SERVICES: &[&str] = &[
    "com.apple.assistantd",
    "com.apple.corespeechd",
    "com.apple.searchd",
    "com.apple.searchtoold",
    "com.apple.assetsd",
    "com.apple.photoanalysisd",
    "com.apple.cloudd",
    "com.apple.healthd",
    "com.apple.homed",
    "com.apple.chronod",
    "com.apple.liveactivitiesd",
];

#[derive(Clone, Copy)]
pub enum Policy {
    Balanced,
    Optimized,
}

pub fn plan(keep: &[String], policy: Policy) -> anyhow::Result<Vec<String>> {
    let registry = registry();
    let candidates = match policy {
        Policy::Balanced => registry.mask_for_labels(BALANCED_SERVICES),
        Policy::Optimized => registry.all(),
    };
    Ok(registry.labels(candidates.difference(registry.retained(keep)?)))
}

pub fn retained_labels(keep: &[String]) -> anyhow::Result<Vec<String>> {
    let registry = registry();
    Ok(registry.labels(registry.retained(keep)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optimized_policy_uses_fixed_masks_for_capabilities() {
        let plan = plan(&["push".into(), "photos".into()], Policy::Optimized).unwrap();
        assert_eq!(plan.len(), 163);
        assert!(!plan.iter().any(|label| matches!(
            label.as_str(),
            "com.apple.apsd" | "com.apple.assetsd" | "com.apple.photoanalysisd"
        )));
    }
}
