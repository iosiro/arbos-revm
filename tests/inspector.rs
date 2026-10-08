//! Inspector lifecycle and host-operation regressions for native Stylus execution.

use revm::{
    InspectEvm, Inspector,
    bytecode::opcode,
    context::{ContextTr, JournalTr},
    interpreter::{
        InstructionResult, Interpreter, InterpreterAction,
        interpreter_types::{Jumps, LoopControl},
    },
    primitives::{Address, Bytes, Log, U256},
};

mod test_utils;
use test_utils::{
    TestContext, create_call_tx, create_evm, deploy_wat_program, execute_tx, fund_account,
    setup_context_with_arbos_state,
};

#[derive(Default)]
struct HostInspector {
    stop_at_init: bool,
    stop_at_store: bool,
    stop_after_load: bool,
    override_load: Option<U256>,
    steps: Vec<(u8, Vec<U256>)>,
    logs: usize,
}

impl Inspector<TestContext> for HostInspector {
    fn initialize_interp(&mut self, interp: &mut Interpreter, _: &mut TestContext) {
        if self.stop_at_init {
            interp.bytecode.set_action(InterpreterAction::new_return(
                InstructionResult::Revert,
                Bytes::from_static(b"stop at init"),
                interp.gas,
            ));
        }
    }

    fn step(&mut self, interp: &mut Interpreter, _: &mut TestContext) {
        let op = interp.bytecode.opcode();
        self.steps.push((op, interp.stack.data().to_vec()));
        if self.stop_at_store && op == opcode::SSTORE {
            interp.bytecode.set_action(InterpreterAction::new_return(
                InstructionResult::Revert,
                Bytes::from_static(b"stop at store"),
                interp.gas,
            ));
        }
    }

    fn log(&mut self, _: &mut TestContext, _: Log) {
        self.logs += 1;
    }

    fn step_end(&mut self, interp: &mut Interpreter, _: &mut TestContext) {
        if self
            .steps
            .last()
            .is_some_and(|(op, _)| *op == opcode::SLOAD)
        {
            if let Some(value) = self.override_load {
                *interp.stack.data_mut().last_mut().unwrap() = value;
            }
            if self.stop_after_load {
                interp.bytecode.set_action(InterpreterAction::new_return(
                    InstructionResult::Revert,
                    Bytes::from_static(b"stop after load"),
                    interp.gas,
                ));
            }
        }
    }
}

#[test]
fn initialize_interp_stops_before_wasm_execution() {
    let mut ctx = setup_context_with_arbos_state();
    let program = deploy_wat_program(
        &mut ctx,
        br#"(module
        (import "vm_hooks" "emit_log" (func $log (param i32 i32 i32)))
        (memory (export "memory") 1 1)
        (func (export "user_entrypoint") (param i32) (result i32)
            (call $log (i32.const 0) (i32.const 0) (i32.const 0))
            (loop $forever (br $forever)) i32.const 0))"#,
    );
    fund_account(&mut ctx, Address::repeat_byte(1), U256::from(1_000_000_000));
    let mut evm = create_evm(ctx).with_inspector(HostInspector {
        stop_at_init: true,
        ..Default::default()
    });
    let result = evm
        .inspect_one_tx(create_call_tx(program, vec![], 1_000_000).into())
        .unwrap();
    assert_eq!(result.output().unwrap().as_ref(), b"stop at init");
    assert_eq!(evm.0.inspector.logs, 0);
    assert!(evm.0.inspector.steps.is_empty());
}

#[test]
fn storage_inspection_preserves_results_state_and_gas() {
    // Exercise both success and out-of-gas paths, with cold and warm reads in
    // the host cache. No-op inspection must not alter Nitro's metering.
    for limit in [30_000, 60_000, 100_000, 1_000_000] {
        let mut ctx = setup_context_with_arbos_state();
        let program = deploy_wat_program(&mut ctx, include_bytes!("../test-data/storage.wat"));
        fund_account(&mut ctx, Address::repeat_byte(1), U256::from(1_000_000_000));
        let mut args = vec![1];
        args.extend(U256::from(7).to_be_bytes::<32>());
        args.extend(U256::from(42).to_be_bytes::<32>());
        let tx = create_call_tx(program, args, limit);
        let mut plain = create_evm(ctx.clone());
        let expected = execute_tx(&mut plain, tx.clone());
        let mut inspected = create_evm(ctx).with_inspector(HostInspector::default());
        let actual = inspected.inspect_one_tx(tx.into()).unwrap();
        assert_eq!(actual, expected, "gas limit {limit}");
        assert_eq!(
            inspected
                .0
                .ctx
                .journal_mut()
                .sload(program, U256::from(7))
                .unwrap()
                .data,
            plain
                .0
                .ctx
                .journal_mut()
                .sload(program, U256::from(7))
                .unwrap()
                .data,
        );
        if actual.is_success() {
            let storage_steps: Vec<_> = inspected
                .0
                .inspector
                .steps
                .iter()
                .filter(|(op, _)| matches!(*op, opcode::SLOAD | opcode::SSTORE))
                .cloned()
                .collect();
            assert_eq!(
                storage_steps,
                vec![
                    (opcode::SLOAD, vec![U256::from(7)]),
                    (opcode::SSTORE, vec![U256::from(42), U256::from(7)]),
                ]
            );
        }
    }
}

#[test]
fn storage_inspector_stops_host_write_and_rolls_back() {
    let mut ctx = setup_context_with_arbos_state();
    let program = deploy_wat_program(&mut ctx, include_bytes!("../test-data/storage.wat"));
    fund_account(&mut ctx, Address::repeat_byte(1), U256::from(1_000_000_000));
    let mut args = vec![1];
    args.extend(U256::ZERO.to_be_bytes::<32>());
    args.extend(U256::from(42).to_be_bytes::<32>());
    let mut evm = create_evm(ctx).with_inspector(HostInspector {
        stop_at_store: true,
        ..Default::default()
    });
    let result = evm
        .inspect_one_tx(create_call_tx(program, args, 1_000_000).into())
        .unwrap();
    assert_eq!(result.output().unwrap().as_ref(), b"stop at store");
    assert_eq!(
        evm.0
            .ctx
            .journal_mut()
            .sload(program, U256::ZERO)
            .unwrap()
            .data,
        U256::ZERO
    );
}

#[test]
fn storage_read_uses_inspector_output() {
    let mut ctx = setup_context_with_arbos_state();
    let program = deploy_wat_program(&mut ctx, include_bytes!("../test-data/storage.wat"));
    fund_account(&mut ctx, Address::repeat_byte(1), U256::from(1_000_000_000));
    let mut evm = create_evm(ctx).with_inspector(HostInspector {
        override_load: Some(U256::from(777)),
        ..Default::default()
    });
    let result = evm
        .inspect_one_tx(create_call_tx(program, vec![0; 33], 1_000_000).into())
        .unwrap();
    assert_eq!(
        U256::from_be_slice(result.output().unwrap()),
        U256::from(777)
    );
    assert_eq!(
        evm.0
            .ctx
            .journal_mut()
            .sload(program, U256::ZERO)
            .unwrap()
            .data,
        U256::ZERO
    );
}

#[test]
fn storage_read_inspector_termination_stops_wasm() {
    let mut ctx = setup_context_with_arbos_state();
    let program = deploy_wat_program(
        &mut ctx,
        br#"(module
        (import "vm_hooks" "storage_load_bytes32" (func $load (param i32 i32)))
        (import "vm_hooks" "emit_log" (func $log (param i32 i32 i32)))
        (memory (export "memory") 1 1)
        (func (export "user_entrypoint") (param i32) (result i32)
            (call $load (i32.const 0) (i32.const 32))
            (call $log (i32.const 0) (i32.const 0) (i32.const 0))
            (loop $forever (br $forever)) i32.const 0))"#,
    );
    fund_account(&mut ctx, Address::repeat_byte(1), U256::from(1_000_000_000));
    let mut evm = create_evm(ctx).with_inspector(HostInspector {
        stop_after_load: true,
        ..Default::default()
    });
    let result = evm
        .inspect_one_tx(create_call_tx(program, vec![], 1_000_000).into())
        .unwrap();
    assert_eq!(result.output().unwrap().as_ref(), b"stop after load");
    assert_eq!(evm.0.inspector.logs, 0);
}
