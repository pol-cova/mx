use crate::process::{output, strings};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub fn discover(path: &Path) -> Result<PathBuf> {
    let path = path.canonicalize().context("Project path does not exist")?;
    anyhow::ensure!(path.is_dir(), "Xcode containers must be directories");
    if matches!(
        path.extension().and_then(|s| s.to_str()),
        Some("xcodeproj" | "xcworkspace")
    ) {
        return Ok(path);
    }
    let mut projects = Vec::new();
    let mut workspaces = Vec::new();
    for entry in std::fs::read_dir(&path)? {
        let entry = entry?.path();
        if !entry.is_dir() {
            continue;
        }
        match entry.extension().and_then(|s| s.to_str()) {
            Some("xcodeproj") => projects.push(entry),
            Some("xcworkspace") => workspaces.push(entry),
            _ => {}
        }
    }
    let candidates = if workspaces.is_empty() {
        projects
    } else {
        workspaces
    };
    match candidates.as_slice() {
        [project] => Ok(project.clone()),
        [] => bail!(
            "No Xcode project in {}; pass --project <path>",
            path.display()
        ),
        _ => bail!("Multiple Xcode projects found; pass --project <path>"),
    }
}
pub fn project_args(project: &Path) -> Vec<String> {
    vec![
        if project.extension().is_some_and(|s| s == "xcworkspace") {
            "-workspace"
        } else {
            "-project"
        }
        .into(),
        project.to_string_lossy().into_owned(),
    ]
}
pub async fn schemes(project: &Path) -> Result<Vec<String>> {
    let mut args = project_args(project);
    args.extend(strings(&["-list", "-json"]));
    let value: Value = serde_json::from_str(&output("xcodebuild", &args).await?)?;
    let schemes = value
        .pointer("/workspace/schemes")
        .or_else(|| value.pointer("/project/schemes"))
        .and_then(Value::as_array)
        .context("Xcode returned no schemes")?;
    Ok(schemes
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect())
}
pub fn select_scheme(schemes: &[String], requested: Option<&str>) -> Result<String> {
    if let Some(name) = requested {
        if schemes.iter().any(|s| s == name) {
            return Ok(name.to_owned());
        }
        bail!("Unknown scheme {name}; available: {}", schemes.join(", "));
    }
    if let [scheme] = schemes {
        return Ok(scheme.clone());
    }
    bail!("Specify --scheme; available: {}", schemes.join(", "))
}
pub fn app_from_settings(settings: &str) -> Result<PathBuf> {
    let targets: Vec<Value> = serde_json::from_str(settings)?;
    let apps: Vec<_> = targets
        .iter()
        .filter_map(|target| {
            let s = &target["buildSettings"];
            if s["PRODUCT_TYPE"].as_str()? != "com.apple.product-type.application" {
                return None;
            }
            Some(
                PathBuf::from(s["TARGET_BUILD_DIR"].as_str()?)
                    .join(s["FULL_PRODUCT_NAME"].as_str()?),
            )
        })
        .collect();
    match apps.as_slice() {
        [app] => Ok(app.clone()),
        _ => bail!(
            "Expected one application target, found {}; choose an app scheme",
            apps.len()
        ),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selects_app_not_extension() {
        let json = r#"[{"buildSettings":{"PRODUCT_TYPE":"com.apple.product-type.app-extension","TARGET_BUILD_DIR":"/tmp","FULL_PRODUCT_NAME":"Widget.appex"}},{"buildSettings":{"PRODUCT_TYPE":"com.apple.product-type.application","TARGET_BUILD_DIR":"/tmp/Build Products","FULL_PRODUCT_NAME":"Demo.app"}}]"#;
        assert_eq!(
            app_from_settings(json).unwrap(),
            PathBuf::from("/tmp/Build Products/Demo.app")
        );
        assert!(app_from_settings("[]").is_err());
    }
    #[test]
    fn scheme_ambiguity_is_explicit() {
        assert!(select_scheme(&["A".into(), "B".into()], None).is_err());
        assert!(select_scheme(&["A".into()], Some("B")).is_err());
        assert_eq!(select_scheme(&["A".into()], None).unwrap(), "A");
    }
}
