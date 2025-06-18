use crate::{
    frame::ArbitrumFrame, handler::ArbitrumHandler, precompiles::ArbitrumPrecompiles,
    ArbitrumContextTr, ArbitrumHaltReason,
};
use revm::{
    context::{
        result::{EVMError, ExecutionResult, InvalidTransaction, ResultAndState},
        ContextSetters, ContextTr, Evm,
    },
    handler::{
        instructions::{EthInstructions, InstructionProvider},
        EvmTr, Handler, PrecompileProvider,
    },
    interpreter::{
        interpreter::EthInterpreter, Interpreter, InterpreterAction, InterpreterResult,
        InterpreterTypes,
    },
    Database, DatabaseCommit, ExecuteCommitEvm, ExecuteEvm, Inspector,
};

pub struct ArbitrumVM<CTX, INSP, I = EthInstructions<EthInterpreter, CTX>, P = ArbitrumPrecompiles>(
    pub Evm<CTX, INSP, I, P>,
);

impl<CTX, I, INSP, P> ArbitrumVM<CTX, INSP, I, P> {
    /// Create a new EVM instance with a given context, inspector, instruction set, and precompile provider.
    pub fn new_with_inspector(ctx: CTX, inspector: INSP, instruction: I, precompiles: P) -> Self {
        ArbitrumVM {
            0: Evm {
                ctx,
                inspector,
                instruction,
                precompiles,
            },
        }
    }
}

impl<CTX, INSP, I, P> ArbitrumVM<CTX, INSP, I, P> {
    /// Consumed self and returns new Evm type with given Inspector.
    pub fn with_inspector<OINSP>(self, inspector: OINSP) -> ArbitrumVM<CTX, OINSP, I, P> {
        ArbitrumVM {
            0: Evm {
                ctx: self.0.ctx,
                inspector,

                instruction: self.0.instruction,
                precompiles: self.0.precompiles,
            },
        }
    }

    /// Consumes self and returns new Evm type with given Precompiles.
    pub fn with_precompiles<OP>(self, precompiles: OP) -> ArbitrumVM<CTX, INSP, I, OP> {
        ArbitrumVM {
            0: Evm {
                ctx: self.0.ctx,
                inspector: self.0.inspector,
                instruction: self.0.instruction,
                precompiles,
            },
        }
    }

    /// Consumes self and returns inner Inspector.
    pub fn into_inspector(self) -> INSP {
        self.0.inspector
    }
}

impl<CTX, INSP, I, P> EvmTr for ArbitrumVM<CTX, INSP, I, P>
where
    CTX: ArbitrumContextTr,
    I: InstructionProvider<
        Context = CTX,
        InterpreterTypes: InterpreterTypes<Output = InterpreterAction>,
    >,
    P: PrecompileProvider<CTX>,
{
    type Context = CTX;
    type Instructions = I;
    type Precompiles = P;

    fn run_interpreter(
        &mut self,
        interpreter: &mut Interpreter<
            <Self::Instructions as InstructionProvider>::InterpreterTypes,
        >,
    ) -> <<Self::Instructions as InstructionProvider>::InterpreterTypes as InterpreterTypes>::Output
    {
        let context = &mut self.0.ctx;

        let instructions = &mut self.0.instruction;
        interpreter.run_plain(instructions.instruction_table(), context)
    }

    fn ctx(&mut self) -> &mut Self::Context {
        &mut self.0.ctx
    }

    fn ctx_ref(&self) -> &Self::Context {
        &self.0.ctx
    }

    fn ctx_instructions(&mut self) -> (&mut Self::Context, &mut Self::Instructions) {
        (&mut self.0.ctx, &mut self.0.instruction)
    }

    fn ctx_precompiles(&mut self) -> (&mut Self::Context, &mut Self::Precompiles) {
        (&mut self.0.ctx, &mut self.0.precompiles)
    }
}

/// Type alias for the error type of the ArbitrumVM.
type ArbitrumError<CTX> = EVMError<<<CTX as ContextTr>::Db as Database>::Error, InvalidTransaction>;

impl<CTX, INSP, PRECOMPILE> ExecuteEvm
    for ArbitrumVM<CTX, INSP, EthInstructions<EthInterpreter, CTX>, PRECOMPILE>
where
    CTX: ArbitrumContextTr + ContextSetters,
    PRECOMPILE: PrecompileProvider<CTX, Output = InterpreterResult>,
    INSP: Inspector<CTX>,
{
    type Output = Result<ResultAndState<ArbitrumHaltReason>, ArbitrumError<CTX>>;

    type Tx = <CTX as ContextTr>::Tx;

    type Block = <CTX as ContextTr>::Block;

    fn set_tx(&mut self, tx: Self::Tx) {
        self.0.ctx.set_tx(tx);
    }

    fn set_block(&mut self, block: Self::Block) {
        self.0.ctx.set_block(block);
    }

    fn replay(&mut self) -> Self::Output {
        let mut h = ArbitrumHandler::<_, _, ArbitrumFrame<_, _, _>>::new();
        h.run(self)
    }
}

impl<CTX, INSP, PRECOMPILE> ExecuteCommitEvm
    for ArbitrumVM<CTX, INSP, EthInstructions<EthInterpreter, CTX>, PRECOMPILE>
where
    CTX: ArbitrumContextTr<Db: DatabaseCommit> + ContextSetters,
    PRECOMPILE: PrecompileProvider<CTX, Output = InterpreterResult>,
    INSP: Inspector<CTX>,
{
    type CommitOutput = Result<ExecutionResult<ArbitrumHaltReason>, ArbitrumError<CTX>>;

    fn replay_commit(&mut self) -> Self::CommitOutput {
        self.replay().map(|r| {
            self.ctx().db().commit(r.state);
            r.result
        })
    }
}
