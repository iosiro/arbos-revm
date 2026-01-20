;; Memory payment edge case test program for arbos-revm
;; Tests pay_for_memory_grow with edge case values:
;;   - 0 pages (no growth)
;;   - -1 (u32::MAX) pages (overflow test)
;; Returns 0 (success)

(module
    (import "vm_hooks" "pay_for_memory_grow" (func $pay_for_memory_grow (param i32)))
    (func (export "user_entrypoint") (param $args_len i32) (result i32)
        (call $pay_for_memory_grow (i32.const 0))
        (call $pay_for_memory_grow (i32.sub (i32.const 0) (i32.const 1)))
        i32.const 0
    )
    (memory (export "memory") 0)
)
