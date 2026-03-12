use serde::{Deserialize, Deserializer, Serialize};

fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueClassification {
    pub category: String,
    pub confidence: f64,
    pub priority: Option<String>,
    pub summary: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub suggested_labels: Vec<String>,
    pub is_duplicate_of: Option<u64>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub is_simple_fix: bool,
    #[serde(default, deserialize_with = "null_as_default")]
    pub relevant_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrAnalysis {
    pub summary: String,
    pub risk_level: String,
    pub pr_type: String,
    #[serde(default)]
    pub linked_issues: Vec<u64>,
    #[serde(default)]
    pub review_checklist: ReviewChecklist,
    #[serde(default)]
    pub suggested_labels: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReviewChecklist {
    #[serde(default)]
    pub tests_present: bool,
    #[serde(default)]
    pub breaking_change: bool,
    #[serde(default)]
    pub docs_updated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineReviewResult {
    #[serde(default)]
    pub comments: Vec<InlineComment>,
    #[serde(default)]
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineComment {
    pub path: String,
    pub line: u64,
    pub body: String,
    #[serde(default = "default_severity")]
    pub severity: String,
}

fn default_severity() -> String {
    "warning".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictResolution {
    pub resolvable: bool,
    pub confidence: f64,
    pub strategy: String,
    pub description: String,
}
