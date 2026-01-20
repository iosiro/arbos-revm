# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2024-02-05

### Added

- Initial release of arbos-revm
- Full EVM execution support via revm v33.0.0
- Stylus/WebAssembly smart contract execution using Wasmer runtime
- Gas-to-ink conversion for WASM execution metering
- LRU-based program caching (1024 entries)
- 13 Arbitrum-specific precompiles:
  - ArbSys - System parameters and L2 block info
  - ArbInfo - Chain information queries
  - ArbAddressTable - Address compression
  - ArbOwner/ArbOwnerPublic - Admin functions
  - ArbGasInfo - Gas pricing and L1 fees
  - ArbAggregator - Preferred aggregator info
  - ArbRetryableTx - Retryable transaction management
  - ArbStatistics - Block statistics
  - ArbWasm/ArbWasmCache - Stylus program management
  - ArbDebug - Debug utilities
- L1/L2 dual-layer fee model with:
  - L1 data cost calculation (EIP-2028)
  - Dynamic L2 gas pricing based on congestion
  - Batch poster reward distribution
- Transaction type support:
  - Legacy (Type 0)
  - EIP-2930 Access List (Type 1)
  - EIP-1559 Fee Market (Type 2)
  - Retryable (Type 111)
  - Internal (Type 119)
  - Deposit (Type 120)
- ArbOS state management:
  - L1/L2 pricing state
  - Retryable transaction queue
  - Address table
  - Block hash history
  - Program registry
- Comprehensive test suite with 100+ test cases
