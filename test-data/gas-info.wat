;; Gas/Ink introspection test program for arbos-revm
;; Protocol:
;;   0x00 = get evm_gas_left (returns 8 bytes u64)
;;   0x01 = get evm_ink_left (returns 8 bytes u64)
;;   0x02 = get tx_gas_price (returns 32 bytes U256)
;;   0x03 = get tx_ink_price (returns 8 bytes u64)

(module
    (import "vm_hooks" "read_args"     (func $read_args     (param i32)))
    (import "vm_hooks" "write_result"  (func $write_result  (param i32 i32)))
    (import "vm_hooks" "evm_gas_left"  (func $evm_gas_left  (result i64)))
    (import "vm_hooks" "evm_ink_left"  (func $evm_ink_left  (result i64)))
    (import "vm_hooks" "tx_gas_price"  (func $tx_gas_price  (param i32)))
    (import "vm_hooks" "tx_ink_price"  (func $tx_ink_price  (result i32)))
    (memory (export "memory") 1 1)

    ;; Memory layout:
    ;; 0-255:   Input args buffer
    ;; 256-287: Result buffer (32 bytes for tx_gas_price, 8 bytes for others)

    (func (export "user_entrypoint") (param $args_len i32) (result i32)
        (local $selector i32)
        (local $gas_left i64)
        (local $ink_left i64)
        (local $ink_price i32)

        ;; Read args into memory at offset 0
        (call $read_args (i32.const 0))

        ;; Get selector from first byte
        (local.set $selector (i32.load8_u (i32.const 0)))

        ;; 0x00 = evm_gas_left
        (if (i32.eqz (local.get $selector))
            (then
                (local.set $gas_left (call $evm_gas_left))
                ;; Store as little-endian u64
                (i64.store (i32.const 256) (local.get $gas_left))
                (call $write_result (i32.const 256) (i32.const 8))
                (return (i32.const 0))
            )
        )

        ;; 0x01 = evm_ink_left
        (if (i32.eq (local.get $selector) (i32.const 1))
            (then
                (local.set $ink_left (call $evm_ink_left))
                ;; Store as little-endian u64
                (i64.store (i32.const 256) (local.get $ink_left))
                (call $write_result (i32.const 256) (i32.const 8))
                (return (i32.const 0))
            )
        )

        ;; 0x02 = tx_gas_price
        (if (i32.eq (local.get $selector) (i32.const 2))
            (then
                ;; tx_gas_price writes 32 bytes to the provided pointer
                (call $tx_gas_price (i32.const 256))
                (call $write_result (i32.const 256) (i32.const 32))
                (return (i32.const 0))
            )
        )

        ;; 0x03 = tx_ink_price
        (if (i32.eq (local.get $selector) (i32.const 3))
            (then
                (local.set $ink_price (call $tx_ink_price))
                ;; Store as little-endian u32 (zero-extend to 8 bytes for consistency)
                (i64.store (i32.const 256) (i64.extend_i32_u (local.get $ink_price)))
                (call $write_result (i32.const 256) (i32.const 8))
                (return (i32.const 0))
            )
        )

        ;; Unknown selector - return empty
        (call $write_result (i32.const 0) (i32.const 0))
        (i32.const 0)
    )
)
