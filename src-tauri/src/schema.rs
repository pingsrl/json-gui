use crate::json_index::{JsonIndex, NodeKind};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write;

// ─── Schema (inferred type tree) ─────────────────────────────────────────────

#[derive(Debug, Clone)]
enum Schema {
    Str,
    Num,
    Bool,
    Null,
    Any,
    Arr(Box<Schema>),
    /// (key, type, optional)
    Obj(Vec<(String, Schema, bool)>),
}

const SAMPLE_LIMIT: usize = 50;

fn infer_node(index: &JsonIndex, id: u32) -> Schema {
    let node = &index.nodes[id as usize];
    match node.kind() {
        NodeKind::Str  => Schema::Str,
        NodeKind::Num  => Schema::Num,
        NodeKind::Bool => Schema::Bool,
        NodeKind::Null => Schema::Null,
        NodeKind::Array => {
            let ch = index.get_children_slice(id);
            let n = ch.len().min(SAMPLE_LIMIT);
            if n == 0 {
                Schema::Arr(Box::new(Schema::Any))
            } else {
                Schema::Arr(Box::new(merge_elements(index, &ch[..n])))
            }
        }
        NodeKind::Object => {
            let ch = index.get_children_slice(id);
            let fields = ch
                .iter()
                .map(|&cid| {
                    let key = index.nodes[cid as usize].key()
                        .map_or_else(String::new, |k| index.keys.get(k).to_string());
                    (key, infer_node(index, cid), false)
                })
                .collect();
            Schema::Obj(fields)
        }
    }
}

fn merge_elements(index: &JsonIndex, ids: &[u32]) -> Schema {
    let all_objs = ids
        .iter()
        .all(|&id| index.nodes[id as usize].kind() == NodeKind::Object);
    if all_objs {
        return merge_object_array(index, ids);
    }
    ids.iter()
        .map(|&id| infer_node(index, id))
        .reduce(merge_schemas)
        .unwrap_or(Schema::Any)
}

fn merge_object_array(index: &JsonIndex, ids: &[u32]) -> Schema {
    let total = ids.len();
    let mut fields: BTreeMap<String, (Vec<Schema>, usize)> = BTreeMap::new();
    for &oid in ids {
        for cid in index.children_iter(oid) {
            let key = index.nodes[cid as usize].key()
                .map_or_else(String::new, |k| index.keys.get(k).to_string());
            let e = fields.entry(key).or_default();
            e.0.push(infer_node(index, cid));
            e.1 += 1;
        }
    }
    let result = fields
        .into_iter()
        .map(|(key, (schemas, count))| {
            let opt = count < total;
            let merged = schemas
                .into_iter()
                .reduce(merge_schemas)
                .unwrap_or(Schema::Any);
            (key, merged, opt)
        })
        .collect();
    Schema::Obj(result)
}

fn merge_schemas(a: Schema, b: Schema) -> Schema {
    match (a, b) {
        (Schema::Any, x) | (x, Schema::Any) => x,
        (Schema::Str, Schema::Str) => Schema::Str,
        (Schema::Num, Schema::Num) => Schema::Num,
        (Schema::Bool, Schema::Bool) => Schema::Bool,
        (Schema::Null, Schema::Null) => Schema::Null,
        (Schema::Arr(a), Schema::Arr(b)) => Schema::Arr(Box::new(merge_schemas(*a, *b))),
        (Schema::Obj(a), Schema::Obj(b)) => merge_obj_fields(a, b),
        _ => Schema::Any,
    }
}

fn merge_obj_fields(a: Vec<(String, Schema, bool)>, b: Vec<(String, Schema, bool)>) -> Schema {
    let mut map: BTreeMap<String, (Schema, bool)> = BTreeMap::new();
    let b_keys: HashSet<String> = b.iter().map(|(k, _, _)| k.clone()).collect();

    for (k, s, opt) in a {
        let missing_in_b = !b_keys.contains(&k);
        map.insert(k, (s, opt || missing_in_b));
    }
    for (k, s, opt) in b {
        if let Some(entry) = map.get_mut(&k) {
            entry.0 = merge_schemas(entry.0.clone(), s);
            entry.1 = entry.1 || opt;
        } else {
            map.insert(k, (s, true));
        }
    }
    Schema::Obj(map.into_iter().map(|(k, (s, opt))| (k, s, opt)).collect())
}

// ─── TypeRef (named type references) ─────────────────────────────────────────

#[derive(Clone, Debug)]
enum TypeRef {
    Str,
    Num,
    Bool,
    Null,
    Any,
    Arr(Box<TypeRef>),
    Ref(String),
}

struct NamedObj {
    name: String,
    fields: Vec<(String, TypeRef, bool)>,
}

// ─── Namer ───────────────────────────────────────────────────────────────────

struct Namer {
    used: HashMap<String, u32>,
}

impl Namer {
    fn new() -> Self {
        Self {
            used: HashMap::new(),
        }
    }
    fn next(&mut self, hint: &str) -> String {
        let base = to_pascal(hint);
        let n = self.used.entry(base.clone()).or_insert(0);
        *n += 1;
        if *n == 1 {
            base
        } else {
            format!("{}{}", base, n)
        }
    }
}

fn to_pascal(s: &str) -> String {
    let base: String = s
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect();
    if base.is_empty() {
        return "Type".to_string();
    }
    if base.starts_with(|c: char| c.is_numeric()) {
        format!("T{}", base)
    } else {
        base
    }
}

fn singularize(s: &str) -> String {
    if s.ends_with("ies") && s.len() > 4 {
        format!("{}y", &s[..s.len() - 3])
    } else if s.ends_with("es") && s.len() > 2 {
        s[..s.len() - 2].to_string()
    } else if s.ends_with('s') && s.len() > 1 {
        s[..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

fn collect_schema(
    schema: &Schema,
    hint: &str,
    out: &mut Vec<NamedObj>,
    namer: &mut Namer,
) -> TypeRef {
    match schema {
        Schema::Str => TypeRef::Str,
        Schema::Num => TypeRef::Num,
        Schema::Bool => TypeRef::Bool,
        Schema::Null => TypeRef::Null,
        Schema::Any => TypeRef::Any,
        Schema::Arr(elem) => {
            let elem_ref = collect_schema(elem, &singularize(hint), out, namer);
            TypeRef::Arr(Box::new(elem_ref))
        }
        Schema::Obj(fields) => {
            let name = namer.next(hint);
            let mut resolved = vec![];
            for (key, s, opt) in fields {
                let tr = collect_schema(s, key, out, namer);
                resolved.push((key.clone(), tr, *opt));
            }
            out.push(NamedObj {
                name: name.clone(),
                fields: resolved,
            });
            TypeRef::Ref(name)
        }
    }
}

fn build_named_types(index: &JsonIndex) -> (Vec<NamedObj>, TypeRef) {
    let root_ch = index.get_children_slice(index.root);
    let schema = match root_ch.len() {
        0 => Schema::Any,
        1 => infer_node(index, root_ch[0]),
        _ => Schema::Arr(Box::new(merge_elements(
            index,
            &root_ch[..root_ch.len().min(SAMPLE_LIMIT)],
        ))),
    };
    let mut out = vec![];
    let mut namer = Namer::new();
    let root_ref = collect_schema(&schema, "Root", &mut out, &mut namer);
    (out, root_ref)
}

// ─── Common helpers ───────────────────────────────────────────────────────────

fn is_identifier(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut ch = s.chars();
    let first = ch.next().unwrap();
    (first.is_alphabetic() || first == '_' || first == '$')
        && ch.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

fn to_snake(s: &str) -> String {
    let mut result = String::new();
    let mut prev_upper = false;
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 && !prev_upper {
                result.push('_');
            }
            result.extend(c.to_lowercase());
            prev_upper = true;
        } else if !c.is_alphanumeric() {
            result.push('_');
            prev_upper = false;
        } else {
            result.push(c);
            prev_upper = false;
        }
    }
    // Remove consecutive underscores
    let deduped: String = result.chars().fold(String::new(), |mut acc, c| {
        if c == '_' && acc.ends_with('_') {
        } else {
            acc.push(c);
        }
        acc
    });
    let trimmed = deduped.trim_matches('_').to_string();
    // Rust reserved words
    const RUST_KW: &[&str] = &[
        "type", "fn", "struct", "enum", "match", "use", "mod", "impl", "pub", "crate", "self",
        "super", "where", "let", "mut", "ref", "move", "dyn", "trait", "for", "in", "if", "else",
        "loop", "while", "return", "break", "continue", "as", "const", "static", "unsafe",
        "extern", "box", "yield", "async", "await", "try", "abstract", "become", "do", "final",
        "macro", "override", "priv", "typeof", "unsized", "virtual",
    ];
    if RUST_KW.contains(&trimmed.as_str()) {
        format!("r#{}", trimmed)
    } else if trimmed.is_empty() {
        "field".to_string()
    } else {
        trimmed
    }
}

fn root_comment(root_ref: &TypeRef, lang: &str) -> Option<String> {
    match root_ref {
        TypeRef::Arr(_) => {
            let ts = format_typeref_ts(root_ref);
            match lang {
                "ts" => Some(format!("// The root JSON value is: {}\n", ts)),
                "rs" => Some(format!("// The root JSON value is: Vec<...>\n")),
                "go" => Some(format!("// The root JSON value is a slice\n")),
                "py" => Some(format!("# The root JSON value is a list\n")),
                _ => None,
            }
        }
        _ => None,
    }
}

fn format_typeref_ts(tr: &TypeRef) -> String {
    match tr {
        TypeRef::Str => "string".to_string(),
        TypeRef::Num => "number".to_string(),
        TypeRef::Bool => "boolean".to_string(),
        TypeRef::Null => "null".to_string(),
        TypeRef::Any => "unknown".to_string(),
        TypeRef::Arr(inner) => format!("{}[]", format_typeref_ts(inner)),
        TypeRef::Ref(name) => name.clone(),
    }
}

// ─── TypeScript ───────────────────────────────────────────────────────────────

pub fn generate_typescript(index: &JsonIndex) -> String {
    let (types, root_ref) = build_named_types(index);
    let mut out = String::new();

    if let Some(c) = root_comment(&root_ref, "ts") {
        out.push_str(&c);
    }
    if !matches!(&root_ref, TypeRef::Ref(_)) {
        writeln!(
            out,
            "export type Root = {};\n",
            format_typeref_ts(&root_ref)
        )
        .ok();
    }

    for obj in &types {
        writeln!(out, "export interface {} {{", obj.name).ok();
        for (key, tr, opt) in &obj.fields {
            let safe = if is_identifier(key) {
                key.clone()
            } else {
                format!("\"{}\"", key.replace('"', "\\\""))
            };
            let opt_mark = if *opt { "?" } else { "" };
            writeln!(out, "  {}{}: {};", safe, opt_mark, format_typeref_ts(tr)).ok();
        }
        writeln!(out, "}}\n").ok();
    }
    out
}

// ─── Zod ──────────────────────────────────────────────────────────────────────

fn format_typeref_zod(tr: &TypeRef, opt: bool) -> String {
    let inner = match tr {
        TypeRef::Str => "z.string()".to_string(),
        TypeRef::Num => "z.number()".to_string(),
        TypeRef::Bool => "z.boolean()".to_string(),
        TypeRef::Null => "z.null()".to_string(),
        TypeRef::Any => "z.unknown()".to_string(),
        TypeRef::Arr(inner) => format!("z.array({})", format_typeref_zod(inner, false)),
        TypeRef::Ref(name) => format!("{}Schema", name),
    };
    if opt {
        format!("{}.optional()", inner)
    } else {
        inner
    }
}

pub fn generate_zod(index: &JsonIndex) -> String {
    let (types, root_ref) = build_named_types(index);
    let mut out = String::new();
    writeln!(out, "import {{ z }} from \"zod\";\n").ok();

    for obj in &types {
        writeln!(out, "export const {}Schema = z.object({{", obj.name).ok();
        for (key, tr, opt) in &obj.fields {
            let safe = if is_identifier(key) {
                key.clone()
            } else {
                format!("\"{}\"", key.replace('"', "\\\""))
            };
            writeln!(out, "  {}: {},", safe, format_typeref_zod(tr, *opt)).ok();
        }
        writeln!(out, "}});\n").ok();
    }

    // Export inferred types
    for obj in &types {
        writeln!(
            out,
            "export type {} = z.infer<typeof {}Schema>;",
            obj.name, obj.name
        )
        .ok();
    }

    // Root alias if array
    if !matches!(&root_ref, TypeRef::Ref(_)) {
        writeln!(
            out,
            "\nexport const RootSchema = {};",
            format_typeref_zod(&root_ref, false)
        )
        .ok();
        writeln!(out, "export type Root = z.infer<typeof RootSchema>;").ok();
    }

    out
}

// ─── Rust ─────────────────────────────────────────────────────────────────────

fn format_typeref_rs(tr: &TypeRef, opt: bool) -> String {
    let inner = match tr {
        TypeRef::Str => "String".to_string(),
        TypeRef::Num => "f64".to_string(),
        TypeRef::Bool => "bool".to_string(),
        TypeRef::Null | TypeRef::Any => "sonic_rs::Value".to_string(),
        TypeRef::Arr(inner) => format!("Vec<{}>", format_typeref_rs(inner, false)),
        TypeRef::Ref(name) => name.clone(),
    };
    if opt {
        format!("Option<{}>", inner)
    } else {
        inner
    }
}

pub fn generate_rust(index: &JsonIndex) -> String {
    let (types, root_ref) = build_named_types(index);
    let mut out = String::new();
    writeln!(out, "use serde::{{Deserialize, Serialize}};\n").ok();

    if let Some(c) = root_comment(&root_ref, "rs") {
        out.push_str(&c);
    }

    for obj in &types {
        writeln!(out, "#[derive(Debug, Clone, Serialize, Deserialize)]").ok();
        writeln!(out, "pub struct {} {{", obj.name).ok();
        for (key, tr, opt) in &obj.fields {
            let field = to_snake(key);
            if field != *key {
                writeln!(
                    out,
                    "    #[serde(rename = \"{}\")]",
                    key.replace('"', "\\\"")
                )
                .ok();
            }
            if *opt {
                writeln!(
                    out,
                    "    #[serde(skip_serializing_if = \"Option::is_none\")]"
                )
                .ok();
            }
            writeln!(out, "    pub {}: {},", field, format_typeref_rs(tr, *opt)).ok();
        }
        writeln!(out, "}}\n").ok();
    }
    out
}

// ─── Go ───────────────────────────────────────────────────────────────────────

fn format_typeref_go(tr: &TypeRef, opt: bool) -> String {
    let inner = match tr {
        TypeRef::Str => "string".to_string(),
        TypeRef::Num => "float64".to_string(),
        TypeRef::Bool => "bool".to_string(),
        TypeRef::Null | TypeRef::Any => "interface{}".to_string(),
        TypeRef::Arr(inner) => format!("[]{}", format_typeref_go(inner, false)),
        TypeRef::Ref(name) => name.clone(),
    };
    if opt { format!("*{}", inner) } else { inner }
}

pub fn generate_go(index: &JsonIndex) -> String {
    let (types, root_ref) = build_named_types(index);
    let mut out = String::new();
    writeln!(out, "package main\n").ok();
    if let Some(c) = root_comment(&root_ref, "go") {
        out.push_str(&c);
    }

    for obj in &types {
        writeln!(out, "type {} struct {{", obj.name).ok();
        for (key, tr, opt) in &obj.fields {
            let field = to_pascal(key);
            let type_str = format_typeref_go(tr, *opt);
            let omit = if *opt { ",omitempty" } else { "" };
            writeln!(out, "\t{} {} `json:\"{}{}\"`", field, type_str, key, omit).ok();
        }
        writeln!(out, "}}\n").ok();
    }
    out
}

// ─── Python TypedDict ─────────────────────────────────────────────────────────

fn format_typeref_py(tr: &TypeRef, opt: bool) -> String {
    let inner = match tr {
        TypeRef::Str => "str".to_string(),
        TypeRef::Num => "float".to_string(),
        TypeRef::Bool => "bool".to_string(),
        TypeRef::Null => "None".to_string(),
        TypeRef::Any => "Any".to_string(),
        TypeRef::Arr(inner) => format!("list[{}]", format_typeref_py(inner, false)),
        TypeRef::Ref(name) => name.clone(),
    };
    if opt {
        format!("NotRequired[{}]", inner)
    } else {
        inner
    }
}

pub fn generate_python(index: &JsonIndex) -> String {
    let (types, root_ref) = build_named_types(index);
    let has_not_required = types
        .iter()
        .any(|t| t.fields.iter().any(|(_, _, opt)| *opt));

    let mut out = String::new();
    writeln!(out, "from __future__ import annotations").ok();
    let mut imports = vec!["Any"];
    if has_not_required {
        imports.push("NotRequired");
    }
    writeln!(out, "from typing import {}", imports.join(", ")).ok();
    writeln!(out, "from typing_extensions import TypedDict\n").ok();

    if let Some(c) = root_comment(&root_ref, "py") {
        out.push_str(&c);
    }

    for obj in &types {
        writeln!(out, "class {}(TypedDict):", obj.name).ok();
        if obj.fields.is_empty() {
            writeln!(out, "    pass").ok();
        } else {
            for (key, tr, opt) in &obj.fields {
                let field = to_snake(key);
                let type_str = format_typeref_py(tr, *opt);
                if field != *key {
                    writeln!(out, "    # original key: \"{}\"", key).ok();
                }
                writeln!(out, "    {}: {}", field, type_str).ok();
            }
        }
        writeln!(out).ok();
    }
    out
}

// ─── JSON Schema ──────────────────────────────────────────────────────────────

/// Writes `s` as a JSON string with minimal escaping.
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                use std::fmt::Write;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Returns the compact JSON schema as a string (no pretty-printing).
fn typeref_to_json_schema(tr: &TypeRef) -> String {
    match tr {
        TypeRef::Str => r#"{"type":"string"}"#.to_string(),
        TypeRef::Num => r#"{"type":"number"}"#.to_string(),
        TypeRef::Bool => r#"{"type":"boolean"}"#.to_string(),
        TypeRef::Null => r#"{"type":"null"}"#.to_string(),
        TypeRef::Any => "{}".to_string(),
        TypeRef::Arr(inner) => {
            format!(r#"{{"type":"array","items":{}}}"#, typeref_to_json_schema(inner))
        }
        TypeRef::Ref(name) => format!("{{\"$ref\":\"#/$defs/{}\"}}", name),
    }
}

pub fn generate_json_schema(index: &JsonIndex) -> String {
    let (types, root_ref) = build_named_types(index);

    // Build the JSON Schema as a compact string, then use sonic_rs for
    // pretty-printing (avoids any dependency on serde_json).
    let mut out = String::from(r#"{"$schema":"https://json-schema.org/draft/2020-12/schema""#);

    if !types.is_empty() {
        out.push_str(r#","$defs":{"#);
        for (i, obj) in types.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&json_str(&obj.name));
            out.push_str(r#":{"type":"object","properties":{"#);
            for (j, (key, tr, _opt)) in obj.fields.iter().enumerate() {
                if j > 0 {
                    out.push(',');
                }
                out.push_str(&json_str(key));
                out.push(':');
                out.push_str(&typeref_to_json_schema(tr));
            }
            out.push('}');
            let required: Vec<&str> = obj
                .fields
                .iter()
                .filter(|(_, _, opt)| !*opt)
                .map(|(key, _, _)| key.as_str())
                .collect();
            if !required.is_empty() {
                out.push_str(r#","required":["#);
                for (k, req) in required.iter().enumerate() {
                    if k > 0 {
                        out.push(',');
                    }
                    out.push_str(&json_str(req));
                }
                out.push(']');
            }
            out.push_str(r#","additionalProperties":false}"#);
        }
        out.push('}');
    }

    // Merge the root schema fields into the top-level object
    let root_schema = typeref_to_json_schema(&root_ref);
    let inner = &root_schema[1..root_schema.len() - 1]; // strip { }
    if !inner.is_empty() {
        out.push(',');
        out.push_str(inner);
    }
    out.push('}');

    // Pretty-print using sonic_rs
    sonic_rs::from_str::<sonic_rs::Value>(&out)
        .ok()
        .and_then(|v| sonic_rs::to_string_pretty(&v).ok())
        .unwrap_or(out)
}
