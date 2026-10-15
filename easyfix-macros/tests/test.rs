use std::{fs, path::Path, process::Command};

#[test]
fn test() {
    let t = trybuild::TestCases::new();
    t.pass("tests/scenarios/compile_ok.rs");
    t.compile_fail("tests/scenarios/fail_on_multibyte_character.rs");
    t.compile_fail("tests/scenarios/fail_on_control_character.rs");
    t.compile_fail("tests/scenarios/fail_on_delete_character.rs");
}

#[test]
fn reexport_without_direct_easyfix_dependency() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let project = root.join("target/tests/fix-str-reexport");
    fs::create_dir_all(project.join("bridge/src")).unwrap();
    fs::create_dir_all(project.join("consumer/src")).unwrap();
    fs::write(
        project.join("Cargo.toml"),
        "[workspace]\nmembers = [\"bridge\", \"consumer\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::write(
        project.join("bridge/Cargo.toml"),
        format!(
            "[package]\nname = \"fix-str-bridge\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\
             [dependencies]\nrenamed = {{ package = \"easyfix\", path = {:?} }}\n",
            root.join("easyfix"),
        ),
    )
    .unwrap();
    fs::write(
        project.join("bridge/src/lib.rs"),
        "pub use renamed::{basic_types::FixStr, fix_str};\n\
         pub const VALUE: &FixStr = fix_str!(\"BRIDGE\");\n",
    )
    .unwrap();
    fs::write(
        project.join("consumer/Cargo.toml"),
        "[package]\nname = \"fix-str-consumer\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\
         [dependencies]\nfix-str-bridge = { path = \"../bridge\" }\n",
    )
    .unwrap();
    fs::write(
        project.join("consumer/src/main.rs"),
        "use fix_str_bridge::{FixStr, fix_str};\n\
         const VALUE: &FixStr = fix_str!(\"CONSUMER\");\n\
         fn main() {\n\
             assert_eq!(VALUE.as_bytes(), b\"CONSUMER\");\n\
             assert_eq!(fix_str_bridge::VALUE.as_bytes(), b\"BRIDGE\");\n\
             assert_eq!(fix_str!(\"runtime\").as_bytes(), b\"runtime\");\n\
         }\n",
    )
    .unwrap();

    // A separate manifest is essential: trybuild inherits this crate's direct
    // easyfix-core dependency, which would hide the original lookup failure.
    let output = Command::new(env!("CARGO"))
        .args(["run", "--offline", "--quiet", "-p", "fix-str-consumer"])
        .arg("--manifest-path")
        .arg(project.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(project.join("target"))
        .output()
        .expect("run the consumer without a direct easyfix dependency");
    assert!(
        output.status.success(),
        "consumer failed: {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );
}
