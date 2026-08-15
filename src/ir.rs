//! Reads the vendored Rojo sources into a serde-aware intermediate form.
//!
//! Only the declarations matter here: what serde will accept from a JSON file,
//! and the doc comments Rojo's authors wrote next to them. Nothing in this
//! module executes or compiles Rojo, so the vendored files stay verbatim.

use std::collections::BTreeMap;

use anyhow::{bail, Context, Result};
use syn::{Attribute, Expr, Fields, Item, ItemEnum, ItemStruct, Lit, LitStr, Meta, Token};

use crate::ty::{self, Ty};

#[derive(Debug, Clone)]
pub enum Container {
    Struct(Struct),
    Enum(Enum),
}

impl Container {
    pub fn name(&self) -> &str {
        match self {
            Container::Struct(item) => &item.name,
            Container::Enum(item) => &item.name,
        }
    }

    pub fn doc(&self) -> &str {
        match self {
            Container::Struct(item) => &item.doc,
            Container::Enum(item) => &item.doc,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Struct {
    pub name: String,
    pub doc: String,
    /// serde rejects documents carrying keys it does not know.
    pub deny_unknown_fields: bool,
    pub fields: Vec<Field>,
}

impl Struct {
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|field| field.name == name)
    }
}

#[derive(Debug, Clone)]
pub struct Field {
    /// The key as it appears in JSON, after `rename` and `rename_all`.
    pub name: String,
    /// Additional keys serde also accepts for this field.
    pub aliases: Vec<String>,
    pub doc: String,
    pub ty: Ty,
    /// serde absorbs every unmatched key into this field.
    pub flatten: bool,
    /// The document may omit the key: the field is optional or has a default.
    pub optional: bool,
}

#[derive(Debug, Clone)]
pub struct Enum {
    pub name: String,
    pub doc: String,
    /// serde tries each variant in turn rather than looking for a tag.
    pub untagged: bool,
    pub variants: Vec<Variant>,
}

#[derive(Debug, Clone)]
pub struct Variant {
    /// The variant name as it appears in JSON, after `rename_all`.
    pub name: String,
    pub doc: String,
    /// The payload, if the variant is a newtype such as `Required(PathBuf)`.
    pub ty: Option<Ty>,
}

/// The declarations found in the vendored sources, converted on demand.
///
/// Rojo's files hold plenty of types that never reach a JSON file, and some of
/// them are shapes this compiler deliberately does not model. Conversion is
/// therefore lazy: an unsupported type is only an error once the grammar
/// actually reaches it.
#[derive(Debug, Default)]
pub struct Registry {
    items: BTreeMap<String, Declaration>,
}

#[derive(Debug, Clone)]
enum Declaration {
    Struct(ItemStruct),
    Enum(ItemEnum),
}

impl Registry {
    pub fn contains(&self, name: &str) -> bool {
        self.items.contains_key(name)
    }

    /// Looks a container up by name, failing loudly when it is gone.
    ///
    /// A missing root is how this compiler notices that a Rojo release moved or
    /// renamed part of the grammar, instead of quietly dropping it.
    pub fn expect(&self, name: &str) -> Result<Container> {
        let declaration = self.items.get(name).with_context(|| {
            format!(
                "`{name}` is not defined in the vendored Rojo sources. \
                 It was moved, renamed or removed upstream, so vendor.toml and \
                 the compiler need updating together."
            )
        })?;

        match declaration {
            Declaration::Struct(item) => Ok(Container::Struct(
                read_struct(item).with_context(|| format!("reading `{name}`"))?,
            )),
            Declaration::Enum(item) => Ok(Container::Enum(
                read_enum(item).with_context(|| format!("reading `{name}`"))?,
            )),
        }
    }

    pub fn expect_struct(&self, name: &str) -> Result<Struct> {
        match self.expect(name)? {
            Container::Struct(item) => Ok(item),
            Container::Enum(_) => bail!("`{name}` became an enum upstream"),
        }
    }

    pub fn expect_enum(&self, name: &str) -> Result<Enum> {
        match self.expect(name)? {
            Container::Enum(item) => Ok(item),
            Container::Struct(_) => bail!("`{name}` became a struct upstream"),
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn insert(&mut self, name: &str, declaration: Declaration) -> Result<()> {
        if self.items.insert(name.to_owned(), declaration).is_some() {
            bail!("two vendored files both define `{name}`, so the grammar is ambiguous");
        }
        Ok(())
    }
}

/// Indexes every deserializable declaration in the vendored files.
pub fn read(files: &[(String, String)]) -> Result<Registry> {
    let mut registry = Registry::default();

    for (path, source) in files {
        let file = syn::parse_file(source).with_context(|| format!("parsing vendor/{path}"))?;
        for item in &file.items {
            match item {
                // Newtype and unit structs carry no keys of their own.
                Item::Struct(item)
                    if derives_deserialize(&item.attrs)
                        && matches!(item.fields, Fields::Named(_)) =>
                {
                    registry.insert(&item.ident.to_string(), Declaration::Struct(item.clone()))?;
                }
                Item::Enum(item) if derives_deserialize(&item.attrs) => {
                    registry.insert(&item.ident.to_string(), Declaration::Enum(item.clone()))?;
                }
                _ => {}
            }
        }
    }

    Ok(registry)
}

fn read_struct(item: &ItemStruct) -> Result<Struct> {
    let container = serde_options(&item.attrs)?;
    let Fields::Named(named) = &item.fields else {
        bail!("`{}` has no named fields", item.ident);
    };

    let mut fields = Vec::new();
    for field in &named.named {
        let ident = field
            .ident
            .as_ref()
            .context("a named field has no identifier")?
            .to_string();
        let options = serde_options(&field.attrs)?;

        if options.skip {
            continue;
        }

        let ty = ty::parse(&field.ty).with_context(|| format!("reading field `{ident}`"))?;
        let name = options
            .rename
            .clone()
            .unwrap_or_else(|| rename(&ident, container.rename_all.as_deref()));

        fields.push(Field {
            optional: ty.is_optional() || options.default || options.flatten,
            name,
            aliases: options.aliases,
            doc: doc(&field.attrs),
            ty,
            flatten: options.flatten,
        });
    }

    Ok(Struct {
        name: item.ident.to_string(),
        doc: doc(&item.attrs),
        deny_unknown_fields: container.deny_unknown_fields,
        fields,
    })
}

fn read_enum(item: &ItemEnum) -> Result<Enum> {
    let options = serde_options(&item.attrs)?;
    let mut variants = Vec::new();

    for variant in &item.variants {
        let variant_options = serde_options(&variant.attrs)?;

        // A variant serde never deserializes cannot appear in a file. Rojo uses
        // this for the middlewares that only directories produce.
        if variant_options.skip || variant_options.skip_deserializing {
            continue;
        }

        let ty = match &variant.fields {
            Fields::Unit => None,
            Fields::Unnamed(unnamed) if unnamed.unnamed.len() == 1 => {
                let field = &unnamed.unnamed[0];
                Some(ty::parse(&field.ty).with_context(|| {
                    format!("reading variant `{}` of `{}`", variant.ident, item.ident)
                })?)
            }
            _ => bail!(
                "variant `{}` of `{}` has a shape this compiler does not model",
                variant.ident,
                item.ident
            ),
        };

        let ident = variant.ident.to_string();
        variants.push(Variant {
            name: variant_options
                .rename
                .clone()
                .unwrap_or_else(|| rename(&ident, options.rename_all.as_deref())),
            doc: doc(&variant.attrs),
            ty,
        });
    }

    Ok(Enum {
        name: item.ident.to_string(),
        doc: doc(&item.attrs),
        untagged: options.untagged,
        variants,
    })
}

/// The `#[serde(...)]` keys the vendored sources actually use.
///
/// One field per serde flag, deliberately: this mirrors an attribute list, and
/// collapsing the flags into states would hide which ones were seen.
#[derive(Debug, Default)]
#[allow(clippy::struct_excessive_bools)]
struct SerdeOptions {
    rename: Option<String>,
    rename_all: Option<String>,
    aliases: Vec<String>,
    default: bool,
    skip: bool,
    skip_deserializing: bool,
    flatten: bool,
    untagged: bool,
    deny_unknown_fields: bool,
}

fn serde_options(attrs: &[Attribute]) -> Result<SerdeOptions> {
    let mut options = SerdeOptions::default();

    for attr in attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }

        attr.parse_nested_meta(|meta| {
            let key = meta
                .path
                .get_ident()
                .map(ToString::to_string)
                .unwrap_or_default();

            match key.as_str() {
                "rename" => options.rename = Some(meta.value()?.parse::<LitStr>()?.value()),
                "rename_all" => options.rename_all = Some(meta.value()?.parse::<LitStr>()?.value()),
                "alias" => options
                    .aliases
                    .push(meta.value()?.parse::<LitStr>()?.value()),
                "skip" => options.skip = true,
                "skip_deserializing" => options.skip_deserializing = true,
                "flatten" => options.flatten = true,
                "untagged" => options.untagged = true,
                "deny_unknown_fields" => options.deny_unknown_fields = true,
                "default" => {
                    options.default = true;
                    if meta.input.peek(Token![=]) {
                        meta.value()?.parse::<LitStr>()?;
                    }
                }
                // Serialization-only knobs say nothing about what a file may
                // contain. Their values still have to be consumed.
                _ => {
                    if meta.input.peek(Token![=]) {
                        meta.value()?.parse::<Expr>()?;
                    }
                }
            }

            Ok(())
        })
        .with_context(|| "reading a #[serde(...)] attribute")?;
    }

    Ok(options)
}

fn derives_deserialize(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if !attr.path().is_ident("derive") {
            return false;
        }

        let mut found = false;
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("Deserialize") {
                found = true;
            }
            Ok(())
        });
        found
    })
}

fn doc(attrs: &[Attribute]) -> String {
    let mut lines: Vec<String> = Vec::new();

    for attr in attrs {
        let Meta::NameValue(value) = &attr.meta else {
            continue;
        };
        if !value.path.is_ident("doc") {
            continue;
        }
        let Expr::Lit(literal) = &value.value else {
            continue;
        };
        let Lit::Str(text) = &literal.lit else {
            continue;
        };

        let line = text.value();
        lines.push(line.strip_prefix(' ').unwrap_or(&line).to_owned());
    }

    // Doc comments wrap at the source margin, so single newlines are joined
    // back into paragraphs and blank lines are kept as paragraph breaks.
    let mut paragraphs: Vec<String> = Vec::new();
    let mut current: Vec<String> = Vec::new();

    for line in lines {
        if line.trim().is_empty() {
            if !current.is_empty() {
                paragraphs.push(current.join(" "));
                current.clear();
            }
        } else if line.starts_with('-') || line.starts_with('*') {
            // A list item keeps its own line.
            if !current.is_empty() {
                paragraphs.push(current.join(" "));
                current.clear();
            }
            paragraphs.push(line);
        } else {
            current.push(line.trim_end().to_owned());
        }
    }

    if !current.is_empty() {
        paragraphs.push(current.join(" "));
    }

    paragraphs.join("\n\n")
}

/// Applies serde's `rename_all` to a field or variant identifier.
fn rename(ident: &str, rule: Option<&str>) -> String {
    match rule {
        None => ident.to_owned(),
        Some("camelCase") => camel_case(ident),
        Some("snake_case") => snake_case(ident),
        Some("PascalCase") => {
            let camel = camel_case(ident);
            let mut chars = camel.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => camel,
            }
        }
        Some("lowercase") => ident.to_lowercase(),
        Some("UPPERCASE") => ident.to_uppercase(),
        // Rojo only uses the rules above. Anything else would silently produce
        // wrong key names, so it is a hard error.
        Some(other) => panic!("unsupported serde rename_all rule `{other}`"),
    }
}

fn camel_case(ident: &str) -> String {
    // Identifiers reach this function either in snake_case (fields) or already
    // in PascalCase (variants), and serde lowercases the leading word of both.
    let mut out = String::with_capacity(ident.len());
    let mut upper_next = false;

    for (index, ch) in ident.chars().enumerate() {
        if ch == '_' {
            upper_next = true;
        } else if index == 0 {
            out.extend(ch.to_lowercase());
        } else if upper_next {
            out.extend(ch.to_uppercase());
            upper_next = false;
        } else {
            out.push(ch);
        }
    }

    out
}

fn snake_case(ident: &str) -> String {
    let mut out = String::with_capacity(ident.len());

    for (index, ch) in ident.chars().enumerate() {
        if ch.is_uppercase() {
            if index != 0 {
                out.push('_');
            }
            out.extend(ch.to_lowercase());
        } else {
            out.push(ch);
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry(source: &str) -> Registry {
        read(&[("test.rs".to_owned(), source.to_owned())]).unwrap()
    }

    #[test]
    fn applies_rename_and_rename_all() {
        let registry = registry(
            r#"
            #[derive(Serialize, Deserialize)]
            #[serde(deny_unknown_fields, rename_all = "camelCase")]
            pub struct Project {
                #[serde(rename = "$schema")]
                schema: Option<String>,
                pub serve_port: Option<u16>,
                pub tree: ProjectNode,
                #[serde(skip)]
                pub file_location: PathBuf,
            }
            "#,
        );

        let item = registry.expect_struct("Project").unwrap();
        assert!(item.deny_unknown_fields);
        assert_eq!(item.fields.len(), 3);
        assert!(item.field("$schema").unwrap().optional);
        assert!(item.field("servePort").unwrap().optional);
        assert!(!item.field("tree").unwrap().optional);
        assert!(item.field("file_location").is_none());
    }

    #[test]
    fn drops_variants_serde_never_reads() {
        let registry = registry(
            r#"
            #[derive(Deserialize, Serialize)]
            #[serde(rename_all = "camelCase")]
            pub enum Middleware {
                JsonModel,
                #[serde(skip_deserializing)]
                Dir,
            }
            "#,
        );

        let item = registry.expect_enum("Middleware").unwrap();
        let names: Vec<_> = item.variants.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, ["jsonModel"]);
    }

    #[test]
    fn keeps_aliases_and_marks_defaults_optional() {
        let registry = registry(
            r#"
            #[derive(Deserialize, Serialize)]
            #[serde(rename_all = "camelCase")]
            struct JsonModel {
                #[serde(alias = "ClassName")]
                class_name: Ustr,
                #[serde(alias = "Children", default = "Vec::new")]
                children: Vec<JsonModel>,
            }
            "#,
        );

        let item = registry.expect_struct("JsonModel").unwrap();
        let class_name = item.field("className").unwrap();
        assert_eq!(class_name.aliases, ["ClassName"]);
        assert!(!class_name.optional);
        assert!(item.field("children").unwrap().optional);
    }

    #[test]
    fn joins_wrapped_doc_comments_into_paragraphs() {
        let registry = registry(
            r"
            #[derive(Deserialize)]
            struct Thing {
                /// The name of the top-level instance
                /// described by the project.
                ///
                /// Second paragraph.
                name: Option<String>,
            }
            ",
        );

        let item = registry.expect_struct("Thing").unwrap();
        let field = item.field("name").unwrap();
        assert_eq!(
            field.doc,
            "The name of the top-level instance described by the project.\n\nSecond paragraph."
        );
    }
}
