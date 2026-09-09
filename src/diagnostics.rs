use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: String,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub message: String,
}
#[derive(Debug, Serialize)]
pub struct BuildFailure {
    pub build_log: PathBuf,
    pub diagnostics: Vec<Diagnostic>,
    pub reason: String,
    pub delta: Delta,
}
impl std::fmt::Display for BuildFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Build failed: {}; see {}",
            self.reason,
            self.build_log.display()
        )
    }
}
impl std::error::Error for BuildFailure {}
pub fn parse(text: &str) -> Vec<Diagnostic> {
    text.lines()
        .filter_map(|line| {
            let (prefix, severity, message) =
                ["error", "warning", "note"].iter().find_map(|severity| {
                    line.split_once(&format!(": {severity}: "))
                        .map(|(p, m)| (p, *severity, m))
                })?;
            let mut pieces = prefix.rsplitn(3, ':');
            let column = pieces.next().and_then(|v| v.parse().ok());
            let line = pieces.next().and_then(|v| v.parse().ok());
            let file = if line.is_some() && column.is_some() {
                pieces.next().map(str::to_owned)
            } else {
                None
            };
            Some(Diagnostic {
                severity: severity.into(),
                file,
                line,
                column,
                message: message.into(),
            })
        })
        .take(200)
        .collect()
}
#[derive(Debug, Serialize)]
pub struct Delta {
    pub added: Vec<Diagnostic>,
    pub resolved: Vec<Diagnostic>,
}
pub fn diff(before: &[Diagnostic], after: &[Diagnostic]) -> Delta {
    Delta {
        added: after
            .iter()
            .filter(|d| !before.contains(d))
            .cloned()
            .collect(),
        resolved: before
            .iter()
            .filter(|d| !after.contains(d))
            .cloned()
            .collect(),
    }
}
pub fn read(path: &std::path::Path) -> std::io::Result<Vec<Diagnostic>> {
    use std::io::BufRead;
    let reader = std::io::BufReader::new(std::fs::File::open(path)?);
    let mut result = Vec::new();
    let mut reader = reader;
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            result.extend(parse(&String::from_utf8_lossy(&line)));
            break;
        }
        let end = available.iter().position(|b| *b == b'\n');
        let consumed = end.map_or(available.len(), |i| i + 1);
        let keep = consumed.min((64 * 1024_usize).saturating_sub(line.len()));
        line.extend_from_slice(&available[..keep]);
        reader.consume(consumed);
        if end.is_some() {
            result.extend(parse(&String::from_utf8_lossy(&line)));
            line.clear();
            if result.len() >= 200 {
                break;
            }
        }
    }
    result.truncate(200);
    Ok(result)
}
#[cfg(test)]
mod tests {
    #[test]
    fn parses_paths_with_spaces_and_colons() {
        let result = super::parse(
            "/tmp/a:b/App File.swift:12:4: error: missing type\nld: warning: ignored option",
        );
        assert_eq!(result[0].file.as_deref(), Some("/tmp/a:b/App File.swift"));
        assert_eq!(result[0].line, Some(12));
        assert_eq!(result[1].severity, "warning");
    }
}
