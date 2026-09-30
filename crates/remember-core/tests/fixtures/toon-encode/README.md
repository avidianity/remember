# TOON encode fixtures

Vendored from the official TOON spec test suite (https://github.com/toon-format/spec, commit `d6db4b04`, spec v4.1), as shipped in `serde_toon_format` 0.2.0.
They are licensed MIT by the TOON format authors.
`tests/toon_conformance.rs` runs every fixture through `remember_core::toon::encode` so an encoder swap or upgrade cannot silently break spec conformance.
