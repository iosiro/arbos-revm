;; Functional regression for Nitro's pre-Stylus-v3 memory.fill value trap.
(module
    (memory (export "memory") 1 1)
    (func (export "user_entrypoint") (param $args_len i32) (result i32)
        (memory.fill (i32.const 0) (i32.const 256) (i32.const 1))
        (i32.const 0)
    )
)
