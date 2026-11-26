//! zrip pins its sub-crates to the exact versions it is released with. With a caret requirement,
//! a sub-crate release that moves to a new zrip-core pairs with an older zrip whose own
//! zrip-core types no longer match, and that zrip stops building on a fresh resolution.

#[test]
fn sub_crates_are_pinned_exactly() {
    let manifest = include_str!("../Cargo.toml");
    let deps = manifest
        .split("\n[dependencies]\n")
        .nth(1)
        .and_then(|rest| rest.split("\n[").next())
        .expect("a [dependencies] table");
    for name in ["zrip-core", "zrip-encode", "zrip-decode"] {
        let line = deps
            .lines()
            .find(|l| l.starts_with(&format!("{name} = ")))
            .unwrap_or_else(|| panic!("no {name} dependency"));
        assert!(
            line.contains("version = \"="),
            "{name} is not pinned exactly: {line}"
        );
    }
}
