# rojo-schema

JSON Schemas for the three file formats Rojo reads, compiled from Rojo's own
source rather than written by hand.

| Schema                      | Applies to                                  |
| --------------------------- | ------------------------------------------- |
| `schema/project.schema.json` | `*.project.json`                            |
| `schema/meta.schema.json`    | `*.meta.json`, including `init.meta.json`   |
| `schema/model.schema.json`   | `*.model.json`                              |

`schema/manifest.json` records which Rojo release the three were compiled from,
the digest of every source file that fed them, and the digest of each schema.

## Using them

Point a file at its schema:

```json
{
  "$schema": "https://raw.githubusercontent.com/rbx-forge/rojo-schema/main/schema/project.schema.json",
  "name": "my-game",
  "tree": { "$className": "DataModel" }
}
```

Rojo declares `$schema` as a real field on all three formats, so this does not
break parsing. Editors can also be configured by file pattern, which is the
better option for `.meta.json` and `.model.json` files.

That URL tracks `main`, so it follows Rojo. To freeze a project on the Rojo
release it actually runs, swap `main` for the matching tag:

```
https://raw.githubusercontent.com/rbx-forge/rojo-schema/rojo-7.7.0/schema/project.schema.json
```

Both go through `raw.githubusercontent.com` on purpose. Editors do not fetch
schemas from just anywhere: VS Code ships a `json.schemaDownload.trustedDomains`
allowlist that already contains that host, while `github.com/.../releases/download/...`
is not on it and redirects besides, so a release asset URL loads in a browser but
is refused in an editor.

## Releases

Every distinct set of schemas gets its own release, named after the Rojo release
it describes: `rojo-7.7.0`. Each one carries the three schemas and the manifest
as assets, and the notes state the Rojo tag, the generator version and the
digest of each file. The tag is what a project pins against, through the raw URL
above; the assets are there for anything that downloads rather than fetches.

Releases are immutable. If the compiler itself changes and produces different
schemas from the same Rojo release, the next snapshot is `rojo-7.7.0-r2`, and so
on. A change that leaves the schemas identical publishes nothing.

Two jobs keep this moving without anyone watching Rojo:

- **Track Rojo** runs daily. When Rojo publishes a release, it re-vendors the
  sources at that tag, recompiles, runs the full check suite and opens a pull
  request carrying the grammar diff. If the release broke the compiler, the job
  fails and no pull request appears, which is the intended outcome: a human
  looks at what moved upstream.
- **Release** runs when `schema/` or `vendor.toml` lands on `main`, and cuts the
  snapshot described above.

## How it is built

The grammar is not transcribed, it is read:

1. `vendor/` holds the Rojo source files that declare the formats, copied
   verbatim from a release tag. They are never compiled, only parsed, which is
   what lets them stay byte for byte identical to upstream.
2. `vendor.toml` pins the tag and a SHA-256 per file.
3. `syn` parses those files into an AST, and the serde attributes on them are
   read the way serde would: `rename`, `rename_all`, `alias`, `default`, `skip`,
   `skip_deserializing`, `flatten`, `untagged`, `deny_unknown_fields`.
4. The doc comments Rojo's authors wrote become the schema descriptions.

The result is that a field Rojo adds, renames or deletes moves the schema with
it, and every description is Rojo's own wording rather than a paraphrase.

Nothing is inferred silently. A type the compiler cannot describe, a container
that disappeared upstream, an enum shape it does not model: each is an error
that stops the build.

## Following a new Rojo release

The Track Rojo job does this on its own and opens a pull request. By hand, it is
the same four commands:

```sh
cargo run -- vendor --tag v7.8.0   # re-copies vendor/ and repins the digests
cargo run -- generate              # recompiles schema/
git diff schema/                   # read what changed in the grammar
cargo test
```

Three outcomes are possible, and they are meant to be distinguishable:

- **The diff is empty.** The release did not touch the formats.
- **The diff shows new or changed fields.** That is the release's grammar
  change, in Rojo's own words. Commit it.
- **`generate` fails.** A type moved out of a vendored file, or grew a shape
  the compiler does not model. The error names the container. Fix `vendor.toml`
  or the compiler, never the vendored file.

The file list itself is version-dependent: `src/syncback/mod.rs` did not exist
before 7.7.0, for instance. `vendor` says so plainly when a pinned path is
absent at the requested tag, which matters mostly when pinning an older release
on purpose.

`cargo run -- check` is the read-only form: it re-hashes `vendor/`, recompiles
twice to prove the output is deterministic, and compares against what is
committed. CI runs it, so a vendored file edited by hand and a schema left
stale both fail loudly.

## What these schemas do not do

- **Property values are not typed per class.** `$properties` accepts any value
  Rojo would resolve, but the schema does not know that `Workspace.Gravity` is a
  number. Doing better means pulling in Roblox's reflection database, a second
  source that moves on its own schedule, and it is deliberately out of scope: a
  schema compiled from Rojo alone is one that follows Rojo alone.
- **Comments are a parser concern, not a schema one.** Rojo reads all three
  formats as JSONC, so comments and a `.jsonc` extension are fine. A validator
  has to strip them before validating, as editors already do.
- **`className` in a `.meta.json`.** Rojo only acts on it inside an
  `init.meta.json`, but it ignores unknown fields elsewhere rather than
  rejecting them, so the schema accepts it in both and says so in the field
  description.
- **`$path` cannot be resolved.** JSON Schema cannot know what class a
  filesystem path produces, so the constraints Rojo enforces between `$path` and
  `$className` are documented in the descriptions and not enforced here.

## Layout

```
vendor/        Rojo sources, verbatim, pinned by vendor.toml
src/ty.rs      the subset of Rust types the grammar is written in
src/ir.rs      syn AST to a serde-aware intermediate form
src/emit.rs    intermediate form to JSON Schema
src/vendor.rs  the pin file, its digests, and refreshing it from a tag
schema/        the generated documents, committed
tests/         fixtures Rojo accepts and fixtures it rejects
```

Point `ROJO_SCHEMA_CORPUS` at a real Rojo project to validate every
`.project.json`, `.meta.json` and `.model.json` under it as part of the test
run. Nothing from that corpus is committed here.

## License

This project is [MPL-2.0](./LICENSE), the same license Rojo uses.

The files under `vendor/` are verbatim copies of Rojo's source, redistributed
unmodified and remaining the work of the Rojo authors. See
[THIRD-PARTY-NOTICES.md](./THIRD-PARTY-NOTICES.md) for the file list and the
tag they were taken from.
