//! Turns the parsed Rojo grammar into JSON Schema documents.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};

use crate::{
    ir::{Container, Enum, Registry, Struct},
    ty::Ty,
};

pub struct Compiler<'a> {
    registry: &'a Registry,
    defs: BTreeMap<String, Value>,
    emitting: BTreeSet<String>,
}

impl<'a> Compiler<'a> {
    pub fn new(registry: &'a Registry) -> Self {
        Self {
            registry,
            defs: BTreeMap::new(),
            emitting: BTreeSet::new(),
        }
    }

    /// The definitions reached while compiling, ready for a `$defs` block.
    pub fn defs(&self) -> Map<String, Value> {
        self.defs
            .iter()
            .map(|(name, schema)| (name.clone(), schema.clone()))
            .collect()
    }

    /// Compiles a container by name and returns its schema inline.
    pub fn root(&mut self, name: &str) -> Result<Value> {
        self.container(name)
    }

    fn reference(&mut self, name: &str) -> Result<Value> {
        if !self.defs.contains_key(name) && !self.emitting.contains(name) {
            self.emitting.insert(name.to_owned());
            let schema = self.container(name);
            self.emitting.remove(name);
            self.defs.insert(name.to_owned(), schema?);
        }

        Ok(json!({ "$ref": format!("#/$defs/{name}") }))
    }

    fn container(&mut self, name: &str) -> Result<Value> {
        match self.registry.expect(name)? {
            Container::Struct(item) => self.structure(&item),
            Container::Enum(item) => self.enumeration(&item),
        }
    }

    fn structure(&mut self, item: &Struct) -> Result<Value> {
        let mut properties = Map::new();
        let mut required: Vec<Value> = Vec::new();
        let mut either: Vec<Value> = Vec::new();
        let mut catch_all = None;

        for field in &item.fields {
            if field.flatten {
                let Ty::Map(value) = &field.ty else {
                    bail!(
                        "`{}.{}` is flattened but is not a map, which this compiler cannot model",
                        item.name,
                        field.name
                    );
                };
                if catch_all.is_some() {
                    bail!("`{}` flattens more than one map", item.name);
                }
                catch_all = Some(self.schema(value)?);
                continue;
            }

            let schema = self
                .schema(&field.ty)
                .with_context(|| format!("compiling `{}.{}`", item.name, field.name))?;
            properties.insert(field.name.clone(), described(schema.clone(), &field.doc));

            for alias in &field.aliases {
                properties.insert(
                    alias.clone(),
                    described(
                        schema.clone(),
                        &format!("Alias of `{}`.\n\n{}", field.name, field.doc),
                    ),
                );
            }

            if !field.optional {
                if field.aliases.is_empty() {
                    required.push(Value::String(field.name.clone()));
                } else {
                    // serde accepts the field under any of its names, so the
                    // document only has to carry one of them.
                    either.push(json!({
                        "anyOf": std::iter::once(&field.name)
                            .chain(field.aliases.iter())
                            .map(|name| json!({ "required": [name] }))
                            .collect::<Vec<_>>(),
                    }));
                }
            }
        }

        let mut schema = Map::new();
        schema.insert("type".into(), json!("object"));
        schema.insert("properties".into(), Value::Object(properties));

        if !required.is_empty() {
            schema.insert("required".into(), Value::Array(required));
        }
        if !either.is_empty() {
            schema.insert("allOf".into(), Value::Array(either));
        }

        let additional = match (catch_all, item.deny_unknown_fields) {
            (Some(schema), _) => schema,
            (None, true) => Value::Bool(false),
            (None, false) => Value::Bool(true),
        };
        schema.insert("additionalProperties".into(), additional);

        Ok(described(Value::Object(schema), &item.doc))
    }

    fn enumeration(&mut self, item: &Enum) -> Result<Value> {
        if item.untagged {
            let mut branches = Vec::new();
            for variant in &item.variants {
                let Some(ty) = &variant.ty else {
                    bail!(
                        "untagged enum `{}` has a unit variant `{}`, which matches nothing",
                        item.name,
                        variant.name
                    );
                };
                branches.push(described(self.schema(ty)?, &variant.doc));
            }
            return Ok(described(json!({ "anyOf": branches }), &item.doc));
        }

        if item.variants.iter().any(|variant| variant.ty.is_some()) {
            bail!(
                "enum `{}` carries payloads without being untagged, which this compiler does not model",
                item.name
            );
        }

        let documented = item.variants.iter().any(|variant| !variant.doc.is_empty());
        let body = if documented {
            json!({
                "oneOf": item
                    .variants
                    .iter()
                    .map(|variant| described(json!({ "const": variant.name }), &variant.doc))
                    .collect::<Vec<_>>(),
            })
        } else {
            json!({
                "type": "string",
                "enum": item.variants.iter().map(|variant| variant.name.clone()).collect::<Vec<_>>(),
            })
        };

        Ok(described(body, &item.doc))
    }

    fn schema(&mut self, ty: &Ty) -> Result<Value> {
        match ty {
            // A bag of user-defined values, each one resolved the same way a
            // property is, so it leans on the container rather than on a leaf.
            Ty::Named(name) if name == "Attributes" => Ok(json!({
                "type": "object",
                "additionalProperties": self.reference("UnresolvedValue")?,
            })),
            Ty::Named(name) => match leaf(name) {
                Some(schema) => Ok(schema),
                None => self.reference(name),
            },
            // serde reads a missing key and an explicit null the same way, so
            // both have to be allowed.
            Ty::Option(inner) => Ok(nullable(self.schema(inner)?)),
            Ty::List(inner) => Ok(json!({ "type": "array", "items": self.schema(inner)? })),
            Ty::Set(inner) => Ok(json!({
                "type": "array",
                "items": self.schema(inner)?,
                "uniqueItems": true,
            })),
            Ty::Map(value) => Ok(json!({
                "type": "object",
                "additionalProperties": self.schema(value)?,
            })),
            Ty::Array(inner, len) => Ok(json!({
                "type": "array",
                "items": self.schema(inner)?,
                "minItems": len,
                "maxItems": len,
            })),
        }
    }
}

/// Types Rojo's grammar uses but does not define, mapped to what serde accepts.
fn leaf(name: &str) -> Option<Value> {
    let schema = match name {
        "String" | "str" | "PathBuf" | "Path" | "Ustr" => json!({ "type": "string" }),
        "bool" => json!({ "type": "boolean" }),
        "f32" | "f64" => json!({ "type": "number" }),
        "u8" => json!({ "type": "integer", "minimum": 0, "maximum": 255 }),
        "u16" => json!({ "type": "integer", "minimum": 0, "maximum": 65_535 }),
        "u32" => json!({ "type": "integer", "minimum": 0, "maximum": 4_294_967_295_u32 }),
        "u64" | "usize" => json!({ "type": "integer", "minimum": 0 }),
        "i8" | "i16" | "i32" | "i64" | "isize" => json!({ "type": "integer" }),
        "IpAddr" => json!({
            "type": "string",
            "description": "An IPv4 or IPv6 address.",
        }),
        "Glob" => json!({
            "type": "string",
            "description": "A glob pattern. `*` crosses path separators, so depth cannot be filtered with it.",
        }),
        "IgnorableGlob" => json!({
            "type": "string",
            "description": "A glob pattern. A leading `!` negates an earlier pattern, and order matters.",
        }),
        // Roblox value types, owned by rbx_types rather than by Rojo.
        "Variant" | "Font" | "MaterialColors" => json!({ "type": "object" }),
        _ => return None,
    };

    Some(schema)
}

/// Widens a schema so an explicit `null` is accepted alongside its own type.
fn nullable(schema: Value) -> Value {
    let Value::Object(mut object) = schema else {
        return json!({ "anyOf": [schema, { "type": "null" }] });
    };

    match object.get("type") {
        Some(Value::String(single)) => {
            let widened = json!([single, "null"]);
            object.insert("type".into(), widened);
            Value::Object(object)
        }
        _ => json!({ "anyOf": [Value::Object(object), { "type": "null" }] }),
    }
}

/// Attaches a Rojo doc comment as the schema description.
fn described(schema: Value, doc: &str) -> Value {
    let doc = doc.trim();
    if doc.is_empty() {
        return schema;
    }

    match schema {
        Value::Object(mut object) => {
            // A leaf may already carry a note of its own; the field's own words
            // come first because they are the more specific.
            let description = match object.remove("description") {
                Some(Value::String(existing)) => format!("{doc}\n\n{existing}"),
                _ => doc.to_owned(),
            };
            object.insert("description".into(), Value::String(description));
            Value::Object(object)
        }
        other => json!({ "allOf": [other], "description": doc }),
    }
}

/// Merges two struct schemas that describe the same file kind.
///
/// Used for `.meta.json`, which Rojo reads into one of two structs depending on
/// whether the file sits next to a file or inside a directory.
pub fn merge(left: &Value, right: &Value) -> Result<Value> {
    let mut merged = left
        .as_object()
        .context("merging a non-object schema")?
        .clone();
    let right = right.as_object().context("merging a non-object schema")?;

    let mut properties = merged
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    if let Some(extra) = right.get("properties").and_then(Value::as_object) {
        for (name, schema) in extra {
            match properties.get(name) {
                Some(existing) if existing != schema => {
                    bail!("`{name}` has two different shapes across the merged schemas")
                }
                Some(_) => {}
                None => {
                    properties.insert(name.clone(), schema.clone());
                }
            }
        }
    }

    if merged.get("required").is_some() || right.get("required").is_some() {
        bail!("merging schemas with required keys is not modelled");
    }

    merged.insert("properties".into(), Value::Object(properties));
    Ok(Value::Object(merged))
}
