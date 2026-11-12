//! The params the run sends: what a hostile caller sends, and the calls a command's own schema
//! says are wrong.

use serde_json::{Map, Value, json};

use super::values::{self, Schema};

/// The key the run adds to params to see an unknown key refused.
pub(super) const UNKNOWN_KEY: &str = "conformance_unknown_key";

/// The id the run calls to see an unknown command refused.
pub(super) const UNKNOWN_ID: &str = "conformance.no_such_command";

/// Params a hostile caller may send a command: wrong types, edge numbers, huge and deep values,
/// path tricks, unknown keys. A command must answer each with a result or a typed error, and never
/// panic.
pub fn adversarial_params() -> Vec<Value> {
    let mut deep = json!(1);
    for _ in 0..100 {
        deep = json!({ "k": deep });
    }
    let many: Map<String, Value> = (0..1_000).map(|n| (format!("k{n}"), json!(n))).collect();
    vec![
        Value::Null,
        json!(true),
        json!(0),
        json!(-1),
        json!(1.0e308),
        json!(u64::MAX),
        json!(i64::MIN),
        json!(""),
        json!("x".repeat(100_000)),
        json!("\u{0}\u{202e}\u{fffd}"),
        json!([]),
        json!([1, "a", null]),
        json!({}),
        json!({"": 1}),
        json!({"unknown": 1}),
        json!({"__proto__": {}, "constructor": 1}),
        json!({"path": "../../etc/passwd"}),
        json!({"path": "C:\\Windows\\System32", "name": "\u{0}"}),
        json!({"text": "x".repeat(100_000)}),
        deep,
        Value::Object(many),
    ]
}

/// What a case says of the failure's `key`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Key {
    /// No claim.
    Any,
    /// It must be this param.
    Is(String),
    /// It must be one of these params.
    OneOf(Vec<String>),
    /// When there is one, it must be this param.
    IfPresent(String),
}

impl Key {
    /// Why `key` breaks the case's claim, if it does.
    pub(super) fn problem(&self, key: Option<&str>) -> Option<String> {
        let fine = match (self, key) {
            (Key::Any, _) | (Key::IfPresent(_), None) => true,
            (Key::Is(want) | Key::IfPresent(want), Some(k)) => k == want,
            (Key::OneOf(wants), Some(k)) => wants.iter().any(|w| w == k),
            (Key::Is(_) | Key::OneOf(_), None) => false,
        };
        let want = match self {
            Key::Any => String::new(),
            Key::Is(want) => format!("`{want}`"),
            Key::IfPresent(want) => format!("`{want}` or absent"),
            Key::OneOf(wants) => format!("one of `{}`", wants.join("`, `")),
        };
        let got = key.map_or("absent".to_owned(), |k| format!("`{k}`"));
        (!fine).then(|| format!("`key` is {got} but should be {want}"))
    }
}

/// A call that must be `bad_params`: what it is, its params and what its `key` must be.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Case {
    pub what: String,
    pub params: Value,
    pub key: Key,
}

/// The calls that must be `bad_params` for a command whose params schema is `schema`, each with the
/// required params it doesn't break filled with valid values. `names_type_key`: the app names the
/// key of a wrong-typed value.
pub(super) fn bad_params(schema: &Value, names_type_key: bool) -> Vec<Case> {
    let case = |what: &str, params: Value, key: Key| Case { what: what.to_owned(), params, key };
    let mut cases: Vec<Case> = [json!([1]), json!(5), json!("x"), json!(true)]
        .into_iter()
        .map(|params| case(&format!("params that are not an object ({params})"), params, Key::Any))
        .collect();
    let Some(top) = Schema::root(schema).resolved() else { return cases };
    let base = values::required_filled(top).unwrap_or_default();
    if !takes_any_key(top) {
        cases.push(case("an unknown key", with(&base, UNKNOWN_KEY, json!(1)), Key::Is(UNKNOWN_KEY.to_owned())));
    }
    let required: Vec<String> = top.required().into_iter().map(str::to_owned).collect();
    if !required.is_empty() {
        cases.push(case("a missing required key", json!({}), Key::OneOf(required)));
    }
    for (name, property) in top.properties() {
        let key = if names_type_key { Key::Is(name.clone()) } else { Key::IfPresent(name.clone()) };
        if let Some(wrong) = wrong_value(property) {
            cases.push(case(&format!("a wrong-typed `{name}` ({wrong})"), with(&base, name, wrong), key.clone()));
        }
        if let Some((inner, nested)) = wrong_inside(property) {
            cases.push(case(&format!("a wrong-typed `{inner}` inside `{name}`"), with(&base, name, nested), key));
        }
    }
    cases
}

/// `base` with `key` set to `value`.
fn with(base: &Map<String, Value>, key: &str, value: Value) -> Value {
    let mut params = base.clone();
    params.insert(key.to_owned(), value);
    Value::Object(params)
}

/// Whether the schema lets a call carry keys it doesn't list (`additionalProperties` true or a
/// schema): a free-form map takes any key.
fn takes_any_key(schema: Schema<'_>) -> bool {
    match schema.get("additionalProperties") {
        Some(Value::Bool(false)) | None => false,
        Some(_) => true,
    }
}

/// How deep [`allowed_types`] and [`object_schema`] follow branches.
const MAX_BRANCH_DEPTH: usize = 8;

/// The JSON types `schema` allows, when it says them: by its `type`, its `const` or `enum` values,
/// its `anyOf`/`oneOf` branches (each must say), a single `allOf`, or its `properties` (an object).
/// `$ref`s are followed.
fn allowed_types(schema: Schema<'_>, depth: usize) -> Option<Vec<&'static str>> {
    if depth > MAX_BRANCH_DEPTH {
        return None;
    }
    let s = schema.resolved()?;
    if let Some(kind) = s.get("type") {
        let named: Vec<&Value> = match kind {
            Value::Array(many) => many.iter().collect(),
            one => vec![one],
        };
        return Some(named.into_iter().filter_map(Value::as_str).filter_map(known_type).collect());
    }
    if let Some(constant) = s.get("const") {
        return Some(vec![json_type(constant)]);
    }
    if let Some(listed) = s.get("enum").and_then(Value::as_array) {
        return Some(listed.iter().map(json_type).collect());
    }
    if let Some(branches) = s.get("anyOf").or_else(|| s.get("oneOf")).and_then(Value::as_array) {
        let each: Option<Vec<Vec<&str>>> = branches.iter().map(|b| allowed_types(s.at(b), depth + 1)).collect();
        return each.map(|each| each.concat());
    }
    match s.get("allOf").and_then(Value::as_array).map(Vec::as_slice) {
        Some([one]) => allowed_types(s.at(one), depth + 1),
        _ if s.get("properties").is_some() => Some(vec!["object"]),
        _ => None,
    }
}

/// The JSON type name `kind`, as a `'static` one.
fn known_type(kind: &str) -> Option<&'static str> {
    ["null", "boolean", "integer", "number", "string", "array", "object"].into_iter().find(|k| *k == kind)
}

/// The JSON type of `value`, as a schema names it.
fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// A value of a JSON type that `property`'s schema doesn't allow, when the schema says its types.
fn wrong_value(property: Schema<'_>) -> Option<Value> {
    let allowed = allowed_types(property, 0)?;
    // `integer` is a kind of `number`, so a number with a fraction is wrong for it only.
    [("string", json!("conformance")), ("boolean", json!(true)), ("array", json!([])), ("object", json!({})), ("number", json!(0.5))]
        .into_iter()
        .find(|(kind, _)| !allowed.contains(kind))
        .map(|(_, value)| value)
}

/// For a param that is an object (a struct, or an `Option` of one): its first property that can be
/// given a wrong type, and the param's plain value with that property wrong.
fn wrong_inside(property: Schema<'_>) -> Option<(String, Value)> {
    let object = object_schema(property, 0)?;
    let Value::Object(plain) = values::plain(object)? else { return None };
    object.properties().into_iter().find_map(|(inner, schema)| wrong_value(schema).map(|wrong| (inner.clone(), with(&plain, inner, wrong))))
}

/// The object schema with `properties` that `schema` is, or that one of its `anyOf`/`oneOf`
/// branches is.
fn object_schema(schema: Schema<'_>, depth: usize) -> Option<Schema<'_>> {
    if depth > MAX_BRANCH_DEPTH {
        return None;
    }
    let s = schema.resolved()?;
    if !s.properties().is_empty() {
        return Some(s);
    }
    let branches = s.get("anyOf").or_else(|| s.get("oneOf")).and_then(Value::as_array)?;
    branches.iter().find_map(|b| object_schema(s.at(b), depth + 1))
}
