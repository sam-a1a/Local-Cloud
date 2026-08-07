//! Generates the Swift and Kotlin bindings from the compiled library.
//!
//! uniffi reads the interface out of the built artefact rather than from a
//! separate description, so this cannot describe something the Rust does not
//! actually export - the bindings and the implementation cannot drift apart.
//!
//!   cargo run -p engine --bin uniffi-bindgen -- generate \
//!     --library target/debug/liblocalcloud.dylib \
//!     --language swift --out-dir bindings/swift
fn main() {
    uniffi::uniffi_bindgen_main()
}
