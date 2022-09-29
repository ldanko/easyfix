mod gen;

use std::{
    error::Error,
    fs,
    io::prelude::*,
    path::Path,
    process::{Command, Stdio},
    time::Instant,
};

use easyfix_dictionary::Dictionary;
use proc_macro2::TokenStream;

use crate::gen::Generator;

fn create_source_file(
    tokens_stream: TokenStream,
    source_file: impl AsRef<Path>,
) -> Result<(), Box<dyn Error + 'static>> {
    let code = tokens_stream.to_string();

    let output = if true {
        let start = Instant::now();
        let mut rustfmt = Command::new("rustfmt")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to run rustfmt");
        rustfmt.stdin.take().unwrap().write_all(code.as_bytes())?;
        let output = rustfmt.wait_with_output()?;
        eprintln!("rustfmt output status: {:?}", output.status);
        let now = Instant::now() - start;
        eprintln!(
            "rustfmt done after {}.{}",
            now.as_secs(),
            now.subsec_millis()
        );
        if output.status.success() {
            output.stdout
        } else {
            std::io::stdout().write_all(&output.stdout)?;
            std::io::stderr().write_all(&output.stderr)?;
            std::process::exit(output.status.code().unwrap_or(1));
        }
    } else {
        code.into_bytes()
    };

    let mut file = fs::File::create(source_file.as_ref())?;
    file.write_all(&output)?;
    eprintln!(
        "{}: {} bytes written",
        source_file.as_ref().display(),
        output.len()
    );

    Ok(())
}

fn log_duration<T>(msg: &str, action: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let result = action();
    let now = Instant::now() - start;
    eprintln!("{msg} after {}.{}", now.as_secs(), now.subsec_millis());
    result
}

pub fn generate_fix_messages(
    fixt_xml_path: impl AsRef<Path>,
    fix_xml_path: impl AsRef<Path>,
    fields_file: impl AsRef<Path>,
    groups_file: impl AsRef<Path>,
    messages_file: impl AsRef<Path>,
) -> std::result::Result<(), Box<dyn std::error::Error + 'static>> {
    eprintln!("fields file path: {}", fields_file.as_ref().display());
    eprintln!("groups file path: {}", groups_file.as_ref().display());
    eprintln!("messages file path: {}", messages_file.as_ref().display());
    let fixt_xml = fs::read_to_string(fixt_xml_path)?;
    let fix_xml = fs::read_to_string(fix_xml_path)?;
    let mut dictionary = Dictionary::new();

    log_duration("FIXT XML processed", || {
        dictionary.process_fixt_xml(&fixt_xml)
    })?;

    log_duration("FIXT XML processed", || {
        dictionary.process_fix_xml(&fix_xml)
    })?;

    let generator = log_duration("Generator ready", || Generator::new(&dictionary));

    create_source_file(
        log_duration("Fields token stream", || generator.generate_fields()),
        fields_file,
    )?;
    create_source_file(
        log_duration("Groups token stream", || generator.generate_groups()),
        groups_file,
    )?;
    create_source_file(
        log_duration("Messages token stream", || generator.generate_messages()),
        messages_file,
    )?;

    Ok(())
}
