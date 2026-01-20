;; Transient storage test program for arbos-revm
;; Protocol:
;;   0x00 + key (32 bytes) = read transient storage slot, return value
;;   0x01 + key (32 bytes) + value (32 bytes) = write transient storage slot
;;   0x02 + key (32 bytes) + value (32 bytes) = write then read back (same tx)

(module
    (import "vm_hooks" "read_args"               (func $read_args               (param i32)))
    (import "vm_hooks" "write_result"            (func $write_result            (param i32 i32)))
    (import "vm_hooks" "transient_load_bytes32"  (func $transient_load_bytes32  (param i32 i32)))
    (import "vm_hooks" "transient_store_bytes32" (func $transient_store_bytes32 (param i32 i32)))
    (memory (export "memory") 1 1)

    ;; Memory layout:
    ;; 0-127:   Input args buffer
    ;; 128-159: Key buffer (32 bytes)
    ;; 160-191: Value buffer (32 bytes)
    ;; 192-223: Result buffer (32 bytes)

    (func (export "user_entrypoint") (param $args_len i32) (result i32)
        (local $selector i32)

        ;; Read args into memory at offset 0
        (call $read_args (i32.const 0))

        ;; Get selector from first byte
        (local.set $selector (i32.load8_u (i32.const 0)))

        ;; Read operation (0x00)
        (if (i32.eqz (local.get $selector))
            (then
                ;; Copy key from offset 1 to offset 128
                (memory.copy (i32.const 128) (i32.const 1) (i32.const 32))

                ;; Load transient storage value into offset 192
                (call $transient_load_bytes32 (i32.const 128) (i32.const 192))

                ;; Return the loaded value (32 bytes)
                (call $write_result (i32.const 192) (i32.const 32))
                (return (i32.const 0))
            )
        )

        ;; Write operation (0x01)
        (if (i32.eq (local.get $selector) (i32.const 1))
            (then
                ;; Copy key from offset 1 to offset 128
                (memory.copy (i32.const 128) (i32.const 1) (i32.const 32))

                ;; Copy value from offset 33 to offset 160
                (memory.copy (i32.const 160) (i32.const 33) (i32.const 32))

                ;; Store to transient storage
                (call $transient_store_bytes32 (i32.const 128) (i32.const 160))

                ;; Return empty (success)
                (call $write_result (i32.const 0) (i32.const 0))
                (return (i32.const 0))
            )
        )

        ;; Write-then-read operation (0x02) - for testing within single tx
        (if (i32.eq (local.get $selector) (i32.const 2))
            (then
                ;; Copy key from offset 1 to offset 128
                (memory.copy (i32.const 128) (i32.const 1) (i32.const 32))

                ;; Copy value from offset 33 to offset 160
                (memory.copy (i32.const 160) (i32.const 33) (i32.const 32))

                ;; Store to transient storage
                (call $transient_store_bytes32 (i32.const 128) (i32.const 160))

                ;; Read it back into offset 192
                (call $transient_load_bytes32 (i32.const 128) (i32.const 192))

                ;; Return the loaded value (should match what was written)
                (call $write_result (i32.const 192) (i32.const 32))
                (return (i32.const 0))
            )
        )

        ;; Unknown selector - return empty
        (call $write_result (i32.const 0) (i32.const 0))
        (i32.const 0)
    )
)
