//! The subset of Rust types the vendored Rojo grammar is written in.
//!
//! Anything outside this subset is an error rather than a guess: a type the
//! compiler cannot describe means Rojo grew a shape we do not model yet, and a
//! silently permissive schema would be worse than a failed build.

use anyhow::{bail, Context, Result};
use syn::{Expr, GenericArgument, Lit, PathArguments, Type};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ty {
    /// A leaf or a container defined in the vendored sources.
    Named(String),
    Option(Box<Ty>),
    /// `Vec<T>`, ordered and repeatable.
    List(Box<Ty>),
    /// `HashSet<T>` / `BTreeSet<T>`, unordered and unique.
    Set(Box<Ty>),
    /// Any of the map types, keyed by string in JSON.
    Map(Box<Ty>),
    /// A fixed-length array, `[T; N]`.
    Array(Box<Ty>, usize),
}

impl Ty {
    /// True when the field may be absent from the JSON document.
    pub fn is_optional(&self) -> bool {
        matches!(self, Ty::Option(_))
    }

    pub fn inner(&self) -> &Ty {
        match self {
            Ty::Option(inner) => inner,
            other => other,
        }
    }
}

/// The map types Rojo uses. All of them serialize as a JSON object.
const MAPS: &[&str] = &["BTreeMap", "HashMap", "IndexMap", "UstrMap"];
const SETS: &[&str] = &["HashSet", "BTreeSet"];

pub fn parse(ty: &Type) -> Result<Ty> {
    match ty {
        Type::Path(path) => {
            let segment = path
                .path
                .segments
                .last()
                .context("type path has no segments")?;
            let name = segment.ident.to_string();
            let args = generic_args(&segment.arguments)?;

            match (name.as_str(), args.len()) {
                ("Option", 1) => Ok(Ty::Option(Box::new(parse(args[0])?))),
                ("Vec", 1) => Ok(Ty::List(Box::new(parse(args[0])?))),
                (name, 1) if SETS.contains(&name) => Ok(Ty::Set(Box::new(parse(args[0])?))),
                (name, 2) if MAPS.contains(&name) => Ok(Ty::Map(Box::new(parse(args[1])?))),
                // `UstrMap<V>` fixes the key type, so it carries one argument.
                ("UstrMap", 1) => Ok(Ty::Map(Box::new(parse(args[0])?))),
                (_, 0) => Ok(Ty::Named(name)),
                _ => bail!(
                    "unsupported generic type `{name}` with {} arguments",
                    args.len()
                ),
            }
        }
        Type::Array(array) => {
            let len = array_len(&array.len)?;
            Ok(Ty::Array(Box::new(parse(&array.elem)?), len))
        }
        Type::Reference(reference) => parse(&reference.elem),
        other => bail!("unsupported type in the Rojo grammar: {}", quote(other)),
    }
}

fn generic_args(arguments: &PathArguments) -> Result<Vec<&Type>> {
    match arguments {
        PathArguments::None => Ok(Vec::new()),
        PathArguments::AngleBracketed(bracketed) => {
            let mut types = Vec::new();
            for argument in &bracketed.args {
                match argument {
                    GenericArgument::Type(ty) => types.push(ty),
                    // Lifetimes carry no JSON meaning.
                    GenericArgument::Lifetime(_) => {}
                    other => bail!("unsupported generic argument: {other:?}"),
                }
            }
            Ok(types)
        }
        PathArguments::Parenthesized(_) => bail!("function types have no JSON representation"),
    }
}

fn array_len(len: &Expr) -> Result<usize> {
    match len {
        Expr::Lit(literal) => match &literal.lit {
            Lit::Int(int) => Ok(int.base10_parse()?),
            other => bail!("array length is not an integer literal: {other:?}"),
        },
        other => bail!("array length is not a literal: {other:?}"),
    }
}

fn quote(ty: &Type) -> String {
    match ty {
        Type::Path(path) => path
            .path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect::<Vec<_>>()
            .join("::"),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ty(source: &str) -> Ty {
        parse(&syn::parse_str::<Type>(source).unwrap()).unwrap()
    }

    #[test]
    fn reads_the_shapes_the_grammar_uses() {
        assert_eq!(ty("String"), Ty::Named("String".into()));
        assert_eq!(
            ty("Option<u16>"),
            Ty::Option(Box::new(Ty::Named("u16".into())))
        );
        assert_eq!(
            ty("Vec<SyncRule>"),
            Ty::List(Box::new(Ty::Named("SyncRule".into())))
        );
        assert_eq!(
            ty("HashSet<u64>"),
            Ty::Set(Box::new(Ty::Named("u64".into())))
        );
        assert_eq!(
            ty("BTreeMap<String, ProjectNode>"),
            Ty::Map(Box::new(Ty::Named("ProjectNode".into())))
        );
        assert_eq!(
            ty("IndexMap<Ustr, Vec<Ustr>>"),
            Ty::Map(Box::new(Ty::List(Box::new(Ty::Named("Ustr".into())))))
        );
        assert_eq!(
            ty("[f64; 12]"),
            Ty::Array(Box::new(Ty::Named("f64".into())), 12)
        );
    }

    #[test]
    fn refuses_what_it_cannot_describe() {
        assert!(parse(&syn::parse_str::<Type>("(u8, u8)").unwrap()).is_err());
    }
}
