;; EVM data access test program for arbos-revm
;; Protocol: First byte selects which data to return
;;   0x00 = block_number (u64)
;;   0x01 = block_timestamp (u64)
;;   0x02 = chainid (u64)
;;   0x03 = msg_sender (20 bytes)
;;   0x04 = contract_address (20 bytes)
;;   0x05 = tx_origin (20 bytes)
;;   0x06 = msg_value (32 bytes)
;;   0x07 = block_basefee (32 bytes)
;;   0x08 = block_gas_limit (u64)
;;   0x09 = block_coinbase (20 bytes)

(module
    (import "vm_hooks" "read_args"         (func $read_args         (param i32)))
    (import "vm_hooks" "write_result"      (func $write_result      (param i32 i32)))
    (import "vm_hooks" "block_number"      (func $block_number      (result i64)))
    (import "vm_hooks" "block_timestamp"   (func $block_timestamp   (result i64)))
    (import "vm_hooks" "chainid"           (func $chainid           (result i64)))
    (import "vm_hooks" "msg_sender"        (func $msg_sender        (param i32)))
    (import "vm_hooks" "contract_address"  (func $contract_address  (param i32)))
    (import "vm_hooks" "tx_origin"         (func $tx_origin         (param i32)))
    (import "vm_hooks" "msg_value"         (func $msg_value         (param i32)))
    (import "vm_hooks" "block_basefee"     (func $block_basefee     (param i32)))
    (import "vm_hooks" "block_gas_limit"   (func $block_gas_limit   (result i64)))
    (import "vm_hooks" "block_coinbase"    (func $block_coinbase    (param i32)))
    (memory (export "memory") 1 1)

    ;; Memory layout:
    ;; 0-31:    Input args
    ;; 32-63:   Result buffer for 32-byte values
    ;; 64-95:   Result buffer for addresses (20 bytes)

    (func (export "user_entrypoint") (param $args_len i32) (result i32)
        (local $selector i32)
        (local $result64 i64)

        ;; Read args into memory at offset 0
        (call $read_args (i32.const 0))

        ;; Get selector from first byte
        (local.set $selector (i32.load8_u (i32.const 0)))

        ;; block_number (0x00)
        (if (i32.eqz (local.get $selector))
            (then
                (local.set $result64 (call $block_number))
                (i64.store (i32.const 32) (local.get $result64))
                (call $write_result (i32.const 32) (i32.const 8))
                (return (i32.const 0))
            )
        )

        ;; block_timestamp (0x01)
        (if (i32.eq (local.get $selector) (i32.const 1))
            (then
                (local.set $result64 (call $block_timestamp))
                (i64.store (i32.const 32) (local.get $result64))
                (call $write_result (i32.const 32) (i32.const 8))
                (return (i32.const 0))
            )
        )

        ;; chainid (0x02)
        (if (i32.eq (local.get $selector) (i32.const 2))
            (then
                (local.set $result64 (call $chainid))
                (i64.store (i32.const 32) (local.get $result64))
                (call $write_result (i32.const 32) (i32.const 8))
                (return (i32.const 0))
            )
        )

        ;; msg_sender (0x03)
        (if (i32.eq (local.get $selector) (i32.const 3))
            (then
                (call $msg_sender (i32.const 64))
                (call $write_result (i32.const 64) (i32.const 20))
                (return (i32.const 0))
            )
        )

        ;; contract_address (0x04)
        (if (i32.eq (local.get $selector) (i32.const 4))
            (then
                (call $contract_address (i32.const 64))
                (call $write_result (i32.const 64) (i32.const 20))
                (return (i32.const 0))
            )
        )

        ;; tx_origin (0x05)
        (if (i32.eq (local.get $selector) (i32.const 5))
            (then
                (call $tx_origin (i32.const 64))
                (call $write_result (i32.const 64) (i32.const 20))
                (return (i32.const 0))
            )
        )

        ;; msg_value (0x06)
        (if (i32.eq (local.get $selector) (i32.const 6))
            (then
                (call $msg_value (i32.const 32))
                (call $write_result (i32.const 32) (i32.const 32))
                (return (i32.const 0))
            )
        )

        ;; block_basefee (0x07)
        (if (i32.eq (local.get $selector) (i32.const 7))
            (then
                (call $block_basefee (i32.const 32))
                (call $write_result (i32.const 32) (i32.const 32))
                (return (i32.const 0))
            )
        )

        ;; block_gas_limit (0x08)
        (if (i32.eq (local.get $selector) (i32.const 8))
            (then
                (local.set $result64 (call $block_gas_limit))
                (i64.store (i32.const 32) (local.get $result64))
                (call $write_result (i32.const 32) (i32.const 8))
                (return (i32.const 0))
            )
        )

        ;; block_coinbase (0x09)
        (if (i32.eq (local.get $selector) (i32.const 9))
            (then
                (call $block_coinbase (i32.const 64))
                (call $write_result (i32.const 64) (i32.const 20))
                (return (i32.const 0))
            )
        )

        ;; Unknown selector - return empty
        (call $write_result (i32.const 0) (i32.const 0))
        (i32.const 0)
    )
)
