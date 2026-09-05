use std::fmt::Display;

use alloy_sol_types::{SolError, sol};
use revm::{
    context::JournalTr,
    interpreter::{
        Gas, InstructionResult, InterpreterAction, InterpreterResult,
        gas::{ISTANBUL_SLOAD_GAS, SSTORE_RESET, SSTORE_SET},
    },
    primitives::{
        Address, B256, Bytes, FixedBytes, I256, U256,
        alloy_primitives::Signed,
        keccak256,
        ruint::{Uint, UintTryTo},
    },
};

use crate::{ArbitrumContextTr, config::ArbitrumConfigTr, constants::ARBOS_STATE_ADDRESS};
use tracing::trace;

sol! {
    error ProgramNotActivated();
    error ProgramNeedsUpgrade(uint16 version, uint16 stylusVersion);
    error ProgramExpired(uint64 ageInSeconds);
}
#[derive(Debug)]
pub enum ArbosStateError {
    OutOfGas,
    StateChangeDuringStaticCall,
    InvalidBlockNumberForBlockHash,
    DecompressError(String),
    ProgramNotActivated,
    ProgramNeedsUpgrade(u16, u16),
    ProgramExpired(u64),
    RectifyMappingNotOwner,
    RectifyMappingNoChange,
    Context(String),
    UnsupportedArbosVersion(u64),
    ArbosVersionDowngrade { current: u64, requested: u64 },
    UnexpectedStylusVersion { current: u16, requested: u16 },
}

impl Display for ArbosStateError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::OutOfGas => write!(f, "Out of gas"),
            Self::StateChangeDuringStaticCall => {
                write!(f, "State change attempted during static call")
            }
            Self::InvalidBlockNumberForBlockHash => {
                write!(f, "Invalid block number for block hash")
            }
            Self::DecompressError(msg) => {
                write!(f, "Decompression error: {msg}")
            }
            Self::ProgramNotActivated => {
                write!(f, "Program not activated")
            }
            Self::ProgramNeedsUpgrade(current, required) => {
                write!(
                    f,
                    "Program needs upgrade from version {current} to {required}"
                )
            }
            Self::ProgramExpired(expired_at) => {
                write!(f, "Program expired at {expired_at}")
            }
            Self::RectifyMappingNotOwner => {
                write!(f, "RectifyMapping: Address is not an owner")
            }
            Self::RectifyMappingNoChange => {
                write!(f, "RectifyMapping: Owner address is correctly mapped")
            }
            Self::Context(err) => {
                write!(f, "Context error: {err}")
            }
            Self::UnsupportedArbosVersion(version) => {
                write!(f, "unsupported ArbOS version {version}")
            }
            Self::ArbosVersionDowngrade { current, requested } => {
                write!(f, "cannot downgrade ArbOS from {current} to {requested}")
            }
            Self::UnexpectedStylusVersion { current, requested } => {
                write!(f, "cannot upgrade Stylus from {current} to {requested}")
            }
        }
    }
}

impl From<ArbosStateError> for String {
    fn from(error: ArbosStateError) -> Self {
        error.to_string()
    }
}

impl From<ArbosStateError> for Bytes {
    fn from(error: ArbosStateError) -> Self {
        match error {
            ArbosStateError::ProgramNotActivated => ProgramNotActivated {}.abi_encode().into(),
            ArbosStateError::ProgramNeedsUpgrade(version, stylus_version) => ProgramNeedsUpgrade {
                version,
                stylusVersion: stylus_version,
            }
            .abi_encode()
            .into(),
            ArbosStateError::ProgramExpired(age) => {
                ProgramExpired { ageInSeconds: age }.abi_encode().into()
            }
            _ => Self::from(error.to_string().into_bytes()),
        }
    }
}

impl From<ArbosStateError> for InterpreterAction {
    fn from(error: ArbosStateError) -> Self {
        Self::Return(error.into())
    }
}

impl From<ArbosStateError> for InstructionResult {
    fn from(error: ArbosStateError) -> Self {
        let description = error.to_string();
        let result = match error {
            ArbosStateError::OutOfGas => Self::OutOfGas,
            ArbosStateError::StateChangeDuringStaticCall => Self::StateChangeDuringStaticCall,
            ArbosStateError::Context(_) => Self::Revert,
            _ => Self::Revert,
        };

        trace!(
            target: "arbos-revm::arbos_state",
            %description,
            ?result,
            "Converted ArbosStateError into InstructionResult"
        );

        result
    }
}

impl From<ArbosStateError> for InterpreterResult {
    fn from(error: ArbosStateError) -> Self {
        let description = error.to_string();
        let result = match error {
            ArbosStateError::OutOfGas => Self {
                result: InstructionResult::OutOfGas,
                gas: Gas::default(),
                output: Bytes::default(),
            },
            ArbosStateError::StateChangeDuringStaticCall => Self {
                result: InstructionResult::StateChangeDuringStaticCall,
                gas: Gas::default(),
                output: Bytes::default(),
            },
            ArbosStateError::RectifyMappingNoChange | ArbosStateError::RectifyMappingNotOwner => {
                Self {
                    result: InstructionResult::Revert,
                    gas: Gas::default(),
                    output: Bytes::from(description.clone().into_bytes()),
                }
            }
            other => Self {
                result: InstructionResult::Revert,
                gas: Gas::default(),
                output: Bytes::from(other),
            },
        };

        trace!(
            target: "arbos-revm::arbos_state",
            %description,
            result = ?result.result,
            output_len = result.output.len(),
            "Converted ArbosStateError into InterpreterResult"
        );

        result
    }
}

pub struct StorageBacked<'a, CTX, T> {
    pub context: &'a mut CTX,
    pub gas: Option<&'a mut Gas>,
    pub is_static: bool,
    pub slot: B256,
    _marker: core::marker::PhantomData<T>,
}

pub trait StorageBackedTr<'a, CTX, T>
where
    CTX: ArbitrumContextTr,
    T: StorageWord,
{
    fn new(context: &'a mut CTX, gas: Option<&'a mut Gas>, is_static: bool, slot: B256) -> Self;
    fn get(&mut self) -> Result<T, ArbosStateError>;
    fn set(&mut self, value: T) -> Result<(), ArbosStateError>;
}

// --- utility helpers moved to module scope ---
pub fn substorage(root: &B256, index: &[u8]) -> B256 {
    let mut subkey_bytes = if root.is_zero() {
        Vec::with_capacity(1)
    } else {
        root.as_slice().to_vec()
    };
    subkey_bytes.extend_from_slice(index);
    keccak256(subkey_bytes)
}

pub fn map_address(storage_key: &B256, key: &B256) -> B256 {
    let key_bytes = key.as_slice();
    let boundary = key_bytes.len() - 1;

    let mut to_hash = Vec::with_capacity(storage_key.len() + boundary);
    if !storage_key.is_zero() {
        to_hash.extend_from_slice(storage_key.as_slice());
    }
    to_hash.extend_from_slice(&key_bytes[..boundary]);

    let digest = keccak256(&to_hash);

    let mut mapped = digest[..boundary].to_vec();
    mapped.push(key_bytes[boundary]);
    B256::from_slice(&mapped)
}

pub type StorageBackedU256<'a, CTX> = StorageBacked<'a, CTX, U256>;
pub type StorageBackedU32<'a, CTX> = StorageBacked<'a, CTX, u32>;
pub type StorageBackedU64<'a, CTX> = StorageBacked<'a, CTX, u64>;
pub type StorageBackedI64<'a, CTX> = StorageBacked<'a, CTX, i64>;
pub type StorageBackedI256<'a, CTX> = StorageBacked<'a, CTX, I256>;
pub type StorageBackedAddress<'a, CTX> = StorageBacked<'a, CTX, Address>;
pub type StorageBackedAddressOrNil<'a, CTX> = StorageBacked<'a, CTX, Option<Address>>;
pub type StorageBackedB256<'a, CTX> = StorageBacked<'a, CTX, B256>;

/// Anything that can round-trip through a single storage word (U256).
pub trait StorageWord: Sized {
    /// Decode from the raw storage word.
    fn from_word(word: U256) -> Self;

    /// Encode into the raw storage word.
    fn into_word(self) -> U256;
}

impl<const BITS: usize, const LIMBS: usize> StorageWord for Uint<BITS, LIMBS>
where
    // U256 → this Uint
    U256: UintTryTo<Self>,
    // this Uint → U256
    Self: UintTryTo<U256>,
{
    fn from_word(word: U256) -> Self {
        // Saturating, so small types don't panic on overflow
        word.to::<Self>()
    }

    fn into_word(self) -> U256 {
        self.to::<U256>()
    }
}

impl StorageWord for Option<Address> {
    fn from_word(word: U256) -> Self {
        // Nitro reserves 2^255 to distinguish a nil retry target (contract
        // creation) from the real zero address.
        if word == (U256::ONE << 255) {
            None
        } else {
            Some(Address::from_word(word.into()))
        }
    }

    fn into_word(self) -> U256 {
        match self {
            Some(address) => U256::from_be_slice(address.as_slice()),
            None => U256::ONE << 255,
        }
    }
}

impl<const BITS: usize, const LIMBS: usize> StorageWord for Signed<BITS, LIMBS>
where
    // U256 → this Uint
    U256: UintTryTo<Uint<BITS, LIMBS>>,
    // this Uint → U256
    Uint<BITS, LIMBS>: UintTryTo<U256>,
{
    fn from_word(word: U256) -> Self {
        // Saturating, so small types don't panic on overflow
        let word = word.to::<Uint<BITS, LIMBS>>();
        Self::from_raw(word)
    }

    fn into_word(self) -> U256 {
        U256::from(self.into_raw())
    }
}

impl<const N: usize> StorageWord for FixedBytes<N>
where
    // these bounds express “Alloy knows how to map
    // between this and B256”
    B256: From<Self>,
    Self: From<B256>,
{
    fn from_word(word: U256) -> Self {
        Self(word.to_be_bytes())
    }

    fn into_word(self) -> U256 {
        let b256: B256 = B256::from(self);
        U256::from_be_slice(b256.as_slice())
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod storage_word_tests {
    use super::*;
    use revm::primitives::address;

    #[test]
    fn optional_address_uses_nitro_nil_sentinel() {
        assert_eq!(Option::<Address>::into_word(None), U256::ONE << 255);
        assert_eq!(Option::<Address>::from_word(U256::ONE << 255), None);

        let zero = Some(Address::ZERO);
        assert_eq!(zero.into_word(), U256::ZERO);
        assert_eq!(Option::<Address>::from_word(U256::ZERO), zero);

        let address = address!("1234567890123456789012345678901234567890");
        assert_eq!(
            Option::<Address>::from_word(Some(address).into_word()),
            Some(address)
        );
    }
}

impl StorageWord for Address {
    fn from_word(word: U256) -> Self {
        let b256: B256 = B256::from(FixedBytes(word.to_be_bytes()));
        Self::from_slice(&b256.as_slice()[12..32])
    }

    fn into_word(self) -> U256 {
        let b256 = B256::from(U256::from_be_slice(self.as_slice()));
        U256::from_be_slice(b256.as_slice())
    }
}

impl StorageWord for u32 {
    fn from_word(word: U256) -> Self {
        word.saturating_to()
    }

    fn into_word(self) -> U256 {
        U256::from(self)
    }
}

impl StorageWord for u64 {
    fn from_word(word: U256) -> Self {
        word.saturating_to()
    }

    fn into_word(self) -> U256 {
        U256::from(self)
    }
}

impl StorageWord for i64 {
    fn from_word(word: U256) -> Self {
        word.as_limbs()[0] as i64
    }

    fn into_word(self) -> U256 {
        I256::try_from(self)
            .expect("i64 always fits I256")
            .into_raw()
    }
}

impl<'a, CTX, T> StorageBackedTr<'a, CTX, T> for StorageBacked<'a, CTX, T>
where
    CTX: ArbitrumContextTr,
    T: StorageWord,
{
    fn new(context: &'a mut CTX, gas: Option<&'a mut Gas>, is_static: bool, slot: B256) -> Self {
        Self {
            context,
            gas,
            is_static,
            slot,
            _marker: std::marker::PhantomData,
        }
    }

    fn get(&mut self) -> Result<T, ArbosStateError> {
        if let Some(gas) = &mut self.gas
            && !gas.record_cost(ISTANBUL_SLOAD_GAS)
        {
            return Err(ArbosStateError::OutOfGas);
        }

        // Assuming your journal sload returns something with `.data: U256`
        let word: U256 = self
            .context
            .journal_mut()
            .sload(ARBOS_STATE_ADDRESS, self.slot.into())
            .map(|s| s.data)
            .map_err(|err| ArbosStateError::Context(err.to_string()))?;

        Ok(T::from_word(word))
    }

    fn set(&mut self, value: T) -> Result<(), ArbosStateError> {
        if self.is_static {
            return Err(ArbosStateError::StateChangeDuringStaticCall);
        }

        let value = value.into_word();

        if let Some(gas) = &mut self.gas {
            if value.is_zero() {
                if !gas.record_cost(SSTORE_RESET) {
                    return Err(ArbosStateError::OutOfGas);
                }
            } else if !gas.record_cost(SSTORE_SET) {
                return Err(ArbosStateError::OutOfGas);
            }
        }

        self.context
            .sstore(ARBOS_STATE_ADDRESS, self.slot.into(), value)
            .ok_or_else(|| ArbosStateError::Context("ArbOS state sstore failed".into()))?;

        // Mark ARBOS_STATE_ADDRESS as touched so CacheDB::commit persists its storage changes.
        // Without this, sstore modifies the journal but the account remains untouched,
        // causing CacheDB::commit to skip the account and discard all storage writes.
        self.context
            .journal_mut()
            .touch_account(ARBOS_STATE_ADDRESS);

        Ok(())
    }
}
/// Storage-backed address set implemented as array-with-length at index 0. Values are left-padded
/// B256.
pub struct StorageBackedAddressSet<'a, CTX>
where
    CTX: ArbitrumContextTr,
{
    pub context: &'a mut CTX,
    pub gas: Option<&'a mut Gas>,
    pub is_static: bool,
    pub slot: B256,
}

impl<'a, CTX> StorageBackedAddressSet<'a, CTX>
where
    CTX: ArbitrumContextTr,
{
    pub fn new(
        context: &'a mut CTX,
        gas: Option<&'a mut Gas>,
        is_static: bool,
        slot: B256,
    ) -> Self {
        Self {
            context,
            gas,
            is_static,
            slot,
        }
    }

    fn size_slot(&self) -> B256 {
        map_address(&self.slot, &B256::from(U256::from(0u64)))
    }

    pub fn size(&mut self) -> Result<usize, ArbosStateError> {
        let size_slot = self.size_slot();
        StorageBackedU256::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            size_slot,
        )
        .get()
        .map(|v| v.saturating_to::<usize>())
    }

    pub fn all(&mut self) -> Result<Vec<Address>, ArbosStateError> {
        let n = self.size()?;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let slot = map_address(&self.slot, &B256::from(U256::from(i as u64 + 1)));
            let addr = StorageBackedAddress::new(
                self.context,
                self.gas.as_deref_mut(),
                self.is_static,
                slot,
            )
            .get()?;
            out.push(addr);
        }
        Ok(out)
    }

    /// Clears the enumerable list while preserving the reverse mapping.
    ///
    /// Nitro used this at ArbOS 11 so owners could repair reverse indices that
    /// older versions could leave stale.
    pub fn clear_list(&mut self) -> Result<(), ArbosStateError> {
        let size = self.size()?;
        for index in 1..=size {
            let slot = map_address(&self.slot, &B256::from(U256::from(index)));
            StorageBackedAddress::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
                .set(Address::ZERO)?;
        }
        let size_slot = self.size_slot();
        StorageBackedU256::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            size_slot,
        )
        .set(U256::ZERO)
    }

    pub fn contains(&mut self, address: Address) -> Result<bool, ArbosStateError> {
        let by_address = substorage(&self.slot, &[0]);
        let slot = map_address(&by_address, &B256::left_padding_from(address.as_slice()));
        let index =
            StorageBackedU256::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
                .get()?;
        Ok(!index.is_zero())
    }

    pub fn add(&mut self, address: Address) -> Result<(), ArbosStateError> {
        if self.contains(address)? {
            return Ok(());
        }

        // push to array
        let size = {
            let size_slot = self.size_slot();
            let mut size_slot = StorageBackedU256::new(
                self.context,
                self.gas.as_deref_mut(),
                self.is_static,
                size_slot,
            );
            let size = size_slot.get()? + U256::ONE;
            size_slot.set(size)?;
            size
        };

        let slot = map_address(&self.slot, &B256::from(size));
        StorageBackedAddress::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
            .set(address)?;

        // also set by-address index so contains() is O(1)
        let by_address = substorage(&self.slot, &[0]);
        StorageBackedU256::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            map_address(&by_address, &B256::left_padding_from(address.as_slice())),
        )
        .set(size)?;

        Ok(())
    }

    pub fn remove(&mut self, address: &Address) -> Result<(), ArbosStateError> {
        let arbos_version = self.context.cfg().arbos_version();
        let by_address = substorage(&self.slot, &[0]);
        let slot = StorageBackedU256::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            map_address(&by_address, &B256::left_padding_from(address.as_slice())),
        )
        .get()?
        .saturating_to::<usize>();

        if slot == 0 {
            return Ok(());
        }

        // clear by-address index
        StorageBackedU256::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            map_address(&by_address, &B256::left_padding_from(address.as_slice())),
        )
        .set(U256::ZERO)?;

        let size = self.size()?;
        if slot < size {
            let at_size_slot = map_address(&self.slot, &B256::from(U256::from(size as u64)));
            let at_size = StorageBackedAddress::new(
                self.context,
                self.gas.as_deref_mut(),
                self.is_static,
                at_size_slot,
            )
            .get()?;

            let slot_slot = map_address(&self.slot, &B256::from(U256::from(slot as u64)));
            StorageBackedAddress::new(
                self.context,
                self.gas.as_deref_mut(),
                self.is_static,
                slot_slot,
            )
            .set(at_size)?;

            // ArbOS before version 11 historically failed to repair the
            // reverse index of the member moved into the vacated slot.
            if arbos_version >= 11 {
                StorageBackedU256::new(
                    self.context,
                    self.gas.as_deref_mut(),
                    self.is_static,
                    map_address(&by_address, &B256::left_padding_from(at_size.as_slice())),
                )
                .set(U256::from(slot as u64))?;
            }
        }

        // clear last slot
        let last_slot = map_address(&self.slot, &B256::from(U256::from(size as u64)));
        StorageBackedAddress::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            last_slot,
        )
        .set(Address::ZERO)?;

        // decrement size
        let size_slot = self.size_slot();
        let mut size_slot = StorageBackedU256::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            size_slot,
        );

        let size = size_slot.get()? - U256::ONE;
        size_slot.set(size)?;

        Ok(())
    }

    pub fn rectify(&mut self, address: Address) -> Result<(), ArbosStateError> {
        if self.contains(address)? {
            return Err(ArbosStateError::RectifyMappingNotOwner);
        }

        let by_address = substorage(&self.slot, &[0]);
        let slot = map_address(&by_address, &B256::left_padding_from(address.as_slice()));
        let index =
            StorageBackedU256::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
                .get()?;

        let slot = map_address(&self.slot, &B256::from(index));
        let addr =
            StorageBackedAddress::new(self.context, self.gas.as_deref_mut(), self.is_static, slot)
                .get()?;
        let size = self.size()?;
        if addr == address && index.saturating_to::<usize>() <= size {
            return Err(ArbosStateError::RectifyMappingNoChange);
        }

        // clear by-address index
        StorageBackedU256::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            map_address(&by_address, &B256::left_padding_from(address.as_slice())),
        )
        .set(U256::ZERO)?;

        // push to array
        self.add(address)?;

        Ok(())
    }
}

pub struct StorageBackedBytes<'a, CTX>
where
    CTX: ArbitrumContextTr,
{
    pub context: &'a mut CTX,
    pub gas: Option<&'a mut Gas>,
    pub is_static: bool,
    pub slot: B256,
}

impl<'a, CTX> StorageBackedBytes<'a, CTX>
where
    CTX: ArbitrumContextTr,
{
    pub fn new(
        context: &'a mut CTX,
        gas: Option<&'a mut Gas>,
        is_static: bool,
        slot: B256,
    ) -> Self {
        Self {
            context,
            gas,
            is_static,
            slot,
        }
    }

    pub fn get(&mut self) -> Result<Vec<u8>, ArbosStateError> {
        let size_slot = map_address(&self.slot, &B256::from(U256::from(0u64)));
        let size = StorageBackedU256::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            size_slot,
        )
        .get()?;

        let size = size.to::<usize>();

        let mut out = Vec::with_capacity(size);
        let mut bytes_left = size;
        let mut word_offset = 1usize;
        while bytes_left >= 32 {
            let chunk_slot = map_address(&self.slot, &B256::from(U256::from(word_offset)));
            let chunk = StorageBackedB256::new(
                self.context,
                self.gas.as_deref_mut(),
                self.is_static,
                chunk_slot,
            )
            .get()?;
            out.extend_from_slice(chunk.as_slice());
            bytes_left -= 32;
            word_offset += 1;
        }

        // Nitro always reads the trailing word. `common.BytesToHash` stores a
        // partial word right-aligned, so only its final `bytes_left` bytes are
        // part of the value.
        let chunk_slot = map_address(&self.slot, &B256::from(U256::from(word_offset)));
        let chunk = StorageBackedB256::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            chunk_slot,
        )
        .get()?;
        out.extend_from_slice(&chunk.as_slice()[32 - bytes_left..]);
        Ok(out)
    }

    pub fn size(&mut self) -> Result<u64, ArbosStateError> {
        let size_slot = map_address(&self.slot, &B256::ZERO);
        StorageBackedU64::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            size_slot,
        )
        .get()
    }

    pub fn set(&mut self, value: &[u8]) -> Result<(), ArbosStateError> {
        self.clear()?;

        let size_slot = map_address(&self.slot, &B256::from(U256::from(0u64)));
        StorageBackedU256::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            size_slot,
        )
        .set(U256::from(value.len() as u64))?;

        let mut remaining = value;
        let mut word_offset = 1usize;
        while remaining.len() >= 32 {
            let chunk_slot = map_address(&self.slot, &B256::from(U256::from(word_offset)));
            StorageBackedB256::new(
                self.context,
                self.gas.as_deref_mut(),
                self.is_static,
                chunk_slot,
            )
            .set(B256::from_slice(&remaining[..32]))?;
            remaining = &remaining[32..];
            word_offset += 1;
        }

        // Go's common.BytesToHash right-aligns short input. Nitro also writes
        // this word when the value is empty or an exact multiple of 32.
        let mut trailing = [0u8; 32];
        trailing[32 - remaining.len()..].copy_from_slice(remaining);
        let chunk_slot = map_address(&self.slot, &B256::from(U256::from(word_offset)));
        StorageBackedB256::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            chunk_slot,
        )
        .set(B256::from(trailing))
    }

    pub fn clear(&mut self) -> Result<(), ArbosStateError> {
        let size_slot = map_address(&self.slot, &B256::ZERO);
        let mut bytes_left = StorageBackedU256::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            size_slot,
        )
        .get()?
        .to::<usize>();

        let mut word_offset = 1usize;
        while bytes_left > 0 {
            let chunk_slot = map_address(&self.slot, &B256::from(U256::from(word_offset)));
            StorageBackedB256::new(
                self.context,
                self.gas.as_deref_mut(),
                self.is_static,
                chunk_slot,
            )
            .set(B256::ZERO)?;
            word_offset += 1;
            bytes_left = bytes_left.saturating_sub(32);
        }

        StorageBackedU256::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            size_slot,
        )
        .set(U256::ZERO)
    }
}

pub struct StorageBackedQueue<'a, CTX>
where
    CTX: ArbitrumContextTr,
{
    pub context: &'a mut CTX,
    pub gas: Option<&'a mut Gas>,
    pub is_static: bool,
    pub slot: B256,
}

impl<'a, CTX> StorageBackedQueue<'a, CTX>
where
    CTX: ArbitrumContextTr,
{
    pub fn new(
        context: &'a mut CTX,
        gas: Option<&'a mut Gas>,
        is_static: bool,
        slot: B256,
    ) -> Self {
        Self {
            context,
            gas,
            is_static,
            slot,
        }
    }

    fn put_slot(&self) -> B256 {
        map_address(&self.slot, &B256::from(U256::from(0u64)))
    }

    fn get_slot(&self) -> B256 {
        map_address(&self.slot, &B256::from(U256::from(1u64)))
    }

    /// Nitro reserves offsets 0 and 1 for the put/get cursors, so the first
    /// queue element is stored at offset 2.
    pub fn initialize(&mut self) -> Result<(), ArbosStateError> {
        let head_slot = self.put_slot();
        StorageBackedU64::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            head_slot,
        )
        .set(2)?;
        let tail_slot = self.get_slot();
        StorageBackedU64::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            tail_slot,
        )
        .set(2)
    }

    pub fn size(&mut self) -> Result<u64, ArbosStateError> {
        let head_slot = self.get_slot();
        let tail_slot = self.put_slot();

        let head = StorageBackedU64::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            head_slot,
        )
        .get()?;
        let tail = StorageBackedU64::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            tail_slot,
        )
        .get()?;
        Ok(tail.saturating_sub(head))
    }

    pub fn peek(&mut self) -> Result<Option<U256>, ArbosStateError> {
        let head_slot = self.get_slot();
        let tail_slot = self.put_slot();

        let head = StorageBackedU64::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            head_slot,
        )
        .get()?;
        let tail = StorageBackedU64::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            tail_slot,
        )
        .get()?;

        if head >= tail {
            return Ok(None);
        }
        let elem_slot = map_address(&self.slot, &B256::from(U256::from(head)));

        let v = StorageBackedU256::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            elem_slot,
        )
        .get()?;
        Ok(Some(v))
    }

    pub fn pop(&mut self) -> Result<Option<U256>, ArbosStateError> {
        let head_slot = self.get_slot();
        let tail_slot = self.put_slot();

        let head = StorageBackedU64::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            head_slot,
        )
        .get()?;
        let tail = StorageBackedU64::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            tail_slot,
        )
        .get()?;

        if head >= tail {
            return Ok(None);
        }
        let elem_slot = map_address(&self.slot, &B256::from(U256::from(head)));
        let v = StorageBackedU256::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            elem_slot,
        )
        .get()?;

        StorageBackedU256::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            elem_slot,
        )
        .set(U256::ZERO)?;

        // increment head
        let new_head = head.saturating_add(1);
        StorageBackedU64::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            head_slot,
        )
        .set(new_head)?;
        Ok(Some(v))
    }

    pub fn push(&mut self, value: U256) -> Result<(), ArbosStateError> {
        let tail_slot = self.put_slot();

        let tail = StorageBackedU64::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            tail_slot,
        )
        .get()?;
        let new_tail = tail.saturating_add(1);
        StorageBackedU64::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            tail_slot,
        )
        .set(new_tail)?;

        let elem_slot = map_address(&self.slot, &B256::from(U256::from(tail)));
        StorageBackedU256::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            elem_slot,
        )
        .set(value)?;

        Ok(())
    }
}
