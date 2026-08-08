//! Defines the `desktop` cfg: a platform where watching a folder is possible.
//!
//! Spelling out `not(any(target_os = "ios", target_os = "android"))` at every
//! use makes the condition look incidental. It is one decision - §8 of
//! DESIGN.md - and it should read as one.
fn main() {
    println!("cargo::rustc-check-cfg=cfg(desktop)");
    let target = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if !matches!(target.as_str(), "ios" | "android") {
        println!("cargo::rustc-cfg=desktop");
    }
}
