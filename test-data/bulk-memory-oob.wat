;; Copyright 2023, Offchain Labs, Inc.
;; For license information, see https://github.com/OffchainLabs/nitro/blob/master/LICENSE.md

;; Bulk memory operations out-of-bounds test program for arbos-revm
;; Tests memory.fill and memory.copy near page boundaries
;; Exports:
;;   fill() - Attempts memory.fill at boundary (should trap on OOB)
;;   copy_left() - Attempts memory.copy leftward at boundary
;;   copy_right() - Attempts memory.copy rightward at boundary
;;   copy_same() - Attempts memory.copy at same location
;;   user_entrypoint() -> always returns 0 (success)

(module
    (func (export "fill")
        (memory.fill (i32.const 0xffff) (i32.const 0) (i32.const 2)))
    (func (export "copy_left")
        (memory.copy (i32.const 0xffff) (i32.const 0xfffe) (i32.const 2)))
    (func (export "copy_right")
        (memory.copy (i32.const 0xfffe) (i32.const 0xffff) (i32.const 2)))
    (func (export "copy_same")
        (memory.copy (i32.const 0xffff) (i32.const 0xffff) (i32.const 2)))
    (func (export "user_entrypoint") (param $args_len i32) (result i32)
        (i32.const 0)
    )
    (data (i32.const 0xfffe) "\01\02") ;; last two bytes shouldn't change
    (memory (export "memory") 1 1))
