# Foundry and REVM migration

## Baseline

- `arbos-revm` now builds on REVM 42.0.1 and owns an `ArbitrumTransaction`,
  `ArbitrumConfig`, `ArbitrumLocalContext`, `ArbitrumHandler`, precompile provider,
  and `ArbitrumEvm` wrapper.
- The reference Foundry checkout at
  `7cd5b431a08e023533fcc898e737c9c37e9ee86d` uses REVM 42.0.1.
- Foundry now selects non-Ethereum execution at one outer boundary through
  `FoundryEvmNetwork` and `FoundryEvmFactory`. The factory owns the concrete spec,
  block, transaction, halt, precompile, context, journal, and EVM types.
- `FoundryChain<Tx>` supplies transaction-position state through `for_transaction`,
  `for_block`, and `refresh_journal`. Executors, replay, nested EVMs, inspectors,
  and backends remain generic over the already-selected execution family.
- Foundry's existing `evm/networks/arbitrum.rs` is only a minimal Ethereum-family
  `ArbSys.arbBlockNumber` compatibility precompile. It is not an ArbOS execution
  family and must not be extended into a second, competing implementation.

## Ownership model

| Data or behavior | Owner after migration |
| --- | --- |
| RPC request, signed envelope, receipt | Arbitrum Alloy `Network` integration |
| ArbOS version/spec and EVM construction | Arbitrum `FoundryEvmFactory` |
| Canonical hash and encoded transaction | `ArbitrumTransaction` |
| L1 poster and delayed-inbox provenance | `ArbitrumTransaction` when envelope-derived; chain context when position-derived |
| Parent/current block transactions and active index | `ArbitrumChain` |
| Parent-frame caller and direct call scheme | Arbitrum frame/local execution state |
| Scheduled retry queue and current retryable | Arbitrum chain/journal state with explicit commit/revert lifecycle |
| ArbOS account storage | REVM journal/database |
| Poster-gas reservation and fee settlement | `ArbitrumHandler` before first-frame validation and at result settlement |
| ArbOS precompiles and Stylus execution | Arbitrum EVM factory, handler, and precompile provider |
| Tool selection | Foundry `NetworkConfigs`, dispatched once at the tool boundary |

Auxiliary state must be copied into nested EVMs, included in snapshots, restored on
revert, reset on fork changes, and refreshed during block replay. Consensus-visible
state must not live solely in an inspector or process-global cache.

## Incremental migration

Stages 1 through 3 are implemented in `arbos-revm`. Stage 4 is implemented on the
`revm42-foundry-factory` branch of `arbos-foundry`, including Forge, Cast, Anvil,
Chisel, scripts, replay, nested EVMs, and the Stylus configuration/cheatcode surface.

### 1. REVM 42 compatibility

- Update the pinned REVM dependency to the version used by reference Foundry.
- Adapt `ContextTr`, `EvmTr`, frame, handler, inspector, precompile, gas-tracker,
  transaction, and execution-result APIs without changing ArbOS behavior.
- Keep the existing suite and Nitro-alignment vectors green before integrating
  Foundry.

### 2. Complete Arbitrum execution inputs

- Add a canonical transaction hash, typed provenance, and explicit run mode to
  `ArbitrumTransaction`; never reconstruct a canonical hash from incomplete
  `TxEnv` fields.
- Add frame metadata that records every caller and the direct `CallScheme`, distinct
  from inherited static state.
- Introduce a journal-aware scheduled transaction queue and `CurrentRetryable`.
- Represent committed system-transaction failures independently from fatal EVM or
  database errors.

### 3. Handler lifecycle

- Calculate poster cost during validation, reserve its gas before initial-gas
  checks, and settle/refund it through the same result lifecycle as execution gas.
- Route ordinary filtering from the canonical transaction identity and provenance.
- Execute scheduled retries deterministically after their scheduling transaction,
  preserving group rollback and receipt-level failure behavior.

### 4. Foundry execution family

- Implement one Arbitrum `FoundryEvmNetwork`/`FoundryEvmFactory` path rather than
  adding Arbitrum branches to generic Ethereum executors or backends.
- Convert signed/RPC envelopes to `ArbitrumTransaction` at the factory boundary.
- Implement `ArbitrumChain::for_block` and `refresh_journal` for replay and scheduled
  execution context.
- Thread the concrete family through Forge, Cast, Anvil, Chisel, scripts, nested
  EVMs, snapshots, fork reset/roll, traces, calls, estimates, and replay.
- Remove or delegate Foundry's minimal ArbSys compatibility precompile once the full
  family is selected, so only one ArbSys implementation is active.
- Initialize empty local backends from the selected ArbOS version and all v61 Stylus
  parameters while preserving initialized fork state. Runtime debug, auto-cache, and
  auto-activation controls travel in `ArbitrumChain`, so nested EVM construction does
  not lose them through an Ethereum `CfgEnv` projection.
- Expose Brotli, Stylus runtime/init-code, and CREATE/CREATE2 deployment cheatcodes.
  Deployment is verified by activating and executing a real vm-hooks echo module.
- Generate the WASM test fixture from checked-in WAT with the standalone `wat` parser;
  do not pull the full Stylus runtime into the build-dependency graph.

### 5. Close the recorded limitations

- ARBOS-023: return committed receipt failures with an Arbitrum-specific typed
  status, without treating them as fatal EVM errors.
- ARBOS-028: derive `myCallersAddressWithoutAliasing` from the actual parent frame.
- ARBOS-045: propagate the Stylus filter result through a backend-visible outcome.
- ARBOS-058: distinguish batch, delayed-inbox, and exempt transaction provenance.
- ARBOS-059: use direct `CallScheme` rather than inherited static state for ArbOS
  precompile mutability behavior.
- ARBOS-075: set and clear `CurrentRetryable` around scheduled retry execution and
  reject self-modification.

## Verification gates

Each stage must retain the existing tests and add functional coverage for:

- complete retryable scheduling, execution, rollback, cancellation, and reaping;
- behavior immediately before and after ArbOS upgrade boundaries;
- Stylus activation/cache invalidation across version and parameter changes;
- multiple transactions sharing a block gas budget;
- sequential L1 pricing reports and all affected balances/state;
- canonical-hash filtering, delayed provenance, nested callers, and call schemes;
- auxiliary state across commit, revert, nested execution, snapshots, and replay.

Completion requires pinned dependencies, formatting and diff checks, the complete
test suite, and clippy with warnings denied. Multidimensional gas and Wasmer 7 are
separate follow-ups, but their future inputs must not require another context
architecture replacement.
