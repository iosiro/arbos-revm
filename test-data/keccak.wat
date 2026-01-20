;; Native keccak256 test program for arbos-revm
;; Takes input data and returns keccak256 hash (32 bytes)

(module
    (import "vm_hooks" "read_args"        (func $read_args        (param i32)))
    (import "vm_hooks" "write_result"     (func $write_result     (param i32 i32)))
    (import "vm_hooks" "native_keccak256" (func $native_keccak256 (param i32 i32 i32)))
    (memory (export "memory") 1 1)

    ;; Memory layout:
    ;; 0-1023:    Input data buffer
    ;; 1024-1055: Output hash buffer (32 bytes)

    (func (export "user_entrypoint") (param $args_len i32) (result i32)
        ;; Read args into memory at offset 0
        (call $read_args (i32.const 0))

        ;; Call native_keccak256(input_ptr, input_len, output_ptr)
        (call $native_keccak256
            (i32.const 0)           ;; input pointer
            (local.get $args_len)   ;; input length
            (i32.const 1024)        ;; output pointer
        )

        ;; Return the hash (32 bytes)
        (call $write_result (i32.const 1024) (i32.const 32))

        ;; Return success
        (i32.const 0)
    )
)
