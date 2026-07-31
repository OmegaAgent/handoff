//! Path resolution and the matcher engine.
//!
//! Paths are dotted, with optional `[n]` indices and a `[]` wildcard that yields every element:
//! `error.code`, `data[0].id`, `data[].org_id`. An empty path addresses the whole document. The
//! syntax is deliberately smaller than JSONPath — a case file is read by people deciding whether an
//! implementation is honest, and every extra construct is somewhere for a subtle assertion to hide.

use crate::case::{Matcher, Op};
use serde_json::Value;
use std::collections::BTreeMap;

/// One resolved value and the concrete path it was found at.
#[derive(Debug, Clone)]
pub struct Hit {
    /// Path with wildcards replaced by the index that matched.
    pub at: String,
    /// The value found there.
    pub value: Value,
}

/// Resolve a path against a document. An unresolvable path yields an empty vector.
pub fn resolve(doc: &Value, path: &str) -> Vec<Hit> {
    let path = path.trim().trim_start_matches("$.").trim_start_matches('$');
    if path.is_empty() {
        return vec![Hit {
            at: String::new(),
            value: doc.clone(),
        }];
    }
    let mut hits = vec![Hit {
        at: String::new(),
        value: doc.clone(),
    }];
    for segment in path.split('.') {
        let (name, accessors) = split_accessors(segment);
        let mut next = Vec::new();
        for hit in &hits {
            let base = if name.is_empty() {
                Some(hit.value.clone())
            } else {
                hit.value.get(name).cloned()
            };
            let Some(base) = base else { continue };
            let at = join(&hit.at, name);
            descend(base, at, &accessors, &mut next);
        }
        hits = next;
        if hits.is_empty() {
            return hits;
        }
    }
    hits
}

fn descend(value: Value, at: String, accessors: &[Accessor], out: &mut Vec<Hit>) {
    match accessors.split_first() {
        None => out.push(Hit { at, value }),
        Some((Accessor::Index(i), rest)) => {
            if let Some(v) = value.get(*i) {
                descend(v.clone(), format!("{at}[{i}]"), rest, out);
            }
        }
        Some((Accessor::Wildcard, rest)) => {
            if let Some(items) = value.as_array() {
                for (i, v) in items.iter().enumerate() {
                    descend(v.clone(), format!("{at}[{i}]"), rest, out);
                }
            }
        }
    }
}

enum Accessor {
    Index(usize),
    Wildcard,
}

fn split_accessors(segment: &str) -> (&str, Vec<Accessor>) {
    let Some(open) = segment.find('[') else {
        return (segment, Vec::new());
    };
    let (name, mut rest) = segment.split_at(open);
    let mut accessors = Vec::new();
    while let Some(start) = rest.find('[') {
        let Some(end) = rest[start..].find(']') else {
            break;
        };
        let inner = &rest[start + 1..start + end];
        accessors.push(match inner.trim().parse::<usize>() {
            Ok(i) => Accessor::Index(i),
            Err(_) => Accessor::Wildcard,
        });
        rest = &rest[start + end + 1..];
    }
    (name, accessors)
}

fn join(prefix: &str, name: &str) -> String {
    match (prefix.is_empty(), name.is_empty()) {
        (_, true) => prefix.to_string(),
        (true, false) => name.to_string(),
        (false, false) => format!("{prefix}.{name}"),
    }
}

/// Check one matcher against a document, returning a reason on failure.
///
/// Values carried by the operator are `${var}`-interpolated first, so a case can write
/// `none_equal: "${signal_id}"` and mean the id an earlier step captured.
pub fn check(
    doc: &Value,
    matcher: &Matcher,
    vars: &BTreeMap<String, String>,
) -> Result<(), String> {
    let op = interpolate_op(&matcher.op, vars)?;
    let matcher = &Matcher {
        path: matcher.path.clone(),
        op,
        because: matcher.because.clone(),
    };
    let hits = resolve(doc, &matcher.path);
    let path = if matcher.path.is_empty() {
        "<document>"
    } else {
        matcher.path.as_str()
    };
    let single = || -> Result<Value, String> {
        match hits.len() {
            0 => Err(format!("`{path}` is absent")),
            _ => Ok(hits[0].value.clone()),
        }
    };

    let outcome = match &matcher.op {
        Op::Exists(want) => {
            let got = !hits.is_empty();
            if got == *want {
                Ok(())
            } else if *want {
                Err(format!("`{path}` is absent, expected it to be present"))
            } else {
                Err(format!(
                    "`{path}` is present ({}), expected it to be absent",
                    brief(&hits[0].value)
                ))
            }
        }
        Op::IsNull(want) => {
            let v = single()?;
            let got = v.is_null();
            if got == *want {
                Ok(())
            } else {
                Err(format!(
                    "`{path}` is {}, expected {}",
                    brief(&v),
                    if *want { "null" } else { "non-null" }
                ))
            }
        }
        Op::Equals(want) => {
            let v = single()?;
            if v == *want {
                Ok(())
            } else {
                Err(format!(
                    "`{path}` is {}, expected {}",
                    brief(&v),
                    brief(want)
                ))
            }
        }
        Op::NotEquals(want) => {
            let v = single()?;
            if v != *want {
                Ok(())
            } else {
                Err(format!("`{path}` is {}, expected anything else", brief(&v)))
            }
        }
        Op::Matches(pattern) => {
            let v = single()?;
            let text = as_text(&v);
            match regex::Regex::new(pattern) {
                Err(e) => Err(format!(
                    "case defect: `{pattern}` is not a valid regex ({e})"
                )),
                Ok(re) if re.is_match(&text) => Ok(()),
                Ok(_) => Err(format!(
                    "`{path}` is {}, expected it to match /{pattern}/",
                    brief(&v)
                )),
            }
        }
        Op::Length(want) => {
            let v = single()?;
            match length_of(&v) {
                Some(got) if got == *want => Ok(()),
                Some(got) => Err(format!("`{path}` has length {got}, expected {want}")),
                None => Err(format!("`{path}` is {}, which has no length", brief(&v))),
            }
        }
        Op::LengthAtLeast(want) => {
            let v = single()?;
            match length_of(&v) {
                Some(got) if got >= *want => Ok(()),
                Some(got) => Err(format!(
                    "`{path}` has length {got}, expected at least {want}"
                )),
                None => Err(format!("`{path}` is {}, which has no length", brief(&v))),
            }
        }
        Op::OneOf(options) => {
            let v = single()?;
            if options.contains(&v) {
                Ok(())
            } else {
                Err(format!(
                    "`{path}` is {}, expected one of {}",
                    brief(&v),
                    brief(&Value::Array(options.clone()))
                ))
            }
        }
        Op::SameAs(var) => {
            let v = single()?;
            let want = lookup(vars, var)?;
            if as_text(&v) == want {
                Ok(())
            } else {
                Err(format!(
                    "`{path}` is {}, expected it to equal ${{{var}}} ({want})",
                    brief(&v)
                ))
            }
        }
        Op::DiffersFrom(var) => {
            let v = single()?;
            let want = lookup(vars, var)?;
            if as_text(&v) != want {
                Ok(())
            } else {
                Err(format!(
                    "`{path}` equals ${{{var}}} ({want}); the two must differ"
                ))
            }
        }
        Op::AllEqual(want) => {
            if hits.is_empty() {
                Err(format!(
                    "`{path}` matched nothing, so `all_equal` proves nothing"
                ))
            } else if let Some(bad) = hits.iter().find(|h| h.value != *want) {
                Err(format!(
                    "`{}` is {}, expected every element to be {}",
                    bad.at,
                    brief(&bad.value),
                    brief(want)
                ))
            } else {
                Ok(())
            }
        }
        Op::NoneEqual(want) => {
            if hits.is_empty() {
                Err(format!(
                    "`{path}` matched nothing, so `none_equal` proves nothing — a collection that \
                     is empty contains no forbidden value and contains no correct one either. An \
                     earlier assertion in the same step must establish that the path resolves."
                ))
            } else if let Some(bad) = hits.iter().find(|h| h.value == *want) {
                Err(format!(
                    "`{}` is {}, which must not appear here",
                    bad.at,
                    brief(&bad.value)
                ))
            } else {
                Ok(())
            }
        }
        Op::SetEquals(want) => {
            let mut got: Vec<String> = hits.iter().map(|h| as_text(&h.value)).collect();
            let mut want: Vec<String> = want
                .iter()
                .map(|w| interpolate_var(w, vars))
                .collect::<Result<_, _>>()?;
            got.sort();
            want.sort();
            if got == want {
                Ok(())
            } else {
                Err(format!(
                    "`{path}` yielded {got:?}, expected exactly {want:?} — length and identity, \
                     because a query missing its tenant predicate returns a superset"
                ))
            }
        }
        Op::ContainsText(needle) => {
            let v = single()?;
            let needle = interpolate_var(needle, vars)?;
            if serialize(&v).contains(&needle) {
                Ok(())
            } else {
                Err(format!("`{path}` does not contain {needle:?}"))
            }
        }
        Op::NotContainsText(needle) => {
            let v = single()?;
            let needle = interpolate_var(needle, vars)?;
            if serialize(&v).contains(&needle) {
                Err(format!(
                    "`{path}` contains {needle:?}, which must not appear"
                ))
            } else {
                Ok(())
            }
        }
    };

    outcome.map_err(|reason| match &matcher.because {
        Some(why) => format!("{reason}\n      because: {why}"),
        None => reason,
    })
}

fn interpolate_op(op: &Op, vars: &BTreeMap<String, String>) -> Result<Op, String> {
    let fill = |v: &Value| crate::vars::interpolate_json(v, vars);
    Ok(match op {
        Op::Equals(v) => Op::Equals(fill(v)?),
        Op::NotEquals(v) => Op::NotEquals(fill(v)?),
        Op::AllEqual(v) => Op::AllEqual(fill(v)?),
        Op::NoneEqual(v) => Op::NoneEqual(fill(v)?),
        Op::OneOf(vs) => Op::OneOf(vs.iter().map(fill).collect::<Result<_, _>>()?),
        other => other.clone(),
    })
}

fn lookup(vars: &BTreeMap<String, String>, name: &str) -> Result<String, String> {
    vars.get(name)
        .cloned()
        .ok_or_else(|| format!("no captured variable `{name}`; an earlier step must capture it"))
}

fn interpolate_var(text: &str, vars: &BTreeMap<String, String>) -> Result<String, String> {
    crate::vars::interpolate(text, vars)
}

fn length_of(v: &Value) -> Option<usize> {
    match v {
        Value::Array(a) => Some(a.len()),
        Value::String(s) => Some(s.chars().count()),
        Value::Object(o) => Some(o.len()),
        _ => None,
    }
}

fn as_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn serialize(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// A short, readable rendering of a value for a failure message.
pub fn brief(v: &Value) -> String {
    let text = v.to_string();
    if text.chars().count() <= 160 {
        text
    } else {
        let head: String = text.chars().take(157).collect();
        format!("{head}...")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> Value {
        serde_json::json!({
            "id": "req_1",
            "error": {"code": "already_answered"},
            "data": [{"org_id": "org_a"}, {"org_id": "org_a"}],
            "receipt": null
        })
    }

    #[test]
    fn resolves_nested_and_indexed_paths() {
        assert_eq!(
            resolve(&doc(), "error.code")[0].value,
            serde_json::json!("already_answered")
        );
        assert_eq!(
            resolve(&doc(), "data[1].org_id")[0].value,
            serde_json::json!("org_a")
        );
        assert_eq!(resolve(&doc(), "data[].org_id").len(), 2);
        assert!(resolve(&doc(), "nope.nothing").is_empty());
    }

    #[test]
    fn empty_path_is_the_whole_document() {
        assert_eq!(resolve(&doc(), "").len(), 1);
    }

    #[test]
    fn set_equals_rejects_a_superset() {
        let m = Matcher {
            path: "data[].org_id".into(),
            op: Op::SetEquals(vec!["org_a".into()]),
            because: None,
        };
        assert!(check(&doc(), &m, &BTreeMap::new()).is_err());
    }

    #[test]
    fn is_null_distinguishes_null_from_absent() {
        let null = Matcher {
            path: "receipt".into(),
            op: Op::IsNull(true),
            because: None,
        };
        assert!(check(&doc(), &null, &BTreeMap::new()).is_ok());
        let absent = Matcher {
            path: "nope".into(),
            op: Op::IsNull(true),
            because: None,
        };
        assert!(check(&doc(), &absent, &BTreeMap::new()).is_err());
    }

    #[test]
    fn none_equal_refuses_an_empty_match_set_the_way_all_equal_does() {
        // The two operators are the same shape and must fail the same way on nothing, because a
        // collection that contains no forbidden value and a collection that contains nothing at
        // all are indistinguishable from the assertion's side. A hostile review found two live
        // uses resolving to zero hits, one of them still passing when repointed at a `waiter_ref`
        // that had never existed.
        let none = Matcher {
            path: "data[].type".into(),
            op: Op::NoneEqual(serde_json::json!("answered")),
            because: None,
        };
        let all = Matcher {
            path: "data[].type".into(),
            op: Op::AllEqual(serde_json::json!("answered")),
            because: None,
        };
        let empty = serde_json::json!({"data": []});
        assert!(check(&empty, &all, &BTreeMap::new()).is_err());
        let err = check(&empty, &none, &BTreeMap::new()).unwrap_err();
        assert!(err.contains("matched nothing"), "{err}");

        // And it still passes where the path resolves and the forbidden value is absent.
        let populated = serde_json::json!({"data": [{"type": "cancelled"}]});
        assert!(check(&populated, &none, &BTreeMap::new()).is_ok());
    }

    #[test]
    fn failure_messages_carry_the_because_line() {
        let m = Matcher {
            path: "error.code".into(),
            op: Op::Equals(serde_json::json!("request_expired")),
            because: Some("§6.7 requires a specific 409".into()),
        };
        let err = check(&doc(), &m, &BTreeMap::new()).unwrap_err();
        assert!(err.contains("§6.7"));
    }
}
