# arbos-revm

[![CI](https://github.com/iosiro/arbos-revm/actions/workflows/ci.yml/badge.svg)](https://github.com/iosiro/arbos-revm/actions/workflows/ci.yml)

Arbitrum EVM implementation built on [revm](https://github.com/bluealloy/revm). This crate provides a complete execution environment for the Arbitrum blockchain, supporting both standard EVM bytecode and WebAssembly (Stylus) smart contracts.

Developed by [iosiro](https://iosiro.com) as part of the [Arbitrum Stylus Sprint](https://blog.arbitrum.io/stylus-sprint/).

## Features

- **Full EVM Compatibility** - Complete Ethereum opcode execution via revm
- **Stylus/WebAssembly Support** - Native WASM execution using the Wasmer runtime with gas-to-ink conversion
- **Arbitrum Precompiles** - 13 Arbitrum-specific precompiles for chain management
- **L1/L2 Fee Model** - Dual-layer pricing with L1 data costs and L2 gas pricing
- **Retryable Transactions** - L1→L2 cross-chain message support with auto-redeem
- **Address Compression** - Address table for calldata optimization
- **Program Caching** - LRU-based caching for compiled Stylus programs

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
arbos-revm = { git = "https://github.com/iosiro/arbos-revm", version = "0.1.0" }
```

## Usage

### Basic EVM Execution

```rust
use arbos_revm::{ArbitrumEvm, ArbitrumContext, ArbitrumTransaction};
use revm::primitives::{Address, U256};

// Create the EVM with Arbitrum configuration
let mut evm = ArbitrumEvm::new(context);

// Execute a transaction
let result = evm.transact_one()?;

// Finalize and get state changes
let output = evm.finalize();
```

### Transaction Types

arbos-revm supports multiple transaction types:

| Type | Code | Description |
|------|------|-------------|
| Legacy | 0 | Standard Ethereum transaction |
| EIP-2930 | 1 | Access list transaction |
| EIP-1559 | 2 | Fee market transaction |
| Retryable | 111 | L1→L2 cross-chain message |
| Internal | 119 | L2 system call |
| Deposit | 120 | L1→L2 value transfer |

### Stylus Programs

Execute WebAssembly smart contracts with automatic gas-to-ink conversion:

```rust
use arbos_revm::stylus_executor::StylusExecutor;

// Programs are automatically cached using LRU eviction
// Gas is converted to ink (1 gas ≈ 10,000 ink)
// Memory growth is tracked and limited (max 128 pages / 8MB)
```

## Architecture

```
┌─────────────────────────────────────┐
│         ArbitrumEvm                 │
├─────────────────────────────────────┤
│   Context (Config, Tx, Block)       │
├─────────────────────────────────────┤
│   Handler (Execution Logic)         │
├─────────────────────────────────────┤
│   Precompiles (13 Arbitrum-specific)│
├─────────────────────────────────────┤
│   State (ArbOS, Pricing, Programs)  │
├─────────────────────────────────────┤
│   Stylus (WASM Runtime + Cache)     │
└─────────────────────────────────────┘
```

### Core Components

| Module | Description |
|--------|-------------|
| `evm` | Main `ArbitrumEvm` entry point |
| `context` | Chain configuration and transaction context |
| `handler` | Transaction execution and gas handling |
| `precompiles` | Arbitrum-specific precompiled contracts |
| `state` | ArbOS persistent state management |
| `stylus_executor` | WebAssembly execution engine |
| `l1_fee` | L1 data cost calculation |

### Precompiles

Arbitrum precompiles are available at reserved addresses (`0x64`-`0x73`):

| Address | Contract | Purpose |
|---------|----------|---------|
| 0x64 | ArbSys | System parameters and L2 block info |
| 0x65 | ArbInfo | Chain information queries |
| 0x66 | ArbAddressTable | Address compression |
| 0x67 | ArbBLS | BLS signature utilities (deprecated) |
| 0x68 | ArbFunctionTable | Function table (deprecated) |
| 0x69 | ArbosTest | Test utilities |
| 0x6b | ArbOwnerPublic | Public admin info |
| 0x6c | ArbGasInfo | Gas pricing and L1 fees |
| 0x6d | ArbAggregator | Preferred aggregator |
| 0x6e | ArbRetryableTx | Retryable transaction management |
| 0x6f | ArbStatistics | Block statistics |
| 0x70 | ArbOwner | Admin functions (restricted) |
| 0x71 | ArbWasm | Stylus program execution |
| 0x72 | ArbWasmCache | Program caching control |
| 0x73 | ArbDebug | Debug utilities |

## Configuration

### ArbOS Constants

Key parameters in `constants.rs`:

| Constant | Value | Description |
|----------|-------|-------------|
| `ARBOS_VERSION` | 42 | ArbOS version |
| `STYLUS_VERSION` | 2 | Stylus runtime version |
| `MAX_WASM_SIZE` | 128 KB | Maximum decompressed WASM size |
| `INK_PRICE` | 10,000 | Gas-to-ink ratio |
| `RETRYABLE_LIFETIME` | 7 days | Default retryable expiry |
| `PROGRAM_EXPIRY` | 365 days | Stylus program cache expiry |

### Features

```toml
[features]
serde = ["dep:serde"]  # Enable serialization support
```

## Development

### Prerequisites

- Rust 2024 edition (nightly required for edition 2024)
- Cargo

### Building

```bash
cargo build --release
```

### Testing

```bash
cargo test --all-features
```

### Linting

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
```

### Documentation

```bash
cargo doc --no-deps --all-features
```

## License

Licensed under the MIT License. See [LICENSE](LICENSE) for details.

## Related Projects

- [revm](https://github.com/bluealloy/revm) - Rust EVM implementation
- [Arbitrum Nitro](https://github.com/OffchainLabs/nitro) - Arbitrum's fraud-proof system
- [Stylus](https://docs.arbitrum.io/stylus/stylus-gentle-introduction) - WebAssembly smart contracts on Arbitrum

## Contributing

Contributions are welcome! Please ensure:

1. Code passes `cargo fmt` and `cargo clippy`
2. All tests pass with `cargo test --all-features`
3. Documentation builds without warnings
