//! `${variable}` interpolation.
//!
//! Cases bind variables with `capture:` and read them back anywhere a string appears — a path, a
//! header value, a JSON string inside a body. An unbound variable is an error rather than an empty
//! string: silently substituting nothing is how a test starts asserting against `/requests//answer`
//! and passing for the wrong reason.

use std::collections::BTreeMap;

/// Substitute every `${name}` in `text`. An unbound name is an error.
pub fn interpolate(text: &str, vars: &BTreeMap<String, String>) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            return Err(format!("unterminated `${{` in {text:?}"));
        };
        let name = &after[..end];
        match vars.get(name) {
            Some(value) => out.push_str(value),
            None => {
                return Err(format!(
                    "`${{{name}}}` is not bound. An earlier step must capture it, or the profile \
                     must define it."
                ))
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Interpolate every string inside a JSON document, in place.
pub fn interpolate_json(
    value: &serde_json::Value,
    vars: &BTreeMap<String, String>,
) -> Result<serde_json::Value, String> {
    use serde_json::Value;
    Ok(match value {
        Value::String(s) => {
            let filled = interpolate(s, vars)?;
            // A lone `${var}` standing for a whole value may need to be a number, a bool, or a
            // parsed object rather than a string — `"${count}"` meaning `2`. The `!json:` prefix
            // asks for that explicitly, so ordinary strings are never reinterpreted by accident.
            match filled.strip_prefix("!json:") {
                Some(raw) => serde_json::from_str(raw)
                    .map_err(|e| format!("`!json:` value {raw:?} is not valid JSON: {e}"))?,
                None => Value::String(filled),
            }
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|v| interpolate_json(v, vars))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(fields) => {
            let mut out = serde_json::Map::with_capacity(fields.len());
            for (k, v) in fields {
                out.insert(interpolate(k, vars)?, interpolate_json(v, vars)?);
            }
            Value::Object(out)
        }
        other => other.clone(),
    })
}

/// A run identifier, unique per invocation, so that reruns against a live deployment do not collide
/// on idempotency keys or dedupe keys left behind by the previous run.
pub fn run_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}{:x}", std::process::id(), nanos)
}

/// Parse the ISO-8601 durations the protocol uses — `PT15M`, `PT4H`, `P1D`, `PT0.5S`.
///
/// Deliberately narrow: this understands the shapes `openapi.yaml` documents and rejects the rest
/// rather than guessing.
pub fn parse_duration(text: &str) -> Result<std::time::Duration, String> {
    let bad = || format!("`{text}` is not an ISO-8601 duration the suite understands (PT15M, P1D)");
    let rest = text.strip_prefix('P').ok_or_else(bad)?;
    let (days_part, time_part) = match rest.split_once('T') {
        Some((d, t)) => (d, t),
        None => (rest, ""),
    };

    let mut seconds = 0f64;
    let mut number = String::new();
    for c in days_part.chars() {
        match c {
            '0'..='9' | '.' => number.push(c),
            'D' => {
                seconds += number.parse::<f64>().map_err(|_| bad())? * 86_400.0;
                number.clear();
            }
            'W' => {
                seconds += number.parse::<f64>().map_err(|_| bad())? * 604_800.0;
                number.clear();
            }
            _ => return Err(bad()),
        }
    }
    if !number.is_empty() {
        return Err(bad());
    }
    for c in time_part.chars() {
        match c {
            '0'..='9' | '.' => number.push(c),
            'H' => {
                seconds += number.parse::<f64>().map_err(|_| bad())? * 3_600.0;
                number.clear();
            }
            'M' => {
                seconds += number.parse::<f64>().map_err(|_| bad())? * 60.0;
                number.clear();
            }
            'S' => {
                seconds += number.parse::<f64>().map_err(|_| bad())?;
                number.clear();
            }
            _ => return Err(bad()),
        }
    }
    if !number.is_empty() {
        return Err(bad());
    }
    Ok(std::time::Duration::from_secs_f64(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars() -> BTreeMap<String, String> {
        BTreeMap::from([("req".to_string(), "req_01".to_string())])
    }

    #[test]
    fn substitutes_bound_names() {
        assert_eq!(
            interpolate("/requests/${req}/answer", &vars()).unwrap(),
            "/requests/req_01/answer"
        );
    }

    #[test]
    fn an_unbound_name_is_an_error_not_an_empty_string() {
        assert!(interpolate("/requests/${nope}", &vars()).is_err());
    }

    #[test]
    fn parses_the_durations_the_protocol_uses() {
        assert_eq!(parse_duration("PT15M").unwrap().as_secs(), 900);
        assert_eq!(parse_duration("PT4H").unwrap().as_secs(), 14_400);
        assert_eq!(parse_duration("P1D").unwrap().as_secs(), 86_400);
        assert_eq!(parse_duration("PT0S").unwrap().as_secs(), 0);
        assert!(parse_duration("15m").is_err());
    }
}
