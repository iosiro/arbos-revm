
;; Revert test program for arbos-revm
;; Protocol:
;;   0x00 = succeed with input as output (echo)
;;   0x01 = revert with input as revert data
;;   0x02 = revert with custom error message "CustomError"
;;   0x03 = revert with empty data

(module
    (import "vm_hooks" "read_args"    (func $read_args    (param i32)))
    (import "vm_hooks" "write_result" (func $write_result (param i32 i32)))
    (memory (export "memory") 1 1)

    ;; Memory layout:
    ;; 0-255:   Input args buffer
    ;; 256-511: Output/revert data buffer

    ;; Custom error message "CustomError" at offset 512
    (data (i32.const 512) "CustomError")

    (func (export "user_entrypoint") (param $args_len i32) (result i32)
        (local $selector i32)
        (local $data_len i32)

        ;; Read args into memory at offset 0
        (call $read_args (i32.const 0))

        ;; Get selector from first byte
        (local.set $selector (i32.load8_u (i32.const 0)))

        ;; Calculate data length (args minus selector)
        (local.set $data_len (i32.sub (local.get $args_len) (i32.const 1)))

        ;; 0x00 = succeed with echo
        (if (i32.eqz (local.get $selector))
            (then
                ;; Copy input data (after selector) to output buffer
                (if (i32.gt_s (local.get $data_len) (i32.const 0))
                    (then
                        (memory.copy (i32.const 256) (i32.const 1) (local.get $data_len))
                    )
                )
                (call $write_result (i32.const 256) (local.get $data_len))
                (return (i32.const 0)) ;; Success
            )
        )

        ;; 0x01 = revert with input as revert data
        (if (i32.eq (local.get $selector) (i32.const 1))
            (then
                ;; Copy input data to revert buffer
                (if (i32.gt_s (local.get $data_len) (i32.const 0))
                    (then
                        (memory.copy (i32.const 256) (i32.const 1) (local.get $data_len))
                    )
                )
                (call $write_result (i32.const 256) (local.get $data_len))
                (return (i32.const 1)) ;; Revert
            )
        )

        ;; 0x02 = revert with custom error "CustomError"
        (if (i32.eq (local.get $selector) (i32.const 2))
            (then
                (call $write_result (i32.const 512) (i32.const 11)) ;; "CustomError" = 11 bytes
                (return (i32.const 1)) ;; Revert
            )
        )

        ;; 0x03 = revert with empty data
        (if (i32.eq (local.get $selector) (i32.const 3))
            (then
                (call $write_result (i32.const 256) (i32.const 0)) ;; Empty
                (return (i32.const 1)) ;; Revert
            )
        )

        ;; Unknown selector - succeed with empty output
        (call $write_result (i32.const 0) (i32.const 0))
        (i32.const 0)
    )
)
