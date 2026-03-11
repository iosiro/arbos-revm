use crate::{
    ArbitrumContextTr,
    state::types::{
        ArbosStateError, StorageBackedAddress, StorageBackedB256, StorageBackedTr,
        StorageBackedU64, StorageBackedU256, map_address, substorage,
    },
};
use alloy_rlp::{Decodable, Encodable};
use revm::{
    interpreter::Gas,
    primitives::{Address, B256, Bytes, U256},
};

pub struct AddressTable<'a, CTX>
where
    CTX: ArbitrumContextTr,
{
    context: &'a mut CTX,
    gas: Option<&'a mut Gas>,
    is_static: bool,
    slot: B256,
}

impl<'a, CTX> AddressTable<'a, CTX>
where
    CTX: ArbitrumContextTr,
{
    /// Open an AddressTable rooted at `slot`
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

    fn backing_slot(&self) -> B256 {
        // the array (size at index 0, elements at 1..)
        self.slot
    }

    fn by_address_substorage(&self) -> B256 {
        // substorage index 0 used for by-address mapping
        substorage(&self.slot, &[])
    }

    fn size_slot(&self) -> B256 {
        // size stored under map(backing_slot, 0)
        map_address(&self.backing_slot(), &B256::from(U256::from(0u64)))
    }

    /// internal: read the stored 1-based index for `address` (0 means not present)
    fn get_stored_index(&mut self, address: Address) -> Result<U256, ArbosStateError> {
        let by_addr = self.by_address_substorage();
        let key = B256::left_padding_from(address.as_slice());
        let slot = map_address(&by_addr, &key);

        StorageBackedU256::new(
            &mut self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            slot,
        )
        .get()
    }

    /// Register `address` if not present and return zero-based index.
    /// If already present, returns existing zero-based index.
    pub fn register(&mut self, address: Address) -> Result<u64, ArbosStateError> {
        // check by-address mapping
        let existing = self.get_stored_index(address)?;
        if !existing.is_zero() {
            // stored index is 1-based in storage
            return Ok(existing.saturating_to::<u64>() - 1);
        }

        // not present: increment size and append into backing_storage at new index (1-based)
        let size_slot = self.size_slot();

        let size_u256 = StorageBackedU256::new(
            &mut self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            size_slot,
        )
        .get()?;

        let size = size_u256.saturating_to::<u64>();
        let new_num = size + 1;

        // store address into backing storage at element index new_num (map(backing, new_num))
        let elem_slot = map_address(&self.backing_slot(), &B256::from(U256::from(new_num)));
        StorageBackedB256::new(
            &mut self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            elem_slot,
        )
        .set(B256::left_padding_from(address.as_slice()))?;

        // update size
        StorageBackedU256::new(
            &mut self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            size_slot,
        )
        .set(U256::from(new_num))?;

        // record by-address -> new_num (1-based)
        let by_addr = self.by_address_substorage();
        let by_key = B256::left_padding_from(address.as_slice());

        StorageBackedB256::new(
            &mut self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            map_address(&by_addr, &by_key),
        )
        .set(B256::from(U256::from(new_num)))?;

        // return zero-based index
        Ok(new_num - 1)
    }

    /// Look up an address; returns (zero_based_index, exists)
    pub fn lookup(&mut self, address: Address) -> Result<Option<u64>, ArbosStateError> {
        let existing = self.get_stored_index(address)?;
        if existing.is_zero() {
            Ok(None)
        } else {
            Ok(Some(existing.saturating_to::<u64>() - 1))
        }
    }

    /// true if address exists
    pub fn address_exists(&mut self, address: Address) -> Result<bool, ArbosStateError> {
        self.lookup(address).map(|opt| opt.is_some())
    }

    /// number of items (size)
    pub fn size(&mut self) -> Result<u64, ArbosStateError> {
        let size_slot = self.size_slot();
        StorageBackedU64::new(
            &mut self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            size_slot,
        )
        .get()
    }

    /// Lookup by zero-based index.
    pub fn lookup_index(&mut self, index: u64) -> Result<Option<Address>, ArbosStateError> {
        let items = self.size()?;
        if index >= items {
            return Ok(None);
        }
        // stored at 1-based index
        let elem_slot = map_address(&self.backing_slot(), &B256::from(U256::from(index + 1)));

        StorageBackedAddress::new(
            &mut self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            elem_slot,
        )
        .get()
        .map(Some)
    }

    /// Compress an address to RLP format matching nitro's encoding:
    /// - Known address: RLP-encoded uint64 index
    /// - Unknown address: RLP-encoded 20-byte address
    pub fn compress(&mut self, address: Address) -> Result<Bytes, ArbosStateError> {
        if let Some(index) = self.lookup(address)? {
            let mut out = Vec::new();
            index.encode(&mut out);
            Ok(Bytes::from(out))
        } else {
            let mut out = Vec::new();
            address.as_slice().encode(&mut out);
            Ok(Bytes::from(out))
        }
    }

    /// Decompress RLP data matching nitro's decoding:
    /// - If decoded bytes have length 20, it's an address
    /// - Otherwise, re-decode as uint64 index and look up
    pub fn decompress(&mut self, data: &[u8]) -> Result<(Address, u64), ArbosStateError> {
        let mut remaining = data;
        let decoded_bytes = <Vec<u8>>::decode(&mut remaining)
            .map_err(|e| ArbosStateError::DecompressError(format!("RLP decode error: {e:?}")))?;
        let consumed = (data.len() - remaining.len()) as u64;

        if decoded_bytes.len() == 20 {
            let addr = Address::from_slice(&decoded_bytes);
            Ok((addr, consumed))
        } else {
            // re-decode from the start as a uint64 index
            let mut buf = data;
            let index = u64::decode(&mut buf).map_err(|e| {
                ArbosStateError::DecompressError(format!("RLP index decode error: {e:?}"))
            })?;
            let consumed = (data.len() - buf.len()) as u64;
            let addr = self.lookup_index(index)?.ok_or_else(|| {
                ArbosStateError::DecompressError("invalid index in compressed address".to_string())
            })?;
            Ok((addr, consumed))
        }
    }
}
