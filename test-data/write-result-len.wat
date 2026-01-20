;; Configurable result length test program for arbos-revm
;; Protocol:
;;   First 4 bytes of input (little-endian) = number of bytes to return
;; Returns: that many bytes from the input buffer
;;
;; Used to test write_result with various output sizes

(module
    (import "vm_hooks" "read_args"    (func $read_args    (param i32)))
    (import "vm_hooks" "write_result" (func $write_result (param i32 i32)))
    (memory (export "memory") 2 2)
    (func $main (export "user_entrypoint") (param $args_len i32) (result i32)
        (local $len i32)

        ;; write args to 0x0
        (call $read_args (i32.const 0))

        ;; treat first 4 bytes as size to write
        (i32.load (i32.const 0))
        local.set $len

        ;; call write
        (call $write_result (i32.const 0) (local.get $len))

        ;; return success
        i32.const 0
    )
)
