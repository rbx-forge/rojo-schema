//! Compiles JSON Schemas for the three Rojo file formats from vendored sources.
//!
//! The grammar is read out of Rojo's own serde declarations, so the schemas
//! describe what Rojo actually accepts rather than what its documentation says.
//! Nothing here is hand-written per field: a field that appears in Rojo appears
//! in the schema, and a field that disappears takes its schema entry with it.

pub mod emit;
pub mod ir;
pub mod ty;
pub mod vendor;

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};

use crate::{emit::Compiler, ir::Registry, vendor::Pin};

/// Where the published schemas are served from, used for `$id`.
const BASE: &str = "https://raw.githubusercontent.com/rbx-forge/rojo-schema/main/schema";
/// The directory the generated schemas live in, relative to the repository.
pub const OUTPUT: &str = "schema";

pub const PROJECT: &str = "project.schema.json";
pub const META: &str = "meta.schema.json";
pub const MODEL: &str = "model.schema.json";
pub const MANIFEST: &str = "manifest.json";

/// The generated documents, keyed by their file name under `schema/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifacts {
    pub files: BTreeMap<String, String>,
}

impl Artifacts {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.files.get(name).map(String::as_str)
    }
}

/// Compiles every schema in memory from the vendored sources under `root`.
pub fn generate(root: &Path) -> Result<Artifacts> {
    let pin = vendor::read_pin(root)?;
    let sources = vendor::load(root, &pin)?;
    let parsed: Vec<(String, String)> = sources
        .into_iter()
        .map(|source| (source.path, source.contents))
        .collect();
    let registry = ir::read(&parsed)?;

    let mut files = BTreeMap::new();
    files.insert(PROJECT.to_owned(), pretty(&project(&registry, &pin)?)?);
    files.insert(META.to_owned(), pretty(&meta(&registry, &pin)?)?);
    files.insert(MODEL.to_owned(), pretty(&model(&registry, &pin)?)?);

    let manifest = manifest(&pin, &registry, &files);
    files.insert(MANIFEST.to_owned(), pretty(&manifest)?);

    Ok(Artifacts { files })
}

/// Writes the compiled documents into `schema/`.
pub fn write(root: &Path, artifacts: &Artifacts) -> Result<()> {
    let directory = root.join(OUTPUT);
    fs::create_dir_all(&directory).with_context(|| format!("creating {}", directory.display()))?;

    for (name, contents) in &artifacts.files {
        let path = directory.join(name);
        fs::write(&path, contents).with_context(|| format!("writing {}", path.display()))?;
    }

    Ok(())
}

/// Recompiles twice and compares against what is committed.
///
/// Two passes catch a compiler that is not deterministic; the comparison against
/// disk catches a vendor that moved without the schemas being regenerated.
pub fn check(root: &Path) -> Result<Artifacts> {
    let first = generate(root)?;
    let second = generate(root)?;

    if first != second {
        bail!("the compiler is not deterministic: two runs produced different schemas");
    }

    let mut stale = Vec::new();
    for (name, contents) in &first.files {
        let path = root.join(OUTPUT).join(name);
        match fs::read_to_string(&path) {
            Ok(committed) if &committed == contents => {}
            Ok(_) => stale.push(format!("{name} differs from the compiled output")),
            Err(error) => stale.push(format!("{name} could not be read: {error}")),
        }
    }

    if !stale.is_empty() {
        bail!(
            "the committed schemas are stale. Run `rojo-schema generate` and commit the result.\n  {}",
            stale.join("\n  ")
        );
    }

    Ok(first)
}

fn project(registry: &Registry, pin: &Pin) -> Result<Value> {
    let mut compiler = Compiler::new(registry);
    let body = compiler.root("Project")?;

    Ok(document(
        PROJECT,
        "Rojo project file",
        "A Rojo project, stored in a `.project.json` file. Describes the tree of \
         instances Rojo builds or serves, and the settings it uses to do so.",
        body,
        compiler.defs(),
        pin,
    ))
}

fn meta(registry: &Registry, pin: &Pin) -> Result<Value> {
    let mut compiler = Compiler::new(registry);
    let adjacent = compiler.root("AdjacentMetadata")?;
    let directory = compiler.root("DirectoryMetadata")?;
    let body = emit::merge(&adjacent, &directory)?;

    Ok(document(
        META,
        "Rojo meta file",
        "Metadata applied to the instance produced by a neighbouring file or by \
         the containing directory. Rojo reads `name.meta.json` next to `name.luau`, \
         and `init.meta.json` inside a directory. `className` is only read from an \
         `init.meta.json`; elsewhere Rojo ignores it.",
        body,
        compiler.defs(),
        pin,
    ))
}

fn model(registry: &Registry, pin: &Pin) -> Result<Value> {
    let mut compiler = Compiler::new(registry);
    let body = compiler.root("JsonModel")?;

    Ok(document(
        MODEL,
        "Rojo JSON model",
        "An instance tree written as JSON, stored in a `.model.json` file. The \
         file name provides the instance name, so a `name` field on the root is \
         ignored.",
        body,
        compiler.defs(),
        pin,
    ))
}

fn document(
    file: &str,
    title: &str,
    description: &str,
    body: Value,
    defs: Map<String, Value>,
    pin: &Pin,
) -> Value {
    let mut root = match body {
        Value::Object(object) => object,
        other => {
            let mut map = Map::new();
            map.insert("allOf".into(), json!([other]));
            map
        }
    };

    // The Rojo doc comment on the container, if any, follows the description of
    // the file format itself.
    let description = match root.remove("description") {
        Some(Value::String(existing)) => format!("{description}\n\n{existing}"),
        _ => description.to_owned(),
    };

    root.insert(
        "$schema".into(),
        json!("https://json-schema.org/draft/2020-12/schema"),
    );
    root.insert("$id".into(), json!(format!("{BASE}/{file}")));
    root.insert("title".into(), json!(title));
    root.insert("description".into(), json!(description));
    root.insert(
        "$comment".into(),
        json!(format!(
            "Compiled from Rojo {} by rojo-schema. Do not edit by hand.",
            pin.tag
        )),
    );

    if !defs.is_empty() {
        root.insert("$defs".into(), Value::Object(defs));
    }

    Value::Object(root)
}

fn manifest(pin: &Pin, registry: &Registry, files: &BTreeMap<String, String>) -> Value {
    let schemas: Map<String, Value> = files
        .iter()
        .map(|(name, contents)| (name.clone(), json!(digest(contents))))
        .collect();

    let sources: Vec<Value> = pin
        .files
        .iter()
        .map(|file| {
            json!({
                "path": file.path,
                "source": file.source,
                "sha256": file.sha256,
            })
        })
        .collect();

    json!({
        "generator": {
            "name": env!("CARGO_PKG_NAME"),
            "version": env!("CARGO_PKG_VERSION"),
        },
        "rojo": {
            "repository": pin.repository,
            "tag": pin.tag,
            "version": pin.version,
        },
        "containers": registry.len(),
        "sources": sources,
        "schemas": schemas,
    })
}

fn digest(contents: &str) -> String {
    use sha2::{Digest, Sha256};

    vendor::hex(&Sha256::digest(contents.as_bytes()))
}

fn pretty(value: &Value) -> Result<String> {
    let mut text = serde_json::to_string_pretty(value).context("rendering a schema")?;
    text.push('\n');
    Ok(text)
}

/// Finds the repository root from the current directory upwards.
pub fn find_root(start: &Path) -> Result<PathBuf> {
    for candidate in start.ancestors() {
        if candidate.join(vendor::PIN_FILE).is_file() {
            return Ok(candidate.to_path_buf());
        }
    }

    bail!(
        "no {} found in {} or any parent directory",
        vendor::PIN_FILE,
        start.display()
    )
}
