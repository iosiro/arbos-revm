;; Math operations test program for arbos-revm
;; Protocol: First byte selects operation, followed by operands (32 bytes each)
;;   0x00 + a + b = math_div(a, b) -> result in a
;;   0x01 + a + b = math_mod(a, b) -> result in a
;;   0x02 + a + b = math_pow(a, b) -> result in a
;;   0x03 + a + b + c = math_add_mod(a, b, c) -> result in a
;;   0x04 + a + b + c = math_mul_mod(a, b, c) -> result in a

(module
    (import "vm_hooks" "read_args"     (func $read_args     (param i32)))
    (import "vm_hooks" "write_result"  (func $write_result  (param i32 i32)))
    (import "vm_hooks" "math_div"      (func $math_div      (param i32 i32)))
    (import "vm_hooks" "math_mod"      (func $math_mod      (param i32 i32)))
    (import "vm_hooks" "math_pow"      (func $math_pow      (param i32 i32)))
    (import "vm_hooks" "math_add_mod"  (func $math_add_mod  (param i32 i32 i32)))
    (import "vm_hooks" "math_mul_mod"  (func $math_mul_mod  (param i32 i32 i32)))
    (memory (export "memory") 1 1)

    ;; Memory layout:
    ;; 0-127:   Input args
    ;; 128-159: Operand a (32 bytes) - also result location
    ;; 160-191: Operand b (32 bytes)
    ;; 192-223: Operand c (32 bytes) - for mod operations

    (func (export "user_entrypoint") (param $args_len i32) (result i32)
        (local $selector i32)

        ;; Read args into memory at offset 0
        (call $read_args (i32.const 0))

        ;; Get selector from first byte
        (local.set $selector (i32.load8_u (i32.const 0)))

        ;; Copy operand a from offset 1 to offset 128
        (memory.copy (i32.const 128) (i32.const 1) (i32.const 32))

        ;; Copy operand b from offset 33 to offset 160
        (memory.copy (i32.const 160) (i32.const 33) (i32.const 32))

        ;; math_div (0x00)
        (if (i32.eqz (local.get $selector))
            (then
                (call $math_div (i32.const 128) (i32.const 160))
                (call $write_result (i32.const 128) (i32.const 32))
                (return (i32.const 0))
            )
        )

        ;; math_mod (0x01)
        (if (i32.eq (local.get $selector) (i32.const 1))
            (then
                (call $math_mod (i32.const 128) (i32.const 160))
                (call $write_result (i32.const 128) (i32.const 32))
                (return (i32.const 0))
            )
        )

        ;; math_pow (0x02)
        (if (i32.eq (local.get $selector) (i32.const 2))
            (then
                (call $math_pow (i32.const 128) (i32.const 160))
                (call $write_result (i32.const 128) (i32.const 32))
                (return (i32.const 0))
            )
        )

        ;; math_add_mod (0x03)
        (if (i32.eq (local.get $selector) (i32.const 3))
            (then
                ;; Copy operand c from offset 65 to offset 192
                (memory.copy (i32.const 192) (i32.const 65) (i32.const 32))
                (call $math_add_mod (i32.const 128) (i32.const 160) (i32.const 192))
                (call $write_result (i32.const 128) (i32.const 32))
                (return (i32.const 0))
            )
        )

        ;; math_mul_mod (0x04)
        (if (i32.eq (local.get $selector) (i32.const 4))
            (then
                ;; Copy operand c from offset 65 to offset 192
                (memory.copy (i32.const 192) (i32.const 65) (i32.const 32))
                (call $math_mul_mod (i32.const 128) (i32.const 160) (i32.const 192))
                (call $write_result (i32.const 128) (i32.const 32))
                (return (i32.const 0))
            )
        )

        ;; Unknown selector - return empty
        (call $write_result (i32.const 0) (i32.const 0))
        (i32.const 0)
    )
)
