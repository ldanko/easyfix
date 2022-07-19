use easyfix_messages_gen::generate_fix_messages;
use std::{env, path::PathBuf};

fn main() {
    let dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let out_path = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR undefined"));
    let fixt_xml_path = env::var("FIXT_XML").unwrap_or_else(|_| format!("{}/xml/FIXT11.xml", dir));
    let fix_xml_path = env::var("FIX_XML").unwrap_or_else(|_| format!("{}/xml/FIX50SP2.xml", dir));
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", fixt_xml_path);
    println!("cargo:rerun-if-changed={}", fix_xml_path);
    generate_fix_messages(
        fixt_xml_path,
        fix_xml_path,
        out_path.join("generated_fields.rs"),
        out_path.join("generated_groups.rs"),
        out_path.join("generated_messages.rs"),
    )
    .expect("failed to generate FIX messages");
}
