# Third-party notices

This project is licensed under [MPL-2.0](./LICENSE). The files under `vendor/`
are **not** part of it: they are verbatim copies of Rojo's source, redistributed
here unmodified, and they remain the work of the Rojo authors under the same
license.

This file covers code that lives *in this repository*. It is not a dependency
manifest; the licenses of crates pulled in by Cargo are recorded in `Cargo.lock`
and in each dependency's own repository.

---

## Rojo — MPL-2.0

- Upstream: <https://github.com/rojo-rbx/rojo>
- Copied at tag: `v7.7.0`, recorded with a digest per file in
  [`vendor.toml`](./vendor.toml)
- License text: <https://github.com/rojo-rbx/rojo/blob/v7.7.0/LICENSE.txt>,
  reproduced in full in [`LICENSE`](./LICENSE)

These files are copied byte for byte and never edited. They are parsed, never
compiled: this project reads Rojo's serde declarations to compile JSON Schemas
from them, so the copies have to stay identical to upstream to be worth
anything. `rojo-schema check` re-hashes them against `vendor.toml` and fails if
any of them was touched.

Files copied from Rojo:

| In this repository                         | Upstream path                             |
| ------------------------------------------ | ----------------------------------------- |
| `vendor/project.rs`                        | `src/project.rs`                          |
| `vendor/glob.rs`                           | `src/glob.rs`                             |
| `vendor/resolution.rs`                     | `src/resolution.rs`                       |
| `vendor/snapshot/metadata.rs`              | `src/snapshot/metadata.rs`                |
| `vendor/snapshot_middleware/mod.rs`        | `src/snapshot_middleware/mod.rs`          |
| `vendor/snapshot_middleware/meta_file.rs`  | `src/snapshot_middleware/meta_file.rs`    |
| `vendor/snapshot_middleware/json_model.rs` | `src/snapshot_middleware/json_model.rs`   |
| `vendor/syncback/mod.rs`                   | `src/syncback/mod.rs`                     |

Because this project is itself MPL-2.0, no license terms are mixed: the copied
files and the code that reads them are governed by the same license. MPL-2.0 is
per-file copyleft, so the copies stay under it regardless.

The generated schemas in `schema/` are compiled from those declarations and
carry the descriptions Rojo's authors wrote in their doc comments.
