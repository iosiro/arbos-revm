//! Inspector lifecycle and host-operation regressions for native Stylus execution.

use revm::{
    InspectEvm, Inspector,
    bytecode::{Bytecode, opcode},
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct FrameLocation {
    depth: usize,
    target: Address,
    bytecode: Option<Address>,
    caller: Address,
    is_static: bool,
}

impl FrameLocation {
    fn from_interpreter(interp: &Interpreter) -> Self {
        Self {
            depth: interp.input.depth,
            target: interp.input.target_address,
            bytecode: interp.input.bytecode_address,
            caller: interp.input.caller_address,
            is_static: interp.runtime_flag.is_static,
        }
    }
}

#[derive(Default)]
struct DepthInspector {
    frames: Vec<FrameLocation>,
    loads: Vec<FrameLocation>,
}

impl Inspector<TestContext> for DepthInspector {
    fn initialize_interp(&mut self, interp: &mut Interpreter, _: &mut TestContext) {
        self.frames.push(FrameLocation::from_interpreter(interp));
    }

    fn step(&mut self, interp: &mut Interpreter, _: &mut TestContext) {
        if interp.bytecode.opcode() == opcode::SLOAD {
            self.loads.push(FrameLocation::from_interpreter(interp));
        }
    }
}

#[test]
fn nested_stylus_calls_preserve_inspector_depth_and_storage_owner() {
    for evm_leaf in [false, true] {
        for mode in 0..=2 {
            let mut ctx = setup_context_with_arbos_state();
            let router = deploy_wat_program(&mut ctx, include_bytes!("../test-data/call.wat"));
            let leaf = if evm_leaf {
                let address = Address::repeat_byte(0x44);
                ctx.journal_mut().load_account(address).unwrap();
                ctx.journal_mut().set_code(
                    address,
                    Bytecode::new_legacy(Bytes::from_static(&[
                        opcode::PUSH0,
                        opcode::SLOAD,
                        opcode::PUSH0,
                        opcode::MSTORE,
                        opcode::PUSH1,
                        32,
                        opcode::PUSH0,
                        opcode::RETURN,
                    ])),
                );
                address
            } else {
                deploy_wat_program(
                    &mut ctx,
                    br#"(module
                        (import "vm_hooks" "storage_load_bytes32" (func $load (param i32 i32)))
                        (import "vm_hooks" "write_result" (func $result (param i32 i32)))
                        (memory (export "memory") 1 1)
                        (func (export "user_entrypoint") (param i32) (result i32)
                            (call $load (i32.const 0) (i32.const 32))
                            (call $result (i32.const 32) (i32.const 32))
                            i32.const 0))"#,
                )
            };
            ctx.journal_mut()
                .sstore(router, U256::ZERO, U256::from(99))
                .unwrap();
            ctx.journal_mut()
                .sstore(leaf, U256::ZERO, U256::from(42))
                .unwrap();
            let sender = Address::repeat_byte(1);
            fund_account(&mut ctx, sender, U256::from(10_000_000_000u64));

            // Route through the same Stylus contract twice before entering the leaf.
            // Every host call temporarily replaces the EVM frame stack.
            let mut args = vec![mode];
            args.extend_from_slice(router.as_slice());
            args.push(mode);
            args.extend_from_slice(leaf.as_slice());
            let tx = create_call_tx(router, args, 10_000_000);
            let mut plain = create_evm(ctx.clone());
            let expected = execute_tx(&mut plain, tx.clone());
            let mut inspected = create_evm(ctx).with_inspector(DepthInspector::default());
            let actual = inspected.inspect_one_tx(tx.into()).unwrap();
            assert!(
                actual.is_success(),
                "mode {mode}, EVM leaf {evm_leaf}: {actual:?}"
            );
            assert_eq!(actual, expected, "inspection must preserve output and gas");
            assert_eq!(
                U256::from_be_slice(actual.output().unwrap()),
                U256::from(if mode == 2 { 99 } else { 42 })
            );
            let leaf_location = FrameLocation {
                depth: 2,
                target: if mode == 2 { router } else { leaf },
                bytecode: Some(leaf),
                caller: if mode == 2 { sender } else { router },
                is_static: mode == 1,
            };
            assert_eq!(
                inspected.0.inspector.frames,
                vec![
                    FrameLocation {
                        depth: 0,
                        target: router,
                        bytecode: Some(router),
                        caller: sender,
                        is_static: false,
                    },
                    FrameLocation {
                        depth: 1,
                        target: router,
                        bytecode: Some(router),
                        caller: if mode == 2 { sender } else { router },
                        is_static: mode == 1,
                    },
                    leaf_location.clone(),
                ]
            );
            assert_eq!(inspected.0.inspector.loads, vec![leaf_location]);
            assert_eq!(inspected.0.ctx.journal().state, plain.0.ctx.journal().state);
        }
    }
}
