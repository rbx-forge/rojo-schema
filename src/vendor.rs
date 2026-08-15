//! The pinned Rojo sources this repository compiles from.
//!
//! The files live under `vendor/` and are copied verbatim from a Rojo tag. They
//! are never compiled, only parsed, which is what lets them stay unmodified.
//! `vendor.toml` records the tag and a digest per file, so a hand-edited or
//! half-refreshed vendor is an error rather than a stale schema.

use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const PIN_FILE: &str = "vendor.toml";
pub const DIRECTORY: &str = "vendor";
const RAW: &str = "https://raw.githubusercontent.com/rojo-rbx/rojo";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pin {
    pub repository: String,
    pub tag: String,
    pub version: String,
    pub files: Vec<PinnedFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinnedFile {
    /// Path under `vendor/`.
    pub path: String,
    /// Path in the Rojo repository this file was copied from.
    pub source: String,
    pub sha256: String,
}

/// A vendored file and its contents, ready for parsing.
pub struct Source {
    pub path: String,
    pub contents: String,
}

pub fn read_pin(root: &Path) -> Result<Pin> {
    let path = root.join(PIN_FILE);
    let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let pin: Pin = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;

    if pin.files.is_empty() {
        bail!("{} pins no files", path.display());
    }

    Ok(pin)
}

/// Loads every pinned file, rejecting any whose digest moved.
pub fn load(root: &Path, pin: &Pin) -> Result<Vec<Source>> {
    let mut sources = Vec::with_capacity(pin.files.len());

    for file in &pin.files {
        let path = vendor_path(root, &file.path);
        let bytes = fs::read(&path).with_context(|| {
            format!(
                "reading {}. Restore it with `rojo-schema vendor --tag {}`.",
                path.display(),
                pin.tag
            )
        })?;

        let digest = hex(&Sha256::digest(&bytes));
        if digest != file.sha256 {
            bail!(
                "{} does not match the digest pinned in {PIN_FILE}.\n  \
                 pinned:  {}\n  on disk: {}\n\
                 Vendored sources are copied verbatim and never edited by hand.",
                path.display(),
                file.sha256,
                digest,
            );
        }

        sources.push(Source {
            path: file.path.clone(),
            contents: String::from_utf8(bytes)
                .with_context(|| format!("{} is not valid UTF-8", path.display()))?,
        });
    }

    Ok(sources)
}

/// Downloads every pinned file at `tag` and rewrites the pin file.
pub fn refresh(root: &Path, pin: &Pin, tag: &str) -> Result<Pin> {
    let mut refreshed = Pin {
        repository: pin.repository.clone(),
        tag: tag.to_owned(),
        version: tag.trim_start_matches('v').to_owned(),
        files: Vec::with_capacity(pin.files.len()),
    };

    for file in &pin.files {
        let url = format!("{RAW}/{tag}/{}", file.source);
        let contents = fetch(&url)?;
        let path = vendor_path(root, &file.path);

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::write(&path, contents.as_bytes())
            .with_context(|| format!("writing {}", path.display()))?;

        refreshed.files.push(PinnedFile {
            path: file.path.clone(),
            source: file.source.clone(),
            sha256: hex(&Sha256::digest(contents.as_bytes())),
        });
    }

    fs::write(root.join(PIN_FILE), render(&refreshed))
        .with_context(|| format!("writing {PIN_FILE}"))?;

    Ok(refreshed)
}

fn fetch(url: &str) -> Result<String> {
    let mut response = ureq::get(url)
        .call()
        .with_context(|| format!("requesting {url}"))?;

    if response.status() == 404 {
        bail!(
            "{url} is gone at this tag. The file moved or was renamed upstream, \
             so its `source` in {PIN_FILE} needs updating by hand."
        );
    }

    response
        .body_mut()
        .read_to_string()
        .with_context(|| format!("reading the body of {url}"))
}

fn vendor_path(root: &Path, path: &str) -> PathBuf {
    root.join(DIRECTORY).join(path)
}

/// Renders a digest the way the pin file and the manifest record it.
pub fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Renders the pin file, comments included, so it stays a generated artifact.
fn render(pin: &Pin) -> String {
    let mut out = String::new();
    out.push_str(
        "# The Rojo sources this repository compiles its schemas from.\n\
         #\n\
         # The files under vendor/ are copied verbatim from the tag below and are never\n\
         # compiled, only parsed. Refresh them with `rojo-schema vendor --tag vX.Y.Z`,\n\
         # which rewrites both the files and the digests recorded here.\n\
         #\n\
         # `rojo-schema check` re-hashes vendor/ against these digests, so an edited or\n\
         # half-refreshed vendor fails instead of silently producing a stale schema.\n\n",
    );

    let _ = writeln!(out, "repository = \"{}\"", pin.repository);
    let _ = writeln!(out, "tag = \"{}\"", pin.tag);
    let _ = writeln!(out, "version = \"{}\"", pin.version);

    for file in &pin.files {
        out.push_str("\n[[files]]\n");
        let _ = writeln!(out, "path = \"{}\"", file.path);
        let _ = writeln!(out, "source = \"{}\"", file.source);
        let _ = writeln!(out, "sha256 = \"{}\"", file.sha256);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendered_pins_parse_back_unchanged() {
        let pin = Pin {
            repository: "https://github.com/rojo-rbx/rojo".into(),
            tag: "v7.7.0".into(),
            version: "7.7.0".into(),
            files: vec![PinnedFile {
                path: "project.rs".into(),
                source: "src/project.rs".into(),
                sha256: "abc".into(),
            }],
        };

        let parsed: Pin = toml::from_str(&render(&pin)).unwrap();
        assert_eq!(parsed.tag, pin.tag);
        assert_eq!(parsed.files[0].sha256, "abc");
    }
}
