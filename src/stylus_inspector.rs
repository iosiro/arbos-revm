//! Inspector views of Stylus host operations.
//!
//! Storage host requests use the corresponding EVM stack layout so existing
//! inspectors can record accesses, override reads, and schedule callbacks. The
//! host API still performs the operation and determines its gas charge; these
//! views do not execute EVM storage instructions or charge their gas a second time.

use std::mem;

use revm::{
    Inspector,
    bytecode::{Bytecode, opcode},
    context::{Cfg, ContextSetters, FrameStack},
    handler::{
        EvmTr, ItemOrResult, PrecompileProvider, evm::ContextDbError,
        instructions::InstructionProvider,
    },
    inspector::{InspectorEvmTr, JournalExt},
    interpreter::{
        InstructionResult, Interpreter, InterpreterAction, InterpreterResult,
        interpreter::{EthInterpreter, ExtBytecode},
        interpreter_types::{Jumps, LoopControl},
    },
    primitives::{Bytes, U256},
};

use crate::{ArbitrumEvm, context::ArbitrumContextMutTr};

impl<CTX, INSP, P, I> ArbitrumEvm<CTX, INSP, P, I>
where
    CTX: ArbitrumContextMutTr<Journal: JournalExt> + ContextSetters,
    I: InstructionProvider<Context = CTX, InterpreterTypes = EthInterpreter>,
    P: PrecompileProvider<CTX, Output = InterpreterResult>,
    INSP: Inspector<CTX>,
{
    /// Inspect one host operation in the current frame, without creating another
    /// call boundary. Operands are in bottom-to-top EVM stack order. A returned
    /// word is exposed to step_end and may be replaced by the inspector.
    pub(crate) fn inspect_host_operation<R>(
        &mut self,
        opcode: u8,
        operands: &[U256],
        gas_limit: u64,
        operation: impl FnOnce(&mut Self) -> (R, Option<U256>, Option<InstructionResult>),
    ) -> Option<(R, Option<U256>)> {
        let spec = self.ctx().cfg().spec().into();
        let frame = self.frame_stack().get();
        let view = Interpreter::new(
            // Nested callback calldata uses the context's shared buffer. Keep
            // the parent's memory arena and checkpoint, not a separate buffer.
            frame.interpreter.memory.clone(),
            ExtBytecode::new(Bytecode::new_legacy(Bytes::from(vec![
                opcode,
                opcode::JUMPDEST,
            ]))),
            frame.interpreter.input.clone(),
            frame.interpreter.runtime_flag.is_static,
            spec,
            gas_limit,
        );
        let original = mem::replace(&mut frame.interpreter, view);
        for operand in operands {
            assert!(frame.interpreter.stack.push(*operand));
        }

        let (ctx, inspector, frame) = self.ctx_inspector_frame();
        inspector.step(&mut frame.interpreter, ctx);
        let result = if frame.interpreter.bytecode.action.is_none() {
            let (result, word, failure) = operation(self);
            let (ctx, inspector, frame) = self.ctx_inspector_frame();
            frame.interpreter.stack.data_mut().clear();
            if let Some(word) = word {
                assert!(frame.interpreter.stack.push(word));
            }
            frame.interpreter.bytecode.relative_jump(1);
            if let Some(failure) = failure {
                frame.interpreter.halt(failure);
            }
            inspector.step_end(&mut frame.interpreter, ctx);
            Some(result)
        } else {
            None
        };

        // Inspectors may inject a call (for example a storage observation hook).
        // Run it with ordinary call/call_end semantics, then resume the parent
        // view so the inspector can restore its saved stack, gas and state.
        while matches!(
            self.frame_stack().get().interpreter.bytecode.action,
            Some(InterpreterAction::NewFrame(_))
        ) {
            if let Err(error) = self.run_host_inspector_callback() {
                *self.ctx().error() = Err(error);
                self.frame_stack()
                    .get()
                    .interpreter
                    .halt(InstructionResult::FatalExternalError);
                break;
            }
            let (ctx, inspector, frame) = self.ctx_inspector_frame();
            inspector.step(&mut frame.interpreter, ctx);
            if frame.interpreter.bytecode.action.is_none() {
                // JUMPDEST is a continuation marker, not the end of the Wasm frame.
                inspector.step_end(&mut frame.interpreter, ctx);
            }
        }

        let frame = self.frame_stack().get();
        let word = frame.interpreter.stack.peek(0).ok();
        let action = frame.interpreter.bytecode.action.take();
        frame.interpreter = original;
        if let Some(action) = action {
            frame.interpreter.bytecode.set_action(action);
            None
        } else {
            result.map(|result| (result, word))
        }
    }

    fn run_host_inspector_callback(&mut self) -> Result<(), ContextDbError<CTX>> {
        let frame = self.0.frame_stack.get();
        let action = frame.interpreter.take_next_action();
        let ItemOrResult::Item(init) =
            frame.process_next_action::<_, ContextDbError<CTX>>(&mut self.0.ctx, action)?
        else {
            unreachable!("inspector callback must start a frame")
        };
        let parent = mem::replace(&mut self.0.frame_stack, FrameStack::new());
        let result = self.inspect_run_exec_loop(init);
        self.0.frame_stack = parent;
        match result {
            Ok(result) => self
                .0
                .frame_stack
                .get()
                .return_result(&mut self.0.ctx, result),
            Err(error) => {
                self.0
                    .frame_stack
                    .get()
                    .interpreter
                    .memory
                    .free_child_context();
                Err(error)
            }
        }
    }
}
