//! End to end tests: the compiler runs on the real vendored sources, and the
//! schemas it produces are validated against files Rojo would accept or reject.

use std::{
    fs,
    path::{Path, PathBuf},
};

use jsonschema::Validator;
use serde_json::Value;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn artifacts() -> rojo_schema::Artifacts {
    rojo_schema::generate(&root()).expect("the vendored sources compile")
}

fn schema(name: &str) -> Value {
    serde_json::from_str(artifacts().get(name).expect("schema was compiled")).unwrap()
}

fn validator(name: &str) -> Validator {
    jsonschema::validator_for(&schema(name)).expect("the compiled schema is a valid JSON Schema")
}

/// The three validators, compiled once, indexed the way an editor would index
/// them: by the suffix of the file name.
struct Validators {
    by_suffix: Vec<(&'static str, &'static str, Validator)>,
}

impl Validators {
    fn new() -> Self {
        let artifacts = artifacts();
        let by_suffix = [
            (".project.json", rojo_schema::PROJECT),
            (".meta.json", rojo_schema::META),
            (".model.json", rojo_schema::MODEL),
        ]
        .into_iter()
        .map(|(suffix, name)| {
            let schema: Value = serde_json::from_str(artifacts.get(name).unwrap()).unwrap();
            let validator = jsonschema::validator_for(&schema)
                .unwrap_or_else(|error| panic!("{name} is not a valid JSON Schema: {error}"));
            (suffix, name, validator)
        })
        .collect();

        Self { by_suffix }
    }

    fn of(&self, file: &str) -> Option<(&'static str, &Validator)> {
        self.by_suffix
            .iter()
            .find(|(suffix, _, _)| file.ends_with(suffix))
            .map(|(_, name, validator)| (*name, validator))
    }
}

fn fixtures(kind: &str) -> Vec<(String, Value)> {
    let directory = root().join("tests/fixtures").join(kind);
    let mut files = Vec::new();

    for entry in fs::read_dir(&directory).expect("fixtures directory exists") {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let text = fs::read_to_string(&path).unwrap();
        let document: Value = serde_json::from_str(&text)
            .unwrap_or_else(|error| panic!("{name} is not valid JSON: {error}"));
        files.push((name, document));
    }

    assert!(
        !files.is_empty(),
        "no fixtures found in {}",
        directory.display()
    );
    files
}

#[test]
fn committed_schemas_are_current_and_reproducible() {
    // Also fails when a vendored file was edited by hand: the digests are
    // checked before anything is compiled.
    rojo_schema::check(&root()).expect("schema/ matches the vendored sources");
}

#[test]
fn every_schema_is_a_valid_json_schema() {
    for name in [rojo_schema::PROJECT, rojo_schema::META, rojo_schema::MODEL] {
        let _ = validator(name);
    }
}

#[test]
fn accepts_files_rojo_accepts() {
    let validators = Validators::new();

    for (name, document) in fixtures("valid") {
        let (schema, validator) = validators.of(&name).expect("fixture maps to a schema");
        let errors: Vec<String> = validator
            .iter_errors(&document)
            .map(|error| format!("{}: {error}", error.instance_path()))
            .collect();

        assert!(
            errors.is_empty(),
            "{name} should pass {schema} but did not:\n  {}",
            errors.join("\n  ")
        );
    }
}

#[test]
fn rejects_files_rojo_rejects() {
    let validators = Validators::new();

    for (name, document) in fixtures("invalid") {
        let (schema, validator) = validators.of(&name).expect("fixture maps to a schema");
        assert!(
            !validator.is_valid(&document),
            "{name} should be rejected by {schema} but passed"
        );
    }
}

#[test]
fn project_keeps_the_shape_rojo_declares() {
    let schema = schema(rojo_schema::PROJECT);

    assert_eq!(schema["required"], serde_json::json!(["tree"]));
    // Project is the one container Rojo declares with deny_unknown_fields.
    assert_eq!(schema["additionalProperties"], Value::Bool(false));
    assert!(schema["properties"]["$schema"].is_object());

    let node = &schema["$defs"]["ProjectNode"];
    for key in [
        "$className",
        "$path",
        "$properties",
        "$attributes",
        "$ignoreUnknownInstances",
        "$id",
    ] {
        assert!(
            node["properties"][key].is_object(),
            "ProjectNode lost {key}"
        );
    }
    // Unknown keys are children, which is what makes the tree recursive.
    assert_eq!(node["additionalProperties"]["$ref"], "#/$defs/ProjectNode");
}

#[test]
fn middleware_offers_only_what_a_file_can_ask_for() {
    let schema = schema(rojo_schema::PROJECT);
    let values = schema["$defs"]["Middleware"]["enum"]
        .as_array()
        .expect("middleware is a string enum")
        .iter()
        .map(|value| value.as_str().unwrap().to_owned())
        .collect::<Vec<_>>();

    assert!(values.contains(&"jsonModel".to_owned()));
    assert!(values.contains(&"project".to_owned()));
    // Directory middlewares carry #[serde(skip_deserializing)], so no sync rule
    // may name them.
    assert!(!values.iter().any(|value| value == "dir"));
    assert!(!values.iter().any(|value| value.ends_with("Dir")));
}

#[test]
fn model_accepts_both_spellings_of_its_keys() {
    let schema = schema(rojo_schema::MODEL);
    let properties = &schema["properties"];

    for key in [
        "className",
        "ClassName",
        "children",
        "Children",
        "properties",
        "Properties",
    ] {
        assert!(properties[key].is_object(), "the model schema lost {key}");
    }
}

#[test]
fn descriptions_come_from_rojo_itself() {
    let schema = schema(rojo_schema::PROJECT);
    let description = schema["properties"]["servePlaceIds"]["description"]
        .as_str()
        .expect("servePlaceIds carries Rojo's own doc comment");

    assert!(
        description.contains("prevent syncing a Rojo project into the wrong Roblox place"),
        "unexpected description: {description}"
    );
}

/// Validates a real project against the schemas, when one is pointed at.
///
/// Set `ROJO_SCHEMA_CORPUS` to a checkout that Rojo builds today. Nothing is
/// committed here, so the corpus can be any private repository.
#[test]
fn accepts_a_real_corpus_when_one_is_given() {
    let Ok(corpus) = std::env::var("ROJO_SCHEMA_CORPUS") else {
        return;
    };

    let validators = Validators::new();
    let mut checked = 0;
    let mut failures = Vec::new();
    walk(Path::new(&corpus), &mut |path| {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let Some((schema, validator)) = validators.of(&name) else {
            return;
        };

        let text = fs::read_to_string(path).unwrap_or_default();
        // Rojo reads these files as JSONC, so comments are not a schema matter.
        let Ok(document) = serde_json::from_str::<Value>(&strip_comments(&text)) else {
            return;
        };

        checked += 1;
        for error in validator.iter_errors(&document) {
            failures.push(format!(
                "{} [{schema}] {}: {error}",
                path.display(),
                error.instance_path()
            ));
        }
    });

    assert!(checked > 0, "the corpus held no Rojo files");
    assert!(
        failures.is_empty(),
        "{} of {checked} files failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

fn walk(directory: &Path, visit: &mut impl FnMut(&Path)) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();

        if path.is_dir() {
            if name != ".git" && name != "target" {
                walk(&path, visit);
            }
        } else {
            visit(&path);
        }
    }
}

fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;

    while let Some(char) = chars.next() {
        if in_string {
            out.push(char);
            if char == '\\' {
                if let Some(escaped) = chars.next() {
                    out.push(escaped);
                }
            } else if char == '"' {
                in_string = false;
            }
            continue;
        }

        match char {
            '"' => {
                in_string = true;
                out.push(char);
            }
            '/' if chars.peek() == Some(&'/') => {
                for char in chars.by_ref() {
                    if char == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut previous = ' ';
                for char in chars.by_ref() {
                    if previous == '*' && char == '/' {
                        break;
                    }
                    previous = char;
                }
            }
            _ => out.push(char),
        }
    }

    out
}
