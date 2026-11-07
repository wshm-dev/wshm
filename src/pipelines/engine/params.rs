//! Validation and reading of a step's `with` parameters.

use serde_json::{Map, Value};

/// Type of one accepted parameter.
#[derive(Debug, Clone, Copy)]
pub enum Param {
    Bool,
    /// Integer in `min..=max`.
    Int(u64, u64),
    /// Non-empty string of at most this many chars.
    Str(usize),
    /// List of non-empty strings.
    List,
    /// List of globs (see `condition::glob_regex`).
    Globs,
    /// One of these strings.
    OneOf(&'static [&'static str]),
}

/// Check `params` against the accepted `spec` of step kind `kind`.
pub fn check(
    kind: &str,
    params: &Map<String, Value>,
    spec: &[(&str, Param)],
) -> Result<(), String> {
    for (k, v) in params {
        let Some((_, p)) = spec.iter().find(|(name, _)| name == k) else {
            let known: Vec<&str> = spec.iter().map(|(n, _)| *n).collect();
            return Err(if known.is_empty() {
                format!("step kind '{kind}' takes no parameters")
            } else {
                format!(
                    "unknown parameter '{k}' for step kind '{kind}' (known: {})",
                    known.join(", ")
                )
            });
        };
        let bad = |what: &str| format!("parameter '{k}' of '{kind}': expected {what}");
        match *p {
            Param::Bool => {
                if !v.is_boolean() {
                    return Err(bad("true or false"));
                }
            }
            Param::Int(min, max) => match v.as_u64() {
                Some(n) if (min..=max).contains(&n) => {}
                _ => return Err(bad(&format!("an integer between {min} and {max}"))),
            },
            Param::Str(max) => match v.as_str() {
                Some(s) if !s.trim().is_empty() && s.chars().count() <= max => {}
                _ => return Err(bad(&format!("a non-empty string of at most {max} chars"))),
            },
            Param::OneOf(allowed) => match v.as_str() {
                Some(s) if allowed.contains(&s) => {}
                _ => return Err(bad(&format!("one of {}", allowed.join(", ")))),
            },
            Param::List | Param::Globs => {
                let Some(items) = v.as_array() else {
                    return Err(bad("a list of strings"));
                };
                for i in items {
                    let Some(s) = i.as_str().filter(|s| !s.trim().is_empty()) else {
                        return Err(bad("a list of non-empty strings"));
                    };
                    if matches!(p, Param::Globs) {
                        super::condition::glob_regex(s).map_err(|e| bad(&e))?;
                    }
                }
            }
        }
    }
    Ok(())
}

pub fn bool_or(params: &Map<String, Value>, key: &str, default: bool) -> bool {
    params.get(key).and_then(Value::as_bool).unwrap_or(default)
}

pub fn u64_or(params: &Map<String, Value>, key: &str, default: u64) -> u64 {
    params.get(key).and_then(Value::as_u64).unwrap_or(default)
}

pub fn str_or<'a>(params: &'a Map<String, Value>, key: &str, default: &'a str) -> &'a str {
    params.get(key).and_then(Value::as_str).unwrap_or(default)
}

/// A list parameter, or `default` when absent.
pub fn list_or(params: &Map<String, Value>, key: &str, default: &[&str]) -> Vec<String> {
    match params.get(key).and_then(Value::as_array) {
        Some(items) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        None => default.iter().map(|s| s.to_string()).collect(),
    }
}

/// Whether `path` matches one of `globs`.
pub fn matches_any(globs: &[String], path: &str) -> bool {
    globs
        .iter()
        .filter_map(|g| super::condition::glob_regex(g).ok())
        .any(|re| re.is_match(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_types_and_names() {
        let spec = [
            ("voters", Param::Int(1, 5)),
            ("post", Param::Bool),
            ("paths", Param::Globs),
            ("mode", Param::OneOf(&["post", "collect"])),
        ];
        let ok = |v: Value| check("k", v.as_object().unwrap(), &spec);
        assert!(ok(
            serde_json::json!({"voters": 3, "post": false, "paths": ["docs/**"], "mode": "collect"})
        )
        .is_ok());
        assert!(ok(serde_json::json!({"voters": 9})).is_err());
        assert!(ok(serde_json::json!({"post": "yes"})).is_err());
        assert!(ok(serde_json::json!({"paths": [""]})).is_err());
        assert!(ok(serde_json::json!({"mode": "x"})).is_err());
        assert!(ok(serde_json::json!({"nope": 1}))
            .unwrap_err()
            .contains("known: voters"));
        assert!(
            check("k", serde_json::json!({"a": 1}).as_object().unwrap(), &[])
                .unwrap_err()
                .contains("takes no parameters")
        );
    }
}
