use std::{env, path::PathBuf};

use easyfix_messages::Generator;

fn main() {
    println!("cargo:rerun-if-changed=xml/FIXT11-mini.xml");
    println!("cargo:rerun-if-changed=xml/FIX50SP2-mini.xml");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    Generator::new()
        .fixt_xml("xml/FIXT11-mini.xml")
        .fix_xml("xml/FIX50SP2-mini.xml")
        .output(out_dir.join("messages.rs"))
        .generate()
        .expect("FIX message generation failed");
}
