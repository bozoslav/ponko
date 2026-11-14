//! `ponko`: the command-line tool for Ponko games.
//!
//! One subcommand per job, each in its own module: `ponko new` makes a
//! project from a template ([`new`]); `ponko package` builds and packages a
//! game for players, and `ponko notices` writes a library's third-party
//! notices ([`package`]); `ponko textures` compresses a folder's images
//! into KTX2 textures ([`textures`], feature `textures`). Arguments are
//! parsed by hand.

mod new;
mod package;
mod templates;
#[cfg(feature = "textures")]
mod textures;

use std::process::ExitCode;

const USAGE: &str = "\
ponko: the command-line tool for Ponko games

Usage:
  ponko new <path> [--2d | --3d | --headless] [--path <engine> | --git <url>]
  ponko package [--bin <name> | --example <name>] [--web] [--format <list>] [...]
  ponko notices --out <file> [-p <package>] [...]
  ponko textures [<folder>] [--out <folder>] [--force] [--check]
  ponko help [<command>]
  ponko --version

Commands:
  new      Make a new game project from a template
  package  Build the game and package it for players (zip, app, dmg, tar.gz, web)
  notices  Write THIRD-PARTY-NOTICES.txt for a crate
  textures Compress images into GPU textures (KTX2: BC7, BC5, BC4, UASTC)

Guide: https://ponko.dev/guide/getting-started.html";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(out) => {
            println!("{out}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("ponko: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Runs one command line (without the program name); returns what to print.
fn run(args: &[String]) -> Result<String, String> {
    let (command, rest) = match args.split_first() {
        Some((c, rest)) => (c.as_str(), rest),
        None => return Ok(USAGE.to_string()),
    };
    match command {
        "new" => {
            if rest.iter().any(|a| a == "-h" || a == "--help") {
                return Ok(new::USAGE.to_string());
            }
            let options = new::Options::parse(rest)?;
            let dir = std::env::current_dir().map_err(|e| format!("can't read the current folder: {e}"))?;
            new::generate(&options, &dir)
        }
        "package" | "notices" => {
            let usage = if command == "package" {
                package::USAGE
            } else {
                package::NOTICES_USAGE
            };
            if rest.iter().any(|a| a == "-h" || a == "--help") {
                return Ok(usage.to_string());
            }
            let options = package::Options::parse(rest, usage)?;
            let dir = std::env::current_dir().map_err(|e| format!("can't read the current folder: {e}"))?;
            if command == "package" {
                package::package_command(&options, &dir)
            } else {
                package::notices_command(&options, &dir)
            }
        }
        "textures" => {
            #[cfg(feature = "textures")]
            {
                if rest.iter().any(|a| a == "-h" || a == "--help") {
                    return Ok(textures::USAGE.to_string());
                }
                let options = textures::Options::parse(rest)?;
                let dir = std::env::current_dir().map_err(|e| format!("can't read the current folder: {e}"))?;
                textures::textures_command(&options, &dir)
            }
            #[cfg(not(feature = "textures"))]
            Err(
                "this ponko was built without `ponko textures`: install it with its default features \
                 (`cargo install ponko_cli`)"
                    .into(),
            )
        }
        "help" | "-h" | "--help" => match rest.first().map(String::as_str) {
            None => Ok(USAGE.to_string()),
            Some("new") => Ok(new::USAGE.to_string()),
            Some("package") => Ok(package::USAGE.to_string()),
            Some("notices") => Ok(package::NOTICES_USAGE.to_string()),
            #[cfg(feature = "textures")]
            Some("textures") => Ok(textures::USAGE.to_string()),
            Some(other) => Err(format!("no command `{other}`\n\n{USAGE}")),
        },
        "-V" | "--version" => Ok(format!("ponko {}", env!("CARGO_PKG_VERSION"))),
        other => Err(format!("no command `{other}`\n\n{USAGE}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_str(args: &[&str]) -> Result<String, String> {
        run(&args.iter().map(|a| a.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn no_arguments_prints_the_usage() {
        assert!(run_str(&[]).unwrap().contains("ponko new <path>"));
    }

    #[test]
    fn version_names_the_crate_version() {
        assert_eq!(
            run_str(&["--version"]).unwrap(),
            format!("ponko {}", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn help_for_new_lists_its_options() {
        for args in [&["help", "new"][..], &["new", "--help"]] {
            let out = run_str(args).unwrap();
            assert!(out.contains("--headless") && out.contains("--git"), "{out}");
        }
    }

    #[test]
    fn help_for_package_lists_its_platforms_and_options() {
        for args in [&["help", "package"][..], &["package", "--help"]] {
            let out = run_str(args).unwrap();
            assert!(
                out.contains("--example") && out.contains("--web") && out.contains("--sign"),
                "{out}"
            );
        }
        assert!(run_str(&["notices", "-h"]).unwrap().contains("--out <file>"));
    }

    #[test]
    fn an_unknown_command_is_an_error_with_the_usage() {
        let err = run_str(&["pack"]).unwrap_err();
        assert!(err.starts_with("no command `pack`") && err.contains("Usage:"), "{err}");
    }
}
