
;; Account info test program for arbos-revm
;; Protocol:
;;   0x00 + address (20 bytes) = get account balance (returns 32 bytes)
;;   0x01 + address (20 bytes) = get account code hash (returns 32 bytes)

(module
    (import "vm_hooks" "read_args"       (func $read_args       (param i32)))
    (import "vm_hooks" "write_result"    (func $write_result    (param i32 i32)))
    (import "vm_hooks" "account_balance" (func $account_balance (param i32 i32)))
    (import "vm_hooks" "account_codehash" (func $account_codehash (param i32 i32)))
    (memory (export "memory") 1 1)

    ;; Memory layout:
    ;; 0-255:   Input args buffer
    ;; 256-275: Address buffer (20 bytes)
    ;; 276-307: Result buffer (32 bytes)

    (func (export "user_entrypoint") (param $args_len i32) (result i32)
        (local $selector i32)

        ;; Read args into memory at offset 0
        (call $read_args (i32.const 0))

        ;; Get selector from first byte
        (local.set $selector (i32.load8_u (i32.const 0)))

        ;; Copy address from offset 1 to offset 256
        (memory.copy (i32.const 256) (i32.const 1) (i32.const 20))

        ;; 0x00 = get account balance
        (if (i32.eqz (local.get $selector))
            (then
                ;; account_balance(address, dest)
                (call $account_balance (i32.const 256) (i32.const 276))
                (call $write_result (i32.const 276) (i32.const 32))
                (return (i32.const 0))
            )
        )

        ;; 0x01 = get account code hash
        (if (i32.eq (local.get $selector) (i32.const 1))
            (then
                ;; account_codehash(address, dest)
                (call $account_codehash (i32.const 256) (i32.const 276))
                (call $write_result (i32.const 276) (i32.const 32))
                (return (i32.const 0))
            )
        )

        ;; Unknown selector - return empty
        (call $write_result (i32.const 0) (i32.const 0))
        (i32.const 0)
    )
)
