use std::ops::{Deref, DerefMut};

use revm::{
    context::{
        Block, BlockEnv, Cfg, CfgEnv, ContextTr, JournalOutput, JournalTr, LocalContext, LocalContextTr, Transaction
    },
    context_interface::block::BlobExcessGasAndPrice,
    database::EmptyDB,
    inspector::JournalExt,
    primitives::{hardfork::SpecId, Address, B256, U256},
    Context, Database, Journal,
};

use crate::{transaction::ArbitrumTransaction, ArbitrumSpecId};

pub trait ArbitrumCfgTr: Cfg<Spec = ArbitrumSpecId> {
    fn arbos_version(&self) -> u16;
    fn stylus_version(&self) -> u16;
    fn max_depth(&self) -> u32;
    fn ink_price(&self) -> u32;
    fn debug_mode(&self) -> bool;
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArbitrumCfgEnv<SPEC = ArbitrumSpecId> {
    pub inner: CfgEnv<SPEC>,
    pub arbos_version: u16,
    pub stylus_version: u16,
    pub max_depth: u32,
    pub ink_price: u32,
    pub debug_mode: bool,
}

impl<SPEC: Default> ArbitrumCfgEnv<SPEC> {
    pub fn new_with_spec(spec: SPEC) -> Self {
        Self {
            inner: CfgEnv::new_with_spec(spec),
            ..Default::default()
        }
    }

    pub fn new(inner: CfgEnv<SPEC>) -> Self {
        Self {
            inner,
            arbos_version: 32,
            stylus_version: 1,
            max_depth: 4 * 65536,
            ink_price: 10000,
            debug_mode: false,
        }
    }
}

impl<SPEC: Default> Default for ArbitrumCfgEnv<SPEC> {
    fn default() -> Self {
        Self {
            inner: CfgEnv::default(),
            arbos_version: 32,
            stylus_version: 1,
            max_depth: 4 * 65536,
            ink_price: 10000,
            debug_mode: false,
        }
    }
}

impl<SPEC: Default> Deref for ArbitrumCfgEnv<SPEC> {
    type Target = CfgEnv<SPEC>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<CfgEnv<ArbitrumSpecId>> for ArbitrumCfgEnv<ArbitrumSpecId> {
    fn from(inner: CfgEnv<ArbitrumSpecId>) -> Self {
        Self {
            inner,
            arbos_version: 32,
            stylus_version: 1,
            max_depth: 4 * 65536,
            ink_price: 10000,
            debug_mode: false,
        }
    }
}

impl<SPEC: Default> DerefMut for ArbitrumCfgEnv<SPEC> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl ArbitrumCfgTr for ArbitrumCfgEnv<ArbitrumSpecId> {
    fn arbos_version(&self) -> u16 {
        self.arbos_version
    }

    fn stylus_version(&self) -> u16 {
        self.stylus_version
    }

    fn max_depth(&self) -> u32 {
        self.max_depth
    }

    fn ink_price(&self) -> u32 {
        self.ink_price
    }

    fn debug_mode(&self) -> bool {
        self.debug_mode
    }
}

impl<SPEC> Cfg for ArbitrumCfgEnv<SPEC>
where
    SPEC: Into<SpecId> + Copy,
{
    type Spec = SPEC;

    fn chain_id(&self) -> u64 {
        self.inner.chain_id()
    }

    fn spec(&self) -> Self::Spec {
        self.inner.spec()
    }

    fn blob_max_count(&self) -> Option<u64> {
        self.inner.blob_max_count()
    }

    fn max_code_size(&self) -> usize {
        self.inner.max_code_size()
    }

    fn is_eip3607_disabled(&self) -> bool {
        self.inner.is_eip3607_disabled()
    }

    fn is_balance_check_disabled(&self) -> bool {
        self.inner.is_balance_check_disabled()
    }

    fn is_block_gas_limit_disabled(&self) -> bool {
        self.inner.is_block_gas_limit_disabled()
    }

    fn is_nonce_check_disabled(&self) -> bool {
        self.inner.is_nonce_check_disabled()
    }

    fn is_base_fee_check_disabled(&self) -> bool {
        self.inner.is_base_fee_check_disabled()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ArbitrumBlockEnv {
    pub inner: BlockEnv,
}

impl Deref for ArbitrumBlockEnv {
    type Target = BlockEnv;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for ArbitrumBlockEnv {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl From<BlockEnv> for ArbitrumBlockEnv {
    fn from(inner: BlockEnv) -> Self {
        Self { inner }
    }
}

impl Block for ArbitrumBlockEnv {
    #[inline]
    fn number(&self) -> u64 {
        self.inner.number
    }

    #[inline]
    fn beneficiary(&self) -> Address {
        self.inner.beneficiary
    }

    #[inline]
    fn timestamp(&self) -> u64 {
        self.inner.timestamp
    }

    #[inline]
    fn gas_limit(&self) -> u64 {
        self.inner.gas_limit
    }

    #[inline]
    fn basefee(&self) -> u64 {
        self.inner.basefee
    }

    #[inline]
    fn difficulty(&self) -> U256 {
        self.inner.difficulty
    }

    #[inline]
    fn prevrandao(&self) -> Option<B256> {
        self.inner.prevrandao
    }

    #[inline]
    fn blob_excess_gas_and_price(&self) -> Option<BlobExcessGasAndPrice> {
        self.inner.blob_excess_gas_and_price
    }
}

impl Default for ArbitrumBlockEnv {
    fn default() -> Self {
        Self {
            inner: BlockEnv::default(),
        }
    }
}

// Type alias for Arbitrum context
pub trait ArbitrumContextTr:
    ContextTr<
    Journal: JournalTr<FinalOutput = JournalOutput> + JournalExt,
    Tx: Transaction,
    Cfg: ArbitrumCfgTr<Spec = ArbitrumSpecId>,
    Chain = (),
    Local: ArbitrumLocalContextTr,
>
{
}

impl<T> ArbitrumContextTr for T where
    T: ContextTr<
        Journal: JournalTr<FinalOutput = JournalOutput> + JournalExt,
        Tx: Transaction,
        Cfg: ArbitrumCfgTr<Spec = ArbitrumSpecId>,
        Chain = (),
        Local: ArbitrumLocalContextTr,
    >
{
}

pub type ArbitrumContext<DB> = Context<
    ArbitrumBlockEnv,
    ArbitrumTransaction,
    ArbitrumCfgEnv<ArbitrumSpecId>,
    DB,
    Journal<DB>,
    (),
    ArbitrumLocalContext,
>;

pub trait DefaultArbitrum {
    fn arbitrum() -> ArbitrumContext<EmptyDB>;
}

impl DefaultArbitrum for ArbitrumContext<EmptyDB> {
    fn arbitrum() -> ArbitrumContext<EmptyDB> {
        ArbitrumContextExt::new(EmptyDB::default(), ArbitrumSpecId::default())
    }
}

pub trait ArbitrumContextExt<DB: Database> {
    fn new(db: DB, spec: ArbitrumSpecId) -> Self;
}

impl<DB> ArbitrumContextExt<DB> for ArbitrumContext<DB>
where
    DB: Database + Default,
{
    fn new(db: DB, spec: ArbitrumSpecId) -> Self {
        let mut journaled_state = Journal::new(db);
        journaled_state.set_spec_id(spec.into_eth_spec());
        Self {
            tx: ArbitrumTransaction::default(),
            block: ArbitrumBlockEnv::default(),
            cfg: ArbitrumCfgEnv::new_with_spec(spec),
            local: ArbitrumLocalContext::default(),
            journaled_state,
            chain: Default::default(),
            error: Ok(()),
        }
    }
}

pub struct ArbitrumLocalContext {
    pub inner: LocalContext,
}

impl Deref for ArbitrumLocalContext {
    type Target = LocalContext;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for ArbitrumLocalContext {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl Default for ArbitrumLocalContext {
    fn default() -> Self {
        Self {
            inner: LocalContext::default(),
        }
    }
}
impl ArbitrumLocalContext {
    pub fn new() -> Self {
        Self::default()
    }
}

pub trait ArbitrumLocalContextTr: LocalContextTr {}

impl ArbitrumLocalContextTr for ArbitrumLocalContext {}

impl LocalContextTr for ArbitrumLocalContext {

    fn insert_initcodes(&mut self, initcodes: &[revm::primitives::Bytes]) {
        self.inner.insert_initcodes(initcodes);
    }
    
    fn get_validated_initcode(&mut self, hash: B256) -> Option<revm::primitives::Bytes> {
        self.inner.get_validated_initcode(hash)
    }
    
    fn shared_memory_buffer(&self) -> &std::rc::Rc<std::cell::RefCell<Vec<u8>>> {
        self.inner.shared_memory_buffer()
    }
    
    fn clear(&mut self) {
        self.inner.clear();
    }
}