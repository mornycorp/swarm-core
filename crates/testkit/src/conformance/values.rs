//! Valid params, generated from a command's params schema, so the run reaches `run` (not only the
//! dispatch's params check) with values a hostile caller may send: each property in turn with its
//! edge values (empty and huge strings, path tricks, unicode, the ends of an integer's range, large
//! arrays and maps, deep values), the required others filled plainly. `$ref`/`$defs`, `anyOf`/`oneOf`
//! (`Option<T>`, enums), `allOf`, `enum` and `const` are followed; a value of a `format` (a date, a
//! UUID) is one of that format.

use serde_json::{Map, Number, Value, json};

use super::REQUEST_CAP;

/// How deep a schema is followed (`$ref` chains, nested objects and arrays): a recursive type stops here.
const MAX_DEPTH: usize = 12;
/// Below this depth a value with every property filled has only its required ones, so a type with
/// several recursive fields doesn't grow without bound.
const FULL_DEPTH: usize = 6;
/// The most values one schema is tried with.
const PER_SCHEMA: usize = 24;
/// The most calls one command is generated.
const PER_COMMAND: usize = 200;
/// The items of a large array and the keys of a large map.
const LARGE: usize = 1_000;
/// The characters of a huge string.
const HUGE: usize = 100_000;
/// The levels of a deep value of a free-form (`serde_json::Value`) param.
const DEEP: usize = 100;
/// The schema that takes any value: an array's items when it doesn't say.
static ANY: Value = Value::Bool(true);

/// A schema and the root its `$ref`s point into.
#[derive(Debug, Clone, Copy)]
pub(super) struct Schema<'a> {
    root: &'a Value,
    node: &'a Value,
}

impl<'a> Schema<'a> {
    /// The root schema itself.
    pub(super) fn root(root: &'a Value) -> Schema<'a> {
        Schema { root, node: root }
    }

    /// A schema inside this one's root.
    pub(super) fn at(self, node: &'a Value) -> Schema<'a> {
        Schema { root: self.root, node }
    }

    /// The schema a `$ref` points at, through chains of them: `None` for one that points nowhere,
    /// or a chain longer than [`MAX_DEPTH`].
    pub(super) fn resolved(self) -> Option<Schema<'a>> {
        let mut node = self.node;
        for _ in 0..MAX_DEPTH {
            match node.get("$ref").and_then(Value::as_str) {
                Some(pointer) => node = self.root.pointer(pointer.strip_prefix('#')?)?,
                None => return Some(self.at(node)),
            }
        }
        None
    }

    pub(super) fn get(self, key: &str) -> Option<&'a Value> {
        self.node.get(key)
    }

    /// The `properties`, in the schema's order.
    pub(super) fn properties(self) -> Vec<(&'a String, Schema<'a>)> {
        self.get("properties").and_then(Value::as_object).into_iter().flatten().map(|(name, node)| (name, self.at(node))).collect()
    }

    /// The `required` property names.
    pub(super) fn required(self) -> Vec<&'a str> {
        self.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect()
    }
}

/// What a value is generated for: one plain value, one with everything filled, or every edge value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Want {
    Plain,
    Full,
    Edges,
}

/// The valid calls of a command whose params schema is `schema`: the required params filled plainly,
/// then each property in turn with each of its edge values (the required others filled plainly),
/// then every property filled. None is over half the request cap. `Err` names a required param no
/// value can be built for.
pub(super) fn valid_params(schema: &Value) -> Result<Vec<Value>, String> {
    let top = Schema::root(schema).resolved().ok_or("the params schema has a `$ref` that points nowhere")?;
    let base = required_filled(top)?;
    let mut calls = vec![Value::Object(base.clone())];
    for (name, property) in top.properties() {
        for value in generate(property, 1, Want::Edges) {
            let mut params = base.clone();
            params.insert(name.clone(), value);
            calls.push(Value::Object(params));
        }
    }
    if top.properties().is_empty() {
        calls.extend(generate(top, 0, Want::Edges).into_iter().filter(Value::is_object));
    }
    calls.extend(generate(top, 0, Want::Full).into_iter().filter(Value::is_object));
    let mut kept: Vec<Value> = Vec::new();
    for params in calls.into_iter().filter(|p| p.to_string().len() <= REQUEST_CAP / 2) {
        if kept.len() < PER_COMMAND && !kept.contains(&params) {
            kept.push(params);
        }
    }
    Ok(kept)
}

/// The required properties of the object schema `top`, each with a plain value: `Err` names one
/// that has none.
pub(super) fn required_filled(top: Schema<'_>) -> Result<Map<String, Value>, String> {
    let properties = top.properties();
    let mut filled = Map::new();
    for name in top.required() {
        let Some((_, property)) = properties.iter().find(|(n, _)| n.as_str() == name) else {
            return Err(format!("`{name}` is required but the schema has no property for it"));
        };
        let value = plain(*property).ok_or_else(|| format!("no value of `{name}`'s schema can be built"))?;
        filled.insert(name.to_owned(), value);
    }
    Ok(filled)
}

/// One plain value of `schema`, if one can be built.
pub(super) fn plain(schema: Schema<'_>) -> Option<Value> {
    generate(schema, 1, Want::Plain).pop()
}

/// The values of `schema` that `want` asks for: at most one for `Plain` and `Full`.
fn generate(schema: Schema<'_>, depth: usize, want: Want) -> Vec<Value> {
    if depth > MAX_DEPTH {
        return Vec::new();
    }
    let Some(s) = schema.resolved() else { return Vec::new() };
    let want = if want == Want::Full && depth > FULL_DEPTH { Want::Plain } else { want };
    let values = match s.node {
        Value::Bool(true) => any(),
        Value::Bool(false) => Vec::new(),
        _ => match (s.get("const"), s.get("enum").and_then(Value::as_array)) {
            (Some(constant), _) => vec![constant.clone()],
            (None, Some(listed)) => listed.clone(),
            (None, None) => composed(s, depth, want),
        },
    };
    pick(values, want)
}

/// The values of a schema that is `anyOf`/`oneOf`/`allOf` branches, or of its `type`s.
fn composed(s: Schema<'_>, depth: usize, want: Want) -> Vec<Value> {
    if let Some(branches) = s.get("anyOf").or_else(|| s.get("oneOf")).and_then(Value::as_array) {
        // A plain value of `Option<T>` is a `T`: the null branch goes last.
        let mut ordered: Vec<&Value> = branches.iter().collect();
        ordered.sort_by_key(|b| b.get("type").and_then(Value::as_str) == Some("null"));
        return ordered.into_iter().flat_map(|b| generate(s.at(b), depth + 1, want)).collect();
    }
    if let Some(all) = s.get("allOf").and_then(Value::as_array) {
        return all_of(s, all, depth, want);
    }
    let mut values = Vec::new();
    for kind in types(s) {
        values.extend(of_type(s, kind, depth, want));
        if want != Want::Edges && !values.is_empty() {
            break;
        }
    }
    values
}

/// `allOf`: one branch is that branch; several are their properties together, as one object.
fn all_of(s: Schema<'_>, all: &[Value], depth: usize, want: Want) -> Vec<Value> {
    if let [one] = all {
        return generate(s.at(one), depth + 1, want);
    }
    let mut properties = Map::new();
    let mut required = Vec::new();
    for branch in all.iter().filter_map(|b| s.at(b).resolved()) {
        properties.extend(branch.properties().into_iter().map(|(name, p)| (name.clone(), p.node.clone())));
        required.extend(branch.required().into_iter().map(|r| json!(r)));
    }
    let merged = json!({"type": "object", "properties": properties, "required": required});
    generate(Schema { root: s.root, node: &merged }, depth + 1, want)
}

/// The JSON types `s` allows, `null` last; a schema without a `type` is told by its keywords.
fn types(s: Schema<'_>) -> Vec<&str> {
    let mut kinds: Vec<&str> = match s.get("type") {
        Some(Value::String(one)) => vec![one.as_str()],
        Some(Value::Array(many)) => many.iter().filter_map(Value::as_str).collect(),
        _ if s.get("properties").is_some() || s.get("additionalProperties").is_some() => vec!["object"],
        _ if s.get("items").is_some() || s.get("prefixItems").is_some() => vec!["array"],
        _ => vec!["any"],
    };
    kinds.sort_by_key(|k| *k == "null");
    kinds
}

fn of_type(s: Schema<'_>, kind: &str, depth: usize, want: Want) -> Vec<Value> {
    match kind {
        "null" => vec![Value::Null],
        "boolean" => vec![json!(true), json!(false)],
        "string" => strings(s).into_iter().map(Value::String).collect(),
        "integer" => integers(s),
        "number" => numbers(s),
        "array" => arrays(s, depth, want),
        "object" => objects(s, depth, want),
        _ => any(),
    }
}

/// The first value for `Plain` and `Full`, the first [`PER_SCHEMA`] for `Edges`.
fn pick(mut values: Vec<Value>, want: Want) -> Vec<Value> {
    values.truncate(if want == Want::Edges { PER_SCHEMA } else { 1 });
    values
}

/// Values of a schema that takes any JSON (a `serde_json::Value` param): every shape, and a deep one.
fn any() -> Vec<Value> {
    let mut deep = json!("conformance");
    for _ in 0..DEEP {
        deep = json!({ "k": [deep] });
    }
    vec![json!("conformance"), Value::Null, json!(true), json!(0), json!(-1), json!(""), json!([]), json!({}), deep]
}

/// A non-negative integer keyword of `s`.
fn size(s: Schema<'_>, key: &str) -> Option<usize> {
    s.get(key).and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok())
}

/// Strings of `s`'s `format` (one the run knows), or else plain, empty, huge, unicode and path
/// tricks: the ones within its `minLength` and `maxLength`, with strings of those lengths.
fn strings(s: Schema<'_>) -> Vec<String> {
    // A format's strings are what its type parses: the lengths are its own.
    if let Some(of_format) = s.get("format").and_then(Value::as_str).and_then(formatted) {
        return of_format;
    }
    let (min, max) = (size(s, "minLength").unwrap_or(0), size(s, "maxLength").unwrap_or(usize::MAX));
    let mut fits: Vec<String> = free_strings().into_iter().filter(|c| (min..=max).contains(&c.chars().count())).collect();
    for length in [min, max] {
        if length <= HUGE && !fits.iter().any(|f| f.chars().count() == length) {
            fits.push("x".repeat(length));
        }
    }
    fits
}

/// Strings a hostile caller sends: the plain one first.
fn free_strings() -> Vec<String> {
    [
        "conformance",
        "",
        "x",
        "\u{0}",
        "\u{1f600}",
        "\u{0}\u{202e}\u{fffd} \u{fc}n\u{ef}c\u{f6}d\u{e9} \u{1f600}",
        " padded ",
        "line\nbreak\r\n",
        "../../../../conformance-escape/x",
        "/conformance-escape/x",
        "C:\\conformance-escape\\x",
        "\\\\?\\C:\\conformance-escape\\x",
        "~/conformance-escape",
        "con",
        "%s%n{}{{0}}$(x)`x`",
    ]
    .into_iter()
    .map(str::to_owned)
    .chain(std::iter::once("x".repeat(HUGE)))
    .collect()
}

/// Strings of a `format` that a parsing type (a date, a UUID, an address) accepts.
fn formatted(format: &str) -> Option<Vec<String>> {
    let list: &[&str] = match format {
        "date-time" => &["2026-01-01T00:00:00Z", "1970-01-01T00:00:00Z", "9999-12-31T23:59:59.999999999+14:00"],
        "date" => &["2026-01-01", "1970-01-01", "9999-12-31"],
        "time" => &["00:00:00", "23:59:59.999"],
        "uuid" => &["00000000-0000-0000-0000-000000000000", "ffffffff-ffff-ffff-ffff-ffffffffffff"],
        "uri" | "url" | "uri-reference" => &["https://example.invalid/", "file:///conformance-escape"],
        "email" => &["conformance@example.invalid"],
        "ipv4" => &["127.0.0.1", "0.0.0.0", "255.255.255.255"],
        "ipv6" => &["::1", "::"],
        "hostname" => &["localhost"],
        _ => return None,
    };
    Some(list.iter().map(|s| (*s).to_owned()).collect())
}

/// The range a JSON integer of `format` holds (what serde_json can carry, at most).
fn format_range(format: Option<&str>) -> (i128, i128) {
    let json = (i128::from(i64::MIN), i128::from(u64::MAX));
    let (lo, hi) = match format {
        Some("int8") => (i128::from(i8::MIN), i128::from(i8::MAX)),
        Some("int16") => (i128::from(i16::MIN), i128::from(i16::MAX)),
        Some("int32") => (i128::from(i32::MIN), i128::from(i32::MAX)),
        Some("int64" | "int") => (i128::from(i64::MIN), i128::from(i64::MAX)),
        Some("uint8") => (0, i128::from(u8::MAX)),
        Some("uint16") => (0, i128::from(u16::MAX)),
        Some("uint32") => (0, i128::from(u32::MAX)),
        Some("uint64" | "uint" | "uint128") => (0, i128::from(u64::MAX)),
        _ => json,
    };
    (lo.max(json.0), hi.min(json.1))
}

/// A bound keyword of `s` as a whole number: `up` rounds a fraction up, else down.
fn bound(s: Schema<'_>, key: &str, up: bool) -> Option<i128> {
    let n = s.get(key)?;
    if let Some(i) = n.as_i64() {
        return Some(i128::from(i));
    }
    if let Some(u) = n.as_u64() {
        return Some(i128::from(u));
    }
    let f = n.as_f64()?;
    let rounded = if up { f.ceil() } else { f.floor() };
    // Saturating: a bound past i128 is no bound within JSON's range.
    Some(rounded as i128)
}

/// The range `s` allows an integer: its `format`'s, narrowed by `minimum`, `maximum` and their
/// exclusive forms.
fn integer_range(s: Schema<'_>) -> (i128, i128) {
    let (mut lo, mut hi) = format_range(s.get("format").and_then(Value::as_str));
    if let Some(min) = bound(s, "minimum", true) {
        lo = lo.max(min);
    }
    if let Some(min) = bound(s, "exclusiveMinimum", false) {
        lo = lo.max(min.saturating_add(1));
    }
    if let Some(max) = bound(s, "maximum", false) {
        hi = hi.min(max);
    }
    if let Some(max) = bound(s, "exclusiveMaximum", true) {
        hi = hi.min(max.saturating_sub(1));
    }
    (lo, hi)
}

/// Integers: 0, 1, -1 and the ends of the range, those within it.
fn integers(s: Schema<'_>) -> Vec<Value> {
    let (lo, hi) = integer_range(s);
    let mut picked: Vec<i128> = Vec::new();
    for n in [0, 1, -1, lo, hi, lo.saturating_add(1), hi.saturating_sub(1)] {
        if (lo..=hi).contains(&n) && !picked.contains(&n) {
            picked.push(n);
        }
    }
    picked.into_iter().filter_map(|n| i64::try_from(n).map(Value::from).ok().or_else(|| u64::try_from(n).map(Value::from).ok())).collect()
}

/// Numbers: a fraction, zero, a negative, the largest and smallest, those within `s`'s bounds.
fn numbers(s: Schema<'_>) -> Vec<Value> {
    let big = if s.get("format").and_then(Value::as_str) == Some("float") { f64::from(f32::MAX) } else { f64::MAX };
    let lo = s.get("minimum").and_then(Value::as_f64).unwrap_or(f64::NEG_INFINITY);
    let hi = s.get("maximum").and_then(Value::as_f64).unwrap_or(f64::INFINITY);
    let mut values: Vec<Value> = [0.5, 0.0, -1.5, big, -big, f64::MIN_POSITIVE, lo, hi]
        .into_iter()
        .filter(|n| n.is_finite() && (lo..=hi).contains(n))
        .filter_map(|n| Number::from_f64(n).map(Value::Number))
        .collect();
    values.dedup();
    values
}

/// Arrays: a tuple's items in order; else empty, one item, one full item and [`LARGE`] items,
/// those within `minItems` and `maxItems`.
fn arrays(s: Schema<'_>, depth: usize, want: Want) -> Vec<Value> {
    if let Some(Value::Array(tuple)) = s.get("prefixItems").or_else(|| s.get("items").filter(|i| i.is_array())) {
        let inner = if want == Want::Edges { Want::Plain } else { want };
        let items: Option<Vec<Value>> = tuple.iter().map(|item| generate(s.at(item), depth + 1, inner).pop()).collect();
        return items.map(Value::Array).into_iter().collect();
    }
    let (min, max) = (size(s, "minItems").unwrap_or(0), size(s, "maxItems").unwrap_or(usize::MAX));
    let item = s.get("items").map_or_else(|| Schema { root: s.root, node: &ANY }, |i| s.at(i));
    let one = |want: Want| generate(item, depth + 1, want).pop();
    let mut values = Vec::new();
    let (plain, full) = (one(Want::Plain), one(Want::Full));
    let lengths: &[(usize, Option<&Value>)] = match want {
        Want::Plain => &[(min, plain.as_ref())],
        Want::Full => &[(min.max(1), full.as_ref())],
        Want::Edges => &[(0, None), (1, plain.as_ref()), (min.max(1), full.as_ref()), (LARGE, plain.as_ref())],
    };
    for (length, value) in lengths {
        let fits = (min..=max).contains(length) && *length <= LARGE;
        match value {
            _ if !fits => {}
            _ if *length == 0 => values.push(json!([])),
            Some(value) => values.push(Value::Array(vec![(*value).clone(); *length])),
            None => {}
        }
    }
    values
}

/// Objects: the required properties filled plainly, or every one filled; for `Edges` also a large
/// map and one with tricky keys, when the schema takes keys it doesn't list.
fn objects(s: Schema<'_>, depth: usize, want: Want) -> Vec<Value> {
    let plain = filled(s, depth, Want::Plain);
    match want {
        Want::Plain => plain.into_iter().collect(),
        Want::Full => filled(s, depth, Want::Full).or(plain).into_iter().collect(),
        Want::Edges => {
            let mut values: Vec<Value> = plain.into_iter().chain(filled(s, depth, Want::Full)).collect();
            values.extend(maps(s, depth));
            values
        }
    }
}

/// The object with its required properties (`Plain`) or every property (`Full`) given a value:
/// `None` when a required one has none.
fn filled(s: Schema<'_>, depth: usize, want: Want) -> Option<Value> {
    let required = s.required();
    let mut object = Map::new();
    for (name, property) in s.properties() {
        let needed = required.contains(&name.as_str());
        if !needed && want == Want::Plain {
            continue;
        }
        match generate(property, depth + 1, want).pop() {
            Some(value) => {
                object.insert(name.clone(), value);
            }
            None if needed => return None,
            None => {}
        }
    }
    Some(Value::Object(object))
}

/// Maps of a schema whose `additionalProperties` is a schema: [`LARGE`] keys, and tricky keys.
fn maps(s: Schema<'_>, depth: usize) -> Vec<Value> {
    let Some(extra) = s.get("additionalProperties").filter(|a| !matches!(a, Value::Bool(false))) else { return Vec::new() };
    let Some(value) = generate(s.at(extra), depth + 1, Want::Plain).pop() else { return Vec::new() };
    let Some(Value::Object(base)) = filled(s, depth, Want::Plain) else { return Vec::new() };
    let with = |keys: Vec<String>| {
        let mut map = base.clone();
        map.extend(keys.into_iter().map(|k| (k, value.clone())));
        Value::Object(map)
    };
    let tricky = ["", "../x", "\u{0}", "\u{1f600}", "__proto__"].into_iter().map(str::to_owned).collect();
    vec![with((0..LARGE).map(|n| format!("k{n}")).collect()), with(tricky)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edges(schema: Value) -> Vec<Value> {
        generate(Schema::root(&schema), 1, Want::Edges)
    }

    #[test]
    fn integers_stay_within_their_format_and_bounds() {
        assert_eq!(edges(json!({"type": "integer", "format": "uint8", "minimum": 0})), [json!(0), json!(1), json!(255), json!(254)]);
        let i32s = edges(json!({"type": "integer", "format": "int32"}));
        assert!(i32s.contains(&json!(i32::MIN)) && i32s.contains(&json!(i32::MAX)) && i32s.contains(&json!(-1)), "{i32s:?}");
        assert!(edges(json!({"type": "integer"})).contains(&json!(u64::MAX)));
        assert_eq!(edges(json!({"type": "integer", "minimum": 3, "exclusiveMaximum": 5})), [json!(3), json!(4)]);
    }

    #[test]
    fn strings_cover_the_hostile_ones_and_keep_to_their_length() {
        let all = edges(json!({"type": "string"}));
        assert!(all.contains(&json!("")) && all.iter().any(|s| s.as_str().is_some_and(|s| s.len() == HUGE)), "{}", all.len());
        assert!(all.iter().any(|s| s.as_str().is_some_and(|s| s.contains(".."))));
        let chars = edges(json!({"type": "string", "minLength": 1, "maxLength": 1}));
        assert!(!chars.is_empty() && chars.iter().all(|c| c.as_str().is_some_and(|c| c.chars().count() == 1)), "{chars:?}");
        assert_eq!(edges(json!({"type": "string", "format": "uuid"})).len(), 2);
    }

    #[test]
    fn refs_options_enums_and_consts_are_followed() {
        let root = json!({
            "type": "object",
            "properties": {
                "inner": {"$ref": "#/$defs/Inner"},
                "maybe": {"anyOf": [{"$ref": "#/$defs/Inner"}, {"type": "null"}]},
                "mode": {"enum": ["a", "b"]},
                "tag": {"const": "x"}
            },
            "required": ["inner", "mode", "tag"],
            "$defs": {"Inner": {"type": "object", "properties": {"n": {"type": "integer", "format": "uint8"}}, "required": ["n"]}}
        });
        let calls = valid_params(&root).unwrap();
        assert_eq!(calls[0], json!({"inner": {"n": 0}, "mode": "a", "tag": "x"}), "the required params, plainly");
        assert!(calls.contains(&json!({"inner": {"n": 0}, "mode": "a", "tag": "x", "maybe": null})));
        assert!(calls.contains(&json!({"inner": {"n": 0}, "mode": "b", "tag": "x"})));
        assert!(calls.iter().all(|c| c.to_string().len() <= REQUEST_CAP / 2));
    }

    #[test]
    fn arrays_and_maps_are_empty_large_and_deep() {
        let arrays = edges(json!({"type": "array", "items": {"type": "integer", "format": "uint8"}}));
        assert!(arrays.contains(&json!([])) && arrays.iter().any(|a| a.as_array().is_some_and(|a| a.len() == LARGE)));
        let maps = edges(json!({"type": "object", "additionalProperties": {"type": "string"}}));
        assert!(maps.iter().any(|m| m.as_object().is_some_and(|m| m.len() == LARGE)) && maps.iter().any(|m| m.get("../x").is_some()));
        assert!(edges(json!(true)).iter().any(|v| v.to_string().len() > DEEP * 5), "a free-form param gets a deep value");
        let tree = json!({"$ref": "#/$defs/T", "$defs": {"T": {"type": "object", "properties": {"kids": {"type": "array", "items": {"$ref": "#/$defs/T"}}}}}});
        let full = generate(Schema::root(&tree), 0, Want::Full).pop().unwrap();
        assert!(full.to_string().matches("kids").count() >= 3, "a recursive type is filled some levels deep: {full}");
    }

    #[test]
    fn a_required_param_that_cannot_be_built_is_said_by_name() {
        let impossible = json!({"type": "object", "properties": {"x": false}, "required": ["x"]});
        assert!(valid_params(&impossible).unwrap_err().contains("`x`"));
        let missing = json!({"type": "object", "properties": {}, "required": ["y"]});
        assert!(valid_params(&missing).unwrap_err().contains("`y`"));
        let self_needing =
            json!({"$ref": "#/$defs/L", "$defs": {"L": {"type": "object", "properties": {"next": {"$ref": "#/$defs/L"}}, "required": ["next"]}}});
        assert!(valid_params(&self_needing).is_err(), "an infinite required chain has no value");
    }
}
