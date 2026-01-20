;; Contract call test program for arbos-revm
;; Protocol:
;;   0x00 + address (20 bytes) + calldata = call_contract
;;   0x01 + address (20 bytes) + calldata = static_call_contract
;;   0x02 + address (20 bytes) + calldata = delegate_call_contract
;;
;; Returns the call result (return data from called contract)

(module
    (import "vm_hooks" "read_args"            (func $read_args            (param i32)))
    (import "vm_hooks" "write_result"         (func $write_result         (param i32 i32)))
    (import "vm_hooks" "call_contract"        (func $call_contract        (param i32 i32 i32 i32 i64 i32) (result i32)))
    (import "vm_hooks" "static_call_contract" (func $static_call_contract (param i32 i32 i32 i64 i32) (result i32)))
    (import "vm_hooks" "delegate_call_contract" (func $delegate_call_contract (param i32 i32 i32 i64 i32) (result i32)))
    (import "vm_hooks" "read_return_data"     (func $read_return_data     (param i32 i32 i32) (result i32)))
    (import "vm_hooks" "return_data_size"     (func $return_data_size     (result i32)))
    (memory (export "memory") 1 1)

    ;; Memory layout:
    ;; 0-255:    Input args buffer
    ;; 256-275:  Target address (20 bytes)
    ;; 276-307:  Value buffer for call (32 bytes, zeros for no value)
    ;; 308-311:  Return data length (4 bytes / i32)
    ;; 312-2047: Calldata buffer
    ;; 2048-4095: Return data buffer

    (func (export "user_entrypoint") (param $args_len i32) (result i32)
        (local $selector i32)
        (local $calldata_len i32)
        (local $call_result i32)
        (local $return_len i32)

        ;; Read args into memory at offset 0
        (call $read_args (i32.const 0))

        ;; Get selector from first byte
        (local.set $selector (i32.load8_u (i32.const 0)))

        ;; Copy target address from offset 1 to offset 256
        (memory.copy (i32.const 256) (i32.const 1) (i32.const 20))

        ;; Calculate calldata length: args_len - 1 (selector) - 20 (address)
        (local.set $calldata_len (i32.sub (local.get $args_len) (i32.const 21)))

        ;; Copy calldata from offset 21 to offset 312
        (if (i32.gt_s (local.get $calldata_len) (i32.const 0))
            (then
                (memory.copy (i32.const 312) (i32.const 21) (local.get $calldata_len))
            )
        )

        ;; Clear value buffer (32 bytes of zeros at offset 276)
        (memory.fill (i32.const 276) (i32.const 0) (i32.const 32))

        ;; call_contract (0x00)
        (if (i32.eqz (local.get $selector))
            (then
                ;; call_contract(contract, calldata, calldata_len, value, gas, return_data_len)
                (local.set $call_result
                    (call $call_contract
                        (i32.const 256)           ;; target address
                        (i32.const 312)           ;; calldata pointer
                        (local.get $calldata_len) ;; calldata length
                        (i32.const 276)           ;; value (zeros = no value)
                        (i64.const 1000000)       ;; gas limit
                        (i32.const 308)           ;; return data length output
                    )
                )

                ;; Get return data size and copy to buffer
                (local.set $return_len (call $return_data_size))
                (if (i32.gt_s (local.get $return_len) (i32.const 0))
                    (then
                        (drop (call $read_return_data
                            (i32.const 2048)          ;; dest
                            (i32.const 0)             ;; offset
                            (local.get $return_len)   ;; size
                        ))
                    )
                )

                ;; If call succeeded, write return data; otherwise write error byte
                (if (i32.eqz (local.get $call_result))
                    (then
                        (call $write_result (i32.const 2048) (local.get $return_len))
                    )
                    (else
                        ;; Write failure status byte
                        (i32.store8 (i32.const 2048) (local.get $call_result))
                        (call $write_result (i32.const 2048) (i32.const 1))
                    )
                )
                (return (i32.const 0))
            )
        )

        ;; static_call_contract (0x01)
        (if (i32.eq (local.get $selector) (i32.const 1))
            (then
                ;; static_call_contract(contract, calldata, calldata_len, gas, return_data_len)
                (local.set $call_result
                    (call $static_call_contract
                        (i32.const 256)           ;; target address
                        (i32.const 312)           ;; calldata pointer
                        (local.get $calldata_len) ;; calldata length
                        (i64.const 1000000)       ;; gas limit
                        (i32.const 308)           ;; return data length output
                    )
                )

                ;; Get return data size and copy to buffer
                (local.set $return_len (call $return_data_size))
                (if (i32.gt_s (local.get $return_len) (i32.const 0))
                    (then
                        (drop (call $read_return_data
                            (i32.const 2048)          ;; dest
                            (i32.const 0)             ;; offset
                            (local.get $return_len)   ;; size
                        ))
                    )
                )

                ;; If call succeeded, write return data; otherwise write error byte
                (if (i32.eqz (local.get $call_result))
                    (then
                        (call $write_result (i32.const 2048) (local.get $return_len))
                    )
                    (else
                        (i32.store8 (i32.const 2048) (local.get $call_result))
                        (call $write_result (i32.const 2048) (i32.const 1))
                    )
                )
                (return (i32.const 0))
            )
        )

        ;; delegate_call_contract (0x02)
        (if (i32.eq (local.get $selector) (i32.const 2))
            (then
                ;; delegate_call_contract(contract, calldata, calldata_len, gas, return_data_len)
                (local.set $call_result
                    (call $delegate_call_contract
                        (i32.const 256)           ;; target address
                        (i32.const 312)           ;; calldata pointer
                        (local.get $calldata_len) ;; calldata length
                        (i64.const 1000000)       ;; gas limit
                        (i32.const 308)           ;; return data length output
                    )
                )

                ;; Get return data size and copy to buffer
                (local.set $return_len (call $return_data_size))
                (if (i32.gt_s (local.get $return_len) (i32.const 0))
                    (then
                        (drop (call $read_return_data
                            (i32.const 2048)          ;; dest
                            (i32.const 0)             ;; offset
                            (local.get $return_len)   ;; size
                        ))
                    )
                )

                ;; If call succeeded, write return data; otherwise write error byte
                (if (i32.eqz (local.get $call_result))
                    (then
                        (call $write_result (i32.const 2048) (local.get $return_len))
                    )
                    (else
                        (i32.store8 (i32.const 2048) (local.get $call_result))
                        (call $write_result (i32.const 2048) (i32.const 1))
                    )
                )
                (return (i32.const 0))
            )
        )

        ;; Unknown selector - return empty
        (call $write_result (i32.const 0) (i32.const 0))
        (i32.const 0)
    )
)
