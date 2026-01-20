;; Contract creation test program for arbos-revm
;; Protocol:
;;   0x00 + value (32 bytes) + init_code = CREATE (returns 20 bytes address or zeros)
;;   0x01 + value (32 bytes) + salt (32 bytes) + init_code = CREATE2 (returns 20 bytes address or zeros)
;;   0x02 = CREATE with minimal contract (zero value, predefined init code)
;;   0x03 + salt (32 bytes) = CREATE2 with minimal contract (zero value, predefined init code)

(module
    (import "vm_hooks" "read_args"       (func $read_args       (param i32)))
    (import "vm_hooks" "write_result"    (func $write_result    (param i32 i32)))
    (import "vm_hooks" "create1"         (func $create1         (param i32 i32 i32 i32 i32)))
    (import "vm_hooks" "create2"         (func $create2         (param i32 i32 i32 i32 i32 i32)))
    (import "vm_hooks" "read_return_data" (func $read_return_data (param i32 i32 i32) (result i32)))
    (import "vm_hooks" "return_data_size" (func $return_data_size (result i32)))
    (memory (export "memory") 1 1)

    ;; Memory layout:
    ;; 0-255:   Input args buffer
    ;; 256-287: Value buffer (32 bytes)
    ;; 288-319: Salt buffer (32 bytes, for CREATE2)
    ;; 320-339: Address output buffer (20 bytes)
    ;; 340-343: Revert data length output (4 bytes / i32)
    ;; 344-600: Init code buffer
    ;; 601-1000: Revert data buffer

    ;; Minimal init code at offset 700 (deploys contract that returns empty)
    ;; Init code: 6005600c60003960056000f360006000f3
    ;; This deploys runtime code: 60006000f3 (PUSH1 0, PUSH1 0, RETURN)
    (data (i32.const 700) "\60\05\60\0c\60\00\39\60\05\60\00\f3\60\00\60\00\f3")

    (func (export "user_entrypoint") (param $args_len i32) (result i32)
        (local $selector i32)
        (local $init_code_len i32)
        (local $return_len i32)

        ;; Read args into memory at offset 0
        (call $read_args (i32.const 0))

        ;; Get selector from first byte
        (local.set $selector (i32.load8_u (i32.const 0)))

        ;; 0x00 = CREATE with custom value and init code
        (if (i32.eqz (local.get $selector))
            (then
                ;; Copy value (32 bytes) from offset 1 to offset 256
                (memory.copy (i32.const 256) (i32.const 1) (i32.const 32))

                ;; Calculate init code length: args_len - 1 (selector) - 32 (value)
                (local.set $init_code_len (i32.sub (local.get $args_len) (i32.const 33)))

                ;; Copy init code from offset 33 to offset 344
                (if (i32.gt_s (local.get $init_code_len) (i32.const 0))
                    (then
                        (memory.copy (i32.const 344) (i32.const 33) (local.get $init_code_len))
                    )
                )

                ;; create1(code, code_len, endowment, contract, revert_data_len)
                (call $create1
                    (i32.const 344)               ;; code pointer
                    (local.get $init_code_len)    ;; code length
                    (i32.const 256)               ;; endowment (value)
                    (i32.const 320)               ;; contract address output
                    (i32.const 340)               ;; revert data length output
                )

                ;; Check if creation succeeded (address is non-zero)
                ;; Write the 20-byte address result
                (call $write_result (i32.const 320) (i32.const 20))
                (return (i32.const 0))
            )
        )

        ;; 0x01 = CREATE2 with custom value, salt and init code
        (if (i32.eq (local.get $selector) (i32.const 1))
            (then
                ;; Copy value (32 bytes) from offset 1 to offset 256
                (memory.copy (i32.const 256) (i32.const 1) (i32.const 32))

                ;; Copy salt (32 bytes) from offset 33 to offset 288
                (memory.copy (i32.const 288) (i32.const 33) (i32.const 32))

                ;; Calculate init code length: args_len - 1 (selector) - 32 (value) - 32 (salt)
                (local.set $init_code_len (i32.sub (local.get $args_len) (i32.const 65)))

                ;; Copy init code from offset 65 to offset 344
                (if (i32.gt_s (local.get $init_code_len) (i32.const 0))
                    (then
                        (memory.copy (i32.const 344) (i32.const 65) (local.get $init_code_len))
                    )
                )

                ;; create2(code, code_len, endowment, salt, contract, revert_data_len)
                (call $create2
                    (i32.const 344)               ;; code pointer
                    (local.get $init_code_len)    ;; code length
                    (i32.const 256)               ;; endowment (value)
                    (i32.const 288)               ;; salt
                    (i32.const 320)               ;; contract address output
                    (i32.const 340)               ;; revert data length output
                )

                ;; Write the 20-byte address result
                (call $write_result (i32.const 320) (i32.const 20))
                (return (i32.const 0))
            )
        )

        ;; 0x02 = CREATE with minimal contract (zero value, predefined init code)
        (if (i32.eq (local.get $selector) (i32.const 2))
            (then
                ;; Zero out value
                (memory.fill (i32.const 256) (i32.const 0) (i32.const 32))

                ;; create1 with predefined init code at offset 700 (17 bytes)
                (call $create1
                    (i32.const 700)               ;; code pointer (predefined)
                    (i32.const 17)                ;; code length (17 bytes)
                    (i32.const 256)               ;; endowment (zero value)
                    (i32.const 320)               ;; contract address output
                    (i32.const 340)               ;; revert data length output
                )

                ;; Write the 20-byte address result
                (call $write_result (i32.const 320) (i32.const 20))
                (return (i32.const 0))
            )
        )

        ;; 0x03 = CREATE2 with minimal contract (zero value, predefined init code, custom salt)
        (if (i32.eq (local.get $selector) (i32.const 3))
            (then
                ;; Zero out value
                (memory.fill (i32.const 256) (i32.const 0) (i32.const 32))

                ;; Copy salt (32 bytes) from offset 1 to offset 288
                (memory.copy (i32.const 288) (i32.const 1) (i32.const 32))

                ;; create2 with predefined init code at offset 700 (17 bytes)
                (call $create2
                    (i32.const 700)               ;; code pointer (predefined)
                    (i32.const 17)                ;; code length (17 bytes)
                    (i32.const 256)               ;; endowment (zero value)
                    (i32.const 288)               ;; salt
                    (i32.const 320)               ;; contract address output
                    (i32.const 340)               ;; revert data length output
                )

                ;; Write the 20-byte address result
                (call $write_result (i32.const 320) (i32.const 20))
                (return (i32.const 0))
            )
        )

        ;; Unknown selector - return empty
        (call $write_result (i32.const 0) (i32.const 0))
        (i32.const 0)
    )
)
