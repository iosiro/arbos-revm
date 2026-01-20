;; Storage test program for arbos-revm
;; Protocol:
;;   0x00 + key (32 bytes) = read storage slot, return value
;;   0x01 + key (32 bytes) + value (32 bytes) = write storage slot, return old value

(module
    (import "vm_hooks" "read_args"              (func $read_args              (param i32)))
    (import "vm_hooks" "write_result"           (func $write_result           (param i32 i32)))
    (import "vm_hooks" "storage_load_bytes32"   (func $storage_load_bytes32   (param i32 i32)))
    (import "vm_hooks" "storage_cache_bytes32"  (func $storage_cache_bytes32  (param i32 i32)))
    (import "vm_hooks" "storage_flush_cache"    (func $storage_flush_cache    (param i32)))
    (memory (export "memory") 1 1)

    ;; Memory layout (with generous spacing to avoid any overlap):
    ;; 0-127:   Input args buffer
    ;; 128-159: Key buffer (32 bytes)
    ;; 160-191: Value buffer for write (32 bytes)
    ;; 192-223: Result buffer for loaded value (32 bytes)
    ;; 224-255: Buffer for old value (32 bytes)

    (func (export "user_entrypoint") (param $args_len i32) (result i32)
        ;; Read args into memory at offset 0
        (call $read_args (i32.const 0))

        ;; Check first byte for operation type
        (if (i32.eqz (i32.load8_u (i32.const 0)))
            (then
                ;; Read operation: 0x00 + key (32 bytes)
                ;; Copy key from offset 1 to offset 128
                (memory.copy (i32.const 128) (i32.const 1) (i32.const 32))

                ;; Load storage value into offset 192
                (call $storage_load_bytes32 (i32.const 128) (i32.const 192))

                ;; Return the loaded value (32 bytes)
                (call $write_result (i32.const 192) (i32.const 32))
            )
            (else
                ;; Write operation: 0x01 + key (32 bytes) + value (32 bytes)
                ;; Copy key from offset 1 to offset 128
                (memory.copy (i32.const 128) (i32.const 1) (i32.const 32))

                ;; Copy value from offset 33 to offset 160
                (memory.copy (i32.const 160) (i32.const 33) (i32.const 32))

                ;; First load current value into offset 224 (to return as old value)
                (call $storage_load_bytes32 (i32.const 128) (i32.const 224))

                ;; Cache the new value
                (call $storage_cache_bytes32 (i32.const 128) (i32.const 160))

                ;; Flush cache to persist (0 = don't clear cache)
                (call $storage_flush_cache (i32.const 0))

                ;; Return the old value (32 bytes)
                (call $write_result (i32.const 224) (i32.const 32))
            )
        )

        ;; Return success
        (i32.const 0)
    )
)
