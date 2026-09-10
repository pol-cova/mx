use crate::process::{self, strings};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Screen {
    pub device: String,
    pub pid: Option<u32>,
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
fn resolve_bridge(override_path: Option<String>, home: Option<std::path::PathBuf>) -> String {
    if let Some(path) = override_path {
        return path;
    }
    if let Some(home) = home {
        let path = home.join("Library/Application Support/Mx/tools/axe-1.8.0/axe");
        use std::os::unix::fs::PermissionsExt;
        if path
            .metadata()
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        {
            return path.to_string_lossy().into_owned();
        }
    }
    "axe".into()
}

pub fn bridge_path() -> String {
    resolve_bridge(
        std::env::var("MX_AXE_PATH").ok(),
        std::env::var_os("HOME").map(std::path::PathBuf::from),
    )
}

pub(crate) async fn batch_steps(device: &str, steps: &[String]) -> Result<()> {
    let mut args = strings(&["batch", "--udid", device, "--ax-cache", "perStep"]);
    for step in steps {
        args.push("--step".into());
        args.push(step.clone());
    }
    process::output(&bridge_path(), &args).await?;
    Ok(())
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
        serde_json::from_str(json).context("AXe returned invalid accessibility JSON")?;
    let roots = roots
        .as_array()
        .context("AXe accessibility response must be an array")?;
    fn visit(node: &Value, result: &mut Vec<Element>) -> Result<()> {
        anyhow::ensure!(
            node.is_object(),
            "AXe accessibility element must be an object"
        );
        let label = scalar(&node["AXLabel"]);
        let identifier = scalar(&node["AXUniqueId"]).or_else(|| scalar(&node["AXIdentifier"]));
        let value = scalar(&node["AXValue"]);
        if label.is_some() || identifier.is_some() || value.is_some() {
            result.push(Element {
                reference: None,
                role: scalar(&node["type"])
                    .or_else(|| scalar(&node["role"]))
                    .unwrap_or_else(|| "Unknown".into()),
                label,
                identifier,
                value,
            });
        }
        if let Some(children) = node.get("children").filter(|v| !v.is_null()) {
            for child in children
                .as_array()
                .context("AXe children must be an array")?
            {
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

async fn describe(device: &str) -> Result<String> {
    process::output(
        &bridge_path(),
        &strings(&["describe-ui", "--udid", device]),
    )
    .await
    .context("UI inspection requires AXe. Run sh scripts/setup-axe.sh from the Mx checkout, install AXe on PATH, or set MX_AXE_PATH")
}

pub async fn dimensions(device: &str) -> Result<(f64, f64)> {
    let raw: Value = serde_json::from_str(&describe(device).await?)?;
    let frame = raw
        .as_array()
        .and_then(|roots| roots.first())
        .and_then(|root| root.get("frame"))
        .context("AXe UI root has no frame")?;
    let width = frame["width"].as_f64().context("AXe frame has no width")?;
    let height = frame["height"]
        .as_f64()
        .context("AXe frame has no height")?;
    anyhow::ensure!(
        width > 0.0 && height > 0.0,
        "AXe returned invalid screen dimensions"
    );
    Ok((width, height))
}

pub async fn inspect(device: &str) -> Result<Screen> {
    let raw = describe(device).await?;
    Ok(Screen {
        device: device.into(),
        pid: serde_json::from_str::<Value>(&raw)?
            .as_array()
            .and_then(|a| a.first())
            .and_then(|n| n["pid"].as_u64())
            .and_then(|p| u32::try_from(p).ok()),
        elements: parse_elements(&raw)?,
    })
}
pub async fn tap(device: &str, selector: Selector) -> Result<()> {
    let screen = inspect(device).await?;
    tap_on_screen(&screen, selector).await
}
pub async fn tap_on_screen(screen: &Screen, selector: Selector) -> Result<()> {
    selector.validate()?;
    let device = &screen.device;
    let matches = screen
        .elements
        .iter()
        .filter(|e| selector.matches(e))
        .count();
    if matches != 1 {
        bail!(
            "UI selector matched {matches} elements; inspect again and use a unique identifier or role"
        );
    }
    let mut args = strings(&["tap", "--udid", device]);
    if let Some(id) = selector.identifier {
        args.push(format!("--id={id}"));
    }
    if let Some(label) = selector.label {
        args.push(format!("--label={label}"));
    }
    if let Some(role) = selector.role {
        args.push(format!("--element-type={role}"));
    }
    process::output(&bridge_path(), &args).await?;
    Ok(())
}
pub async fn type_text(device: &str, text: &str) -> Result<()> {
    anyhow::ensure!(
        text.len() <= 16 * 1024,
        "Text input is limited to 16 KiB per call"
    );
    anyhow::ensure!(
        text.chars()
            .all(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t')),
        "Text contains unsupported control characters"
    );
    if !text.is_ascii() {
        process::input(
            "xcrun",
            &strings(&["simctl", "pbcopy", device]),
            text.as_bytes(),
        )
        .await?;
        process::output(
            &bridge_path(),
            &strings(&[
                "key-combo",
                "--modifiers",
                "227",
                "--key",
                "25",
                "--udid",
                device,
            ]),
        )
        .await?;
        return Ok(());
    }
    process::input(
        &bridge_path(),
        &strings(&["type", "--stdin", "--udid", device]),
        text.as_bytes(),
    )
    .await
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

#[cfg(test)]
mod bridge_discovery_tests {
    use super::resolve_bridge;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn discovers_user_install_without_cwd_or_path_and_preserves_override() {
        let home = tempfile::tempdir().unwrap();
        let path = home
            .path()
            .join("Library/Application Support/Mx/tools/axe-1.8.0/axe");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(resolve_bridge(None, Some(home.path().into())), "axe");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            resolve_bridge(None, Some(home.path().into())),
            path.to_str().unwrap()
        );
        assert_eq!(
            resolve_bridge(Some("/custom/axe".into()), Some(home.path().into())),
            "/custom/axe"
        );
        assert_eq!(resolve_bridge(None, None), "axe");
    }
}
