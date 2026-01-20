;; WASM start function test program for arbos-revm
;; Tests that WASM start functions execute before user_entrypoint
;; Exports:
;;   status (global) - Initialized to 10, incremented to 11 by start function
;;   move_me() - The start function that increments status
;;   user_entrypoint() -> always returns 0 (success)

(module
    (global $status (export "status") (mut i32) (i32.const 10))
    (memory 0 0)
    (export "memory" (memory 0))
    (type $void (func (param) (result)))
    (func $start (export "move_me") (type $void)
        get_global $status
        i32.const 1
        i32.add
        set_global $status ;; increment the global
    )
    (func (export "user_entrypoint") (param $args_len i32) (result i32)
        (i32.const 0)
    )
    (start $start))
