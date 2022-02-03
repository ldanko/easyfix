mod gen;

use crate::gen::Generator;
use easyfix_dictionary::Dictionary;
use std::{
    fs,
    io::prelude::*,
    path::Path,
    process::{Command, Stdio},
    time::Instant,
};

pub fn generate_fix_messages(
    fixt_xml_path: impl AsRef<Path>,
    fix_xml_path: impl AsRef<Path>,
    out_file: impl AsRef<Path>,
) -> std::result::Result<(), Box<dyn std::error::Error + 'static>> {
    let start = Instant::now();
    eprintln!("> OUT_FILE: {}", out_file.as_ref().display());
    let fixt_xml = fs::read_to_string(fixt_xml_path)?;
    let fix_xml = fs::read_to_string(fix_xml_path)?;
    let mut dictionary = Dictionary::new();
    dictionary.process_fixt_xml(&fixt_xml)?;
    let now = Instant::now() - start;
    eprintln!(
        "FIXT processed after {}.{}",
        now.as_secs(),
        now.subsec_millis()
    );
    dictionary.process_fix_xml(&fix_xml)?;
    let now = Instant::now() - start;
    eprintln!(
        "FIX processed after {}.{}",
        now.as_secs(),
        now.subsec_millis()
    );

    let generator = Generator::new(&dictionary);
    let now = Instant::now() - start;
    eprintln!(
        "Generator ready after {}.{}",
        now.as_secs(),
        now.subsec_millis()
    );
    let tokens_stream = generator.generate();
    let now = Instant::now() - start;
    eprintln!(
        "Token stream ready after {}.{}",
        now.as_secs(),
        now.subsec_millis()
    );
    let code = tokens_stream.to_string();

    if true {
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
            let mut file = fs::File::create(out_file)?;
            eprintln!("write to file: {:?}", file);
            eprintln!("stdout len: {}", output.stdout.len());
            file.write_all(&output.stdout)?;
        } else {
            std::io::stdout().write_all(&output.stdout)?;
            std::io::stderr().write_all(&output.stderr)?;
            std::process::exit(output.status.code().unwrap_or(1));
        }
    } else {
        let mut file = fs::File::create(out_file)?;
        eprintln!("write to file: {:?}", file);
        eprintln!("stdout len: {}", code.len());
        file.write_all(code.as_bytes())?;
    }

    Ok(())
}
