//! Utility functions for WASM and compression operations.
//!
//! This module provides utilities for:
//! - `wat2wasm` for WebAssembly text format conversion
//! - `brotli_compress` and `brotli_decompress` for Brotli compression
//! - WASM stripping functions for Stylus deployment

use wasm_encoder::{Module, RawSection};
use wasmparser::{Parser, Payload};

pub use stylus::brotli::{
    Dictionary, compress as brotli_compress, decompress as brotli_decompress,
};
pub use wasmer::wat2wasm;

/// Error type for WASM stripping operations.
#[derive(Debug, thiserror::Error)]
pub enum StripWasmError {
    #[error("failed to parse WASM: {0}")]
    Parse(#[from] wasmparser::BinaryReaderError),
    #[error("failed to convert WASM to WAT: {0}")]
    Wasm2Wat(String),
    #[error("failed to convert WAT to WASM: {0}")]
    Wat2Wasm(String),
}

/// Strips user metadata and dangling reference types from a WASM binary.
///
/// This function prepares a WASM binary for Stylus deployment by:
/// 1. Removing custom and unknown sections that may contain sensitive metadata
/// 2. Converting WASM to WAT and back to remove dangling reference types
///    that are not yet supported by Arbitrum chain backends
///
/// # Arguments
/// * `wasm` - The raw WASM binary bytes
///
/// # Returns
/// The stripped WASM binary bytes, or an error if processing fails
pub fn strip_wasm_for_stylus(wasm: impl AsRef<[u8]>) -> Result<Vec<u8>, StripWasmError> {
    // Step 1: Strip custom and unknown sections to remove sensitive metadata
    let stripped = strip_user_metadata(wasm.as_ref())?;

    // Step 2: Convert WASM to WAT and back to remove dangling reference types
    let cleaned = remove_dangling_references(&stripped)?;

    Ok(cleaned)
}

/// Strip all custom and unknown sections from a WASM binary.
///
/// This removes any user metadata which we do not want to leak as part of the final binary.
fn strip_user_metadata(
    wasm_file_bytes: impl AsRef<[u8]>,
) -> Result<Vec<u8>, wasmparser::BinaryReaderError> {
    let mut module = Module::new();
    let parser = Parser::new(0);
    for payload in parser.parse_all(wasm_file_bytes.as_ref()) {
        match payload? {
            Payload::CustomSection { .. } => {
                // Skip custom sections to remove sensitive metadata
            }
            Payload::UnknownSection { .. } => {
                // Skip unknown sections that might contain sensitive data
            }
            item => {
                if let Some((id, range)) = item.as_section() {
                    let data = &wasm_file_bytes.as_ref()[range];
                    let raw_section = RawSection { id, data };
                    module.section(&raw_section);
                }
            }
        }
    }
    Ok(module.finish())
}

/// Convert WASM from binary to text and back to binary.
///
/// This trick removes any dangling mentions of reference types in the WASM body,
/// which are not yet supported by Arbitrum chain backends.
fn remove_dangling_references(wasm: impl AsRef<[u8]>) -> Result<Vec<u8>, StripWasmError> {
    let wat_string =
        wasmprinter::print_bytes(wasm).map_err(|e| StripWasmError::Wasm2Wat(e.to_string()))?;
    let wasm = wasmer::wat2wasm(wat_string.as_bytes())
        .map_err(|e| StripWasmError::Wat2Wasm(e.to_string()))?;
    Ok(wasm.to_vec())
}
