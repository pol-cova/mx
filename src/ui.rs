use crate::native;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Screen {
    pub device: String,
    pub pid: Option<u32>,
    #[serde(default)]
    pub width: f64,
    #[serde(default)]
    pub height: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    pub elements: Vec<Element>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Element {
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<u32>,
    #[serde(default, skip_serializing)]
    pub frame: Option<[f64; 4]>,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Selector {
    /// Accessibility identifier from mx_ui. Provide exactly one of identifier or label.
    pub identifier: Option<String>,
    /// Exact accessibility label. Ambiguous matches are rejected.
    pub label: Option<String>,
    /// Optional accessibility type such as Button or TextField.
    pub role: Option<String>,
}
impl Selector {
    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.identifier.is_some() != self.label.is_some(),
            "Provide exactly one of identifier or label"
        );
        for value in [&self.identifier, &self.label, &self.role]
            .into_iter()
            .flatten()
        {
            anyhow::ensure!(!value.trim().is_empty(), "UI selectors cannot be empty");
        }
        Ok(())
    }
    pub fn matches(&self, element: &Element) -> bool {
        self.identifier
            .as_ref()
            .is_none_or(|s| element.identifier.as_ref() == Some(s))
            && self
                .label
                .as_ref()
                .is_none_or(|s| element.label.as_ref() == Some(s))
            && self.role.as_ref().is_none_or(|s| &element.role == s)
    }
}

fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
        Value::Number(_) | Value::Bool(_) => Some(value.to_string()),
        _ => None,
    }
}

pub fn parse_elements(json: &str) -> Result<Vec<Element>> {
    let roots: Value =
        serde_json::from_str(json).context("Guest returned invalid accessibility JSON")?;
    let roots = roots
        .as_array()
        .context("Accessibility response must be an array")?;
    fn visit(node: &Value, result: &mut Vec<Element>) -> Result<()> {
        anyhow::ensure!(node.is_object(), "Accessibility element must be an object");
        let label = scalar(&node["AXLabel"]).or_else(|| scalar(&node["label"]));
        let identifier = scalar(&node["AXUniqueId"])
            .or_else(|| scalar(&node["AXIdentifier"]))
            .or_else(|| scalar(&node["identifier"]));
        let value = scalar(&node["AXValue"]).or_else(|| scalar(&node["value"]));
        if label.is_some() || identifier.is_some() || value.is_some() {
            result.push(Element {
                reference: None,
                role: scalar(&node["type"])
                    .or_else(|| scalar(&node["role"]))
                    .unwrap_or_else(|| "Unknown".into()),
                label,
                identifier,
                value,
                index: None,
                frame: None,
            });
        }
        if let Some(children) = node.get("children").filter(|v| !v.is_null()) {
            for child in children.as_array().context("children must be an array")? {
                visit(child, result)?;
            }
        }
        Ok(())
    }
    let mut elements = Vec::new();
    for root in roots {
        visit(root, &mut elements)?;
    }
    Ok(elements)
}

pub async fn dimensions(device: &str) -> Result<(f64, f64)> {
    native::dimensions(device).await
}

pub async fn inspect(device: &str) -> Result<Screen> {
    native::inspect(device).await
}

pub async fn tap(device: &str, selector: Selector) -> Result<()> {
    let screen = inspect(device).await?;
    tap_on_screen(&screen, selector).await
}

pub async fn tap_on_screen(screen: &Screen, selector: Selector) -> Result<()> {
    native::tap_on_screen(screen, selector).await
}

pub async fn type_text(device: &str, text: &str) -> Result<()> {
    native::type_text(device, text).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compacts_hierarchy_and_preserves_scalar_values() {
        let elements = parse_elements(r#"[{"type":"Application","children":[{"type":"Switch","AXLabel":"Enabled","AXValue":true,"AXUniqueId":"toggle"},{"type":"TextField","AXLabel":"Name","AXValue":"Paul","frame":{"x":1}}]}]"#).unwrap();
        assert_eq!(elements.len(), 2);
        assert_eq!(elements[0].value.as_deref(), Some("true"));
        assert_eq!(elements[1].label.as_deref(), Some("Name"));
        assert!(!serde_json::to_string(&elements).unwrap().contains("frame"));
    }
    #[test]
    fn malformed_hierarchies_fail_instead_of_returning_empty_success() {
        assert!(parse_elements("{}").is_err());
        assert!(parse_elements(r#"[{"children":5}]"#).is_err());
    }
    #[test]
    fn selectors_require_one_target() {
        assert!(
            Selector {
                identifier: None,
                label: None,
                role: None
            }
            .validate()
            .is_err()
        );
        assert!(
            Selector {
                identifier: Some("x".into()),
                label: Some("y".into()),
                role: None
            }
            .validate()
            .is_err()
        );
    }
}
