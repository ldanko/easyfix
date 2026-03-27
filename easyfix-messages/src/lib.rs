use std::{
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

use anyhow::Context;
use easyfix_dictionary::DictionaryBuilder;
use proc_macro2::TokenStream;

mod code_gen;

fn log_duration<T>(msg: &str, action: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let result = action();
    let elapsed = start.elapsed();
    eprintln!(
        "{msg} after {}.{}",
        elapsed.as_secs(),
        elapsed.subsec_millis()
    );
    result
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
    serde_serialize: bool,
    serde_deserialize: bool,
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
            serde_serialize: false,
            serde_deserialize: false,
        }
    }

    /// Path to FIXT transport XML (e.g., FIXT11.xml).
    /// Optional — omit for FIX versions that don't use FIXT (pre-FIX5).
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

    /// Generate `#[derive(serde::Serialize)]` on generated types.
    /// When true, the user must also enable `easyfix-core/serde-serialize` for core types.
    /// Default: false.
    pub fn serde_serialize(mut self, value: bool) -> Self {
        self.serde_serialize = value;
        self
    }

    /// Generate `#[derive(serde::Deserialize)]` on generated types.
    /// When true, the user must also enable `easyfix-core/serde-deserialize` for core types.
    /// Default: false.
    pub fn serde_deserialize(mut self, value: bool) -> Self {
        self.serde_deserialize = value;
        self
    }

    /// Run the generator. Returns error if required fields (fix_xml, output) are missing.
    pub fn generate(self) -> anyhow::Result<()> {
        let fix_xml = self.fix_xml.context("fix_xml path is required")?;
        let output = self.output.context("output path is required")?;

        let mut builder = DictionaryBuilder::new()
            .with_fix_xml(&fix_xml)
            .with_strict_check(true);

        if let Some(fixt_xml) = &self.fixt_xml {
            builder = builder.with_fixt_xml(fixt_xml);
        }

        let dictionary = builder.build()?;
        let generator = log_duration("Generator ready", || code_gen::Generator::new(&dictionary));

        let serde_ser = self.serde_serialize;
        let serde_de = self.serde_deserialize;

        let fields = log_duration("Fields token stream", || {
            generator.generate_fields(serde_ser, serde_de)
        });
        let groups = log_duration("Groups token stream", || {
            generator.generate_groups(serde_ser, serde_de)
        });
        let messages = log_duration("Messages token stream", || {
            generator.generate_messages(serde_ser, serde_de)
        });

        let combined = quote::quote! {
            #fields
            #groups
            #messages
        };

        let formatted = log_duration("Format", || format_token_stream(combined))?;
        let len = formatted.len();

        fs::write(&output, formatted)
            .with_context(|| format!("failed to write {}", output.display()))?;

        eprintln!("{}: {len} bytes written", output.display());

        Ok(())
    }
}
