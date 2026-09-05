use revm::{
    interpreter::Gas,
    primitives::{B256, U256, keccak256},
};

use crate::{
    ArbitrumContextTr,
    state::types::{
        ArbosStateError, StorageBackedB256, StorageBackedTr, StorageBackedU64, map_address,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MerkleTreeNodeEvent {
    pub level: u64,
    pub num_leaves: u64,
    pub hash: B256,
}

pub struct MerkleAccumulator<'a, CTX: ArbitrumContextTr> {
    context: &'a mut CTX,
    gas: Option<&'a mut Gas>,
    is_static: bool,
    slot: B256,
}

impl<'a, CTX: ArbitrumContextTr> MerkleAccumulator<'a, CTX> {
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

    fn size_word(&mut self) -> StorageBackedU64<'_, CTX> {
        StorageBackedU64::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            map_address(&self.slot, &B256::ZERO),
        )
    }

    fn partial_word(&mut self, level: u64) -> StorageBackedB256<'_, CTX> {
        StorageBackedB256::new(
            self.context,
            self.gas.as_deref_mut(),
            self.is_static,
            map_address(&self.slot, &B256::from(U256::from(2 + level))),
        )
    }

    fn hash_pair(&mut self, left: B256, right: B256) -> Result<B256, ArbosStateError> {
        // Storage.Keccak charges Keccak256Gas + two Keccak256WordGas units for
        // the 64-byte accumulator node.
        if let Some(gas) = self.gas.as_deref_mut()
            && !gas.record_regular_cost(42)
        {
            return Err(ArbosStateError::OutOfGas);
        }
        Ok(keccak256([left.as_slice(), right.as_slice()].concat()))
    }

    pub fn size(&mut self) -> Result<u64, ArbosStateError> {
        self.size_word().get()
    }

    pub fn append(&mut self, item_hash: B256) -> Result<Vec<MerkleTreeNodeEvent>, ArbosStateError> {
        let old_size = self.size()?;
        let size = old_size.saturating_add(1);
        self.size_word().set(size)?;

        let mut events = Vec::new();
        let mut level = 0;
        let mut so_far = keccak256(item_hash);
        loop {
            if level == num_partials(old_size) {
                self.partial_word(level).set(so_far)?;
                break;
            }
            let partial = self.partial_word(level).get()?;
            if partial.is_zero() {
                self.partial_word(level).set(so_far)?;
                break;
            }
            so_far = self.hash_pair(partial, so_far)?;
            self.partial_word(level).set(B256::ZERO)?;
            level += 1;
            events.push(MerkleTreeNodeEvent {
                level,
                num_leaves: old_size,
                hash: so_far,
            });
        }
        Ok(events)
    }

    pub fn partials(&mut self) -> Result<Vec<B256>, ArbosStateError> {
        let count = num_partials(self.size()?);
        (0..count)
            .map(|level| self.partial_word(level).get())
            .collect()
    }

    pub fn root(&mut self) -> Result<B256, ArbosStateError> {
        let size = self.size()?;
        if size == 0 {
            return Ok(B256::ZERO);
        }
        let mut hash_so_far: Option<B256> = None;
        let mut capacity_in_hash = 0u64;
        let mut capacity = 1u64;
        for level in 0..num_partials(size) {
            let partial = self.partial_word(level).get()?;
            if !partial.is_zero() {
                hash_so_far = Some(if let Some(mut hash) = hash_so_far {
                    while capacity_in_hash < capacity {
                        hash = self.hash_pair(hash, B256::ZERO)?;
                        capacity_in_hash *= 2;
                    }
                    capacity_in_hash = 2 * capacity;
                    self.hash_pair(partial, hash)?
                } else {
                    capacity_in_hash = capacity;
                    partial
                });
            }
            capacity *= 2;
        }
        Ok(hash_so_far.unwrap_or_default())
    }

    pub fn state_for_export(&mut self) -> Result<(u64, B256, Vec<B256>), ArbosStateError> {
        Ok((self.size()?, self.root()?, self.partials()?))
    }
}

fn num_partials(size: u64) -> u64 {
    u64::BITS as u64 - size.leading_zeros() as u64
}
