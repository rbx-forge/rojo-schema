use std::{env, path::PathBuf};

use anyhow::Result;
use clap::{Parser, Subcommand};
use rojo_schema::{check, find_root, generate, vendor, write};

#[derive(Debug, Parser)]
#[command(name = "rojo-schema", version, about)]
struct Cli {
    /// Repository root. Defaults to the nearest directory holding vendor.toml.
    #[arg(long, global = true)]
    root: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Compile the schemas and write them into schema/.
    Generate,
    /// Verify the vendored sources, recompile twice, and compare with schema/.
    Check,
    /// Copy the Rojo sources again at a given tag and repin their digests.
    Vendor {
        /// Rojo release tag, such as v7.7.0.
        #[arg(long)]
        tag: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let root = match cli.root {
        Some(root) => root,
        None => find_root(&env::current_dir()?)?,
    };

    match cli.command {
        Command::Generate => {
            let artifacts = generate(&root)?;
            write(&root, &artifacts)?;
            for name in artifacts.files.keys() {
                println!("wrote {}/{name}", rojo_schema::OUTPUT);
            }
        }
        Command::Check => {
            check(&root)?;
            println!("schemas are current and reproducible");
        }
        Command::Vendor { tag } => {
            let pin = vendor::read_pin(&root)?;
            let refreshed = vendor::refresh(&root, &pin, &tag)?;
            println!(
                "vendored {} files from Rojo {}",
                refreshed.files.len(),
                refreshed.tag
            );
            println!("run `rojo-schema generate` next, then review the schema diff");
        }
    }

    Ok(())
}
