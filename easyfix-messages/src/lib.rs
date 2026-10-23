//! Code generator turning FIX XML dictionaries into Rust message types.
//!
//! [`Generator`] parses the dictionaries, then emits one Rust source file
//! holding every field enum, repeating group, message struct, the `Header` /
//! `Trailer` pair and the `Message` enum that ties them together, along with
//! their `SessionMessage` implementations. The output is pretty-printed, so it
//! stays readable when a generated deserializer needs debugging - though not
//! by `rustfmt`, so it does not follow the consuming project's `rustfmt.toml`.
//!
//! The caller picks the output path. Build scripts write into `OUT_DIR` and
//! `include!` the result:
//!
//! ```no_run
//! // build.rs
//! use std::{env, path::PathBuf};
//!
//! # fn main() -> anyhow::Result<()> {
//! let out_dir = PathBuf::from(env::var("OUT_DIR")?);
//!
//! easyfix_messages::Generator::new()
//!     .fixt_xml("xml/FIXT11.xml")
//!     .fix_xml("xml/FIX50SP2.xml")
//!     .output(out_dir.join("messages.rs"))
//!     .generate()?;
//! # Ok(())
//! # }
//! ```
//!
//! The consuming crate then pulls the file in with
//! `include!(concat!(env!("OUT_DIR"), "/messages.rs"))`. Test fixtures take
//! the other route: `easyfix-test-messages` commits the generated file and
//! regenerates it through `scripts/regenerate-test-messages.sh`.
//!
//! Generated imports use `easyfix_core` by default. When the consuming crate
//! depends on the `easyfix` facade, set [`Generator::core_path`] to
//! `"easyfix::core"` instead.
//!
//! For pre-generated code, use [`Generator::serde_serialize_feature`] and
//! [`Generator::serde_deserialize_feature`] to gate serde derives on features
//! of the consuming crate. For example,
//! `.serde_serialize_feature("serde-serialize")` emits
//! `#[cfg_attr(feature = "serde-serialize", derive(serde::Serialize))]`.
//! The consuming crate must declare that feature, enable serde's `derive`
//! support, and forward the feature to `easyfix/serde-serialize` (or
//! `easyfix-core/serde-serialize`). If its serde dependency is optional,
//! include `dep:serde` in the feature as well. The deserialize direction
//! requires the corresponding `serde-deserialize` feature.
//!
//! Generation is strict: the dictionaries are validated
//! (`with_strict_check(true)`) and a dictionary that customizes a codeset the
//! FIX standard closes - `ApplVerID(1128)` / `DefaultApplVerID(1137)` - fails
//! the build rather than silently changing accept/reject behavior.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::Context;
use easyfix_dictionary::DictionaryBuilder;
use proc_macro2::TokenStream;
use quote::quote;
use syn::{Path as RustPath, parse::Parser};

mod code_gen;

enum SerdeMode {
    Disabled,
    Unconditional,
    Feature(String),
}

impl SerdeMode {
    fn derive(&self, trait_path: TokenStream) -> TokenStream {
        match self {
            Self::Disabled => quote! {},
            Self::Unconditional => quote! { #[derive(#trait_path)] },
            Self::Feature(feature) => {
                quote! { #[cfg_attr(feature = #feature, derive(#trait_path))] }
            }
        }
    }
}

fn format_token_stream(tokens: TokenStream) -> anyhow::Result<String> {
    let syntax_tree = syn::parse2::<syn::File>(tokens).context("failed to parse generated code")?;
    Ok(prettyplease::unparse(&syntax_tree))
}

/// Builder for configuring and running FIX message code generation.
pub struct Generator {
    fixt_xml: Option<PathBuf>,
    fix_xml: Option<PathBuf>,
    output: Option<PathBuf>,
    core_path: String,
    serde_serialize: SerdeMode,
    serde_deserialize: SerdeMode,
}

impl Default for Generator {
    fn default() -> Self {
        Self::new()
    }
}

impl Generator {
    /// Create a new generator with default settings.
    pub fn new() -> Self {
        Generator {
            fixt_xml: None,
            fix_xml: None,
            output: None,
            core_path: "easyfix_core".into(),
            serde_serialize: SerdeMode::Disabled,
            serde_deserialize: SerdeMode::Disabled,
        }
    }

    /// Path to FIXT transport XML (e.g., FIXT11.xml).
    /// Optional - omit for FIX versions that don't use FIXT (pre-FIX5).
    pub fn fixt_xml(mut self, path: impl AsRef<Path>) -> Self {
        self.fixt_xml = Some(path.as_ref().to_owned());
        self
    }

    /// Path to FIX application XML (e.g., FIX50SP2.xml). Required.
    pub fn fix_xml(mut self, path: impl AsRef<Path>) -> Self {
        self.fix_xml = Some(path.as_ref().to_owned());
        self
    }

    /// Output path for the generated Rust source file. Required.
    pub fn output(mut self, path: impl AsRef<Path>) -> Self {
        self.output = Some(path.as_ref().to_owned());
        self
    }

    /// Rust module path to the core API in the consuming crate.
    /// Defaults to `easyfix_core`. Use `easyfix::core` with the facade, or
    /// the corresponding path if the dependency is renamed.
    /// Invalid module path syntax is reported by [`Self::generate`].
    pub fn core_path(mut self, path: impl Into<String>) -> Self {
        self.core_path = path.into();
        self
    }

    /// Generate `#[derive(serde::Serialize)]` on generated types.
    /// When true, the user must also enable `easyfix-core/serde-serialize` for core types.
    /// Default: false. Replaces any previous serialize feature setting.
    pub fn serde_serialize(mut self, value: bool) -> Self {
        self.serde_serialize = if value {
            SerdeMode::Unconditional
        } else {
            SerdeMode::Disabled
        };
        self
    }

    /// Generate `#[derive(serde::Deserialize)]` on generated types.
    /// When true, the user must also enable `easyfix-core/serde-deserialize` for core types.
    /// Default: false. Replaces any previous deserialize feature setting.
    pub fn serde_deserialize(mut self, value: bool) -> Self {
        self.serde_deserialize = if value {
            SerdeMode::Unconditional
        } else {
            SerdeMode::Disabled
        };
        self
    }

    /// Generate `serde::Serialize` derives gated by a consuming crate feature.
    /// The feature must enable serde with derive support and
    /// `easyfix-core/serde-serialize` (or `easyfix/serde-serialize`).
    /// Replaces any previous serialize setting; no separate enable call is needed.
    pub fn serde_serialize_feature(mut self, feature: impl Into<String>) -> Self {
        self.serde_serialize = SerdeMode::Feature(feature.into());
        self
    }

    /// Generate `serde::Deserialize` derives gated by a consuming crate feature.
    /// The feature must enable serde with derive support and
    /// `easyfix-core/serde-deserialize` (or `easyfix/serde-deserialize`).
    /// Replaces any previous deserialize setting; no separate enable call is needed.
    /// Both directions may use the same feature name.
    pub fn serde_deserialize_feature(mut self, feature: impl Into<String>) -> Self {
        self.serde_deserialize = SerdeMode::Feature(feature.into());
        self
    }

    /// Run the generator, validating the configured paths and dictionaries.
    /// Returns an error if required options are missing, the core module path
    /// is invalid, or reading, generating, or writing the source fails.
    pub fn generate(self) -> anyhow::Result<()> {
        let fix_xml = self.fix_xml.context("fix_xml path is required")?;
        let output = self.output.context("output path is required")?;
        let core_path = RustPath::parse_mod_style
            .parse_str(&self.core_path)
            .with_context(|| format!("invalid core path: {:?}", self.core_path))?;

        let mut builder = DictionaryBuilder::new()
            .with_fix_xml(&fix_xml)
            .with_strict_check(true);

        if let Some(fixt_xml) = &self.fixt_xml {
            builder = builder.with_fixt_xml(fixt_xml);
        }

        let dictionary = builder.build()?;
        let generator = code_gen::Generator::new(&dictionary);

        let serde_ser = self.serde_serialize.derive(quote! { serde::Serialize });
        let serde_de = self.serde_deserialize.derive(quote! { serde::Deserialize });
        let serde_derives = quote! { #serde_ser #serde_de };

        let imports = code_gen::generate_imports(&core_path);
        let fields = generator.generate_fields(&serde_derives);
        let groups = generator.generate_groups(&serde_derives);
        let messages = generator.generate_messages(&serde_derives);

        let combined = quote! {
            #imports
            #fields
            #groups
            #messages
        };

        let formatted = format_token_stream(combined)?;
        let mut file = fs::File::create(&output)
            .with_context(|| format!("failed to create {}", output.display()))?;
        file.write_all(
            b"// @generated by easyfix-messages from FIX XML definitions.\n\
              // Do not edit this file manually. Changes will be overwritten.\n\
              // Edit the source dictionaries or generator and regenerate this file.\n\n",
        )
        .and_then(|()| file.write_all(formatted.as_bytes()))
        .with_context(|| format!("failed to write {}", output.display()))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{env, fs, path::PathBuf, process};

    use super::Generator;

    #[test]
    fn serde_settings_use_the_last_value_for_each_direction() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let output = env::temp_dir().join(format!("easyfix_serde_settings_{}.rs", process::id()));
        for (name, generator, serialize, deserialize) in [
            (
                "unconditional",
                Generator::new()
                    .serde_serialize(true)
                    .serde_deserialize(true),
                Some("#[derive(serde::Serialize)]"),
                Some("#[derive(serde::Deserialize)]"),
            ),
            (
                "disable conditional",
                Generator::new()
                    .serde_serialize_feature("old-ser")
                    .serde_deserialize_feature("old-de")
                    .serde_serialize(false)
                    .serde_deserialize(false),
                None,
                None,
            ),
            (
                "replace conditional with unconditional",
                Generator::new()
                    .serde_serialize_feature("old-ser")
                    .serde_deserialize_feature("old-de")
                    .serde_serialize(true)
                    .serde_deserialize(true),
                Some("#[derive(serde::Serialize)]"),
                Some("#[derive(serde::Deserialize)]"),
            ),
            (
                "replace unconditional with conditional",
                Generator::new()
                    .serde_serialize(true)
                    .serde_deserialize(true)
                    .serde_serialize_feature("fix-to-json")
                    .serde_deserialize_feature("from-json"),
                Some("#[cfg_attr(feature = \"fix-to-json\", derive(serde::Serialize))]"),
                Some("#[cfg_attr(feature = \"from-json\", derive(serde::Deserialize))]"),
            ),
            (
                "replace feature names independently",
                Generator::new()
                    .serde_serialize_feature("old-ser")
                    .serde_deserialize_feature("old-de")
                    .serde_serialize_feature("fix-to-json")
                    .serde_deserialize(true),
                Some("#[cfg_attr(feature = \"fix-to-json\", derive(serde::Serialize))]"),
                Some("#[derive(serde::Deserialize)]"),
            ),
        ] {
            generator
                .fixt_xml(root.join("../easyfix-test-messages/xml/FIXT11-test.xml"))
                .fix_xml(root.join("../easyfix-test-messages/xml/FIX50SP2-test.xml"))
                .output(&output)
                .generate()
                .unwrap();
            let code = fs::read_to_string(&output).unwrap();
            fs::remove_file(&output).unwrap();
            assert!(!code.contains("old-ser"), "{name}");
            assert!(!code.contains("old-de"), "{name}");
            for (trait_name, expected) in [
                ("serde::Serialize", serialize),
                ("serde::Deserialize", deserialize),
            ] {
                let attrs: Vec<_> = code
                    .lines()
                    .filter(|line| line.contains(trait_name))
                    .collect();
                match expected {
                    Some(expected) => {
                        assert!(!attrs.is_empty(), "{name}: missing {trait_name}");
                        assert!(
                            attrs.iter().all(|line| line.trim() == expected),
                            "{name}: {attrs:?}"
                        );
                    }
                    None => assert!(attrs.is_empty(), "{name}: unexpected {trait_name}"),
                }
            }
        }
    }

    #[test]
    fn generated_imports_use_configured_core_path() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let output = env::temp_dir().join(format!("easyfix_core_path_{}.rs", process::id()));
        for (configured, expected) in [
            (None, "easyfix_core"),
            (Some("easyfix::core"), "easyfix::core"),
            (Some("::renamed_fix::core"), "::renamed_fix::core"),
            (Some("crate::runtime"), "crate::runtime"),
        ] {
            let mut generator = Generator::new()
                .fixt_xml(root.join("../easyfix-test-messages/xml/FIXT11-test.xml"))
                .fix_xml(root.join("../easyfix-test-messages/xml/FIX50SP2-test.xml"))
                .output(&output);
            if let Some(path) = configured {
                generator = generator.core_path(path);
            }
            generator.generate().unwrap();
            let code = fs::read_to_string(&output).unwrap();
            fs::remove_file(&output).unwrap();

            assert!(code.contains(&format!("pub use {expected}::{{")));
            if configured.is_some() {
                assert!(!code.contains("easyfix_core"), "core path: {expected}");
            }
        }
    }

    #[test]
    fn invalid_core_path_does_not_overwrite_output() {
        let output =
            env::temp_dir().join(format!("easyfix_invalid_core_path_{}.rs", process::id()));
        for path in [
            "",
            "easyfix-core",
            "easyfix::",
            "easyfix<T>",
            "easyfix; mod other;",
        ] {
            fs::write(&output, "existing output").unwrap();
            let result = Generator::new()
                .fix_xml("unused.xml")
                .output(&output)
                .core_path(path)
                .generate();
            let code = fs::read_to_string(&output).unwrap();
            fs::remove_file(&output).unwrap();

            let error = result.unwrap_err();
            assert!(
                error.to_string().starts_with("invalid core path:"),
                "{error:#}"
            );
            assert_eq!(code, "existing output");
        }
    }
}
