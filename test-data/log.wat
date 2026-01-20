;; Log emission test program for arbos-revm
;; Protocol:
;;   First byte = number of topics (0-4)
;;   Following bytes = topics (32 bytes each) + data (remaining bytes)

(module
    (import "vm_hooks" "read_args"    (func $read_args    (param i32)))
    (import "vm_hooks" "write_result" (func $write_result (param i32 i32)))
    (import "vm_hooks" "emit_log"     (func $emit_log     (param i32 i32 i32)))
    (memory (export "memory") 1 1)

    ;; Memory layout:
    ;; 0:       topic count
    ;; 1-128:   topics (up to 4 * 32 bytes)
    ;; 129+:    log data

    (func (export "user_entrypoint") (param $args_len i32) (result i32)
        (local $topic_count i32)
        (local $data_offset i32)
        (local $data_len i32)

        ;; Read args into memory at offset 0
        (call $read_args (i32.const 0))

        ;; Get topic count from first byte
        (local.set $topic_count (i32.load8_u (i32.const 0)))

        ;; Calculate data offset: 1 + (topic_count * 32)
        (local.set $data_offset
            (i32.add
                (i32.const 1)
                (i32.mul (local.get $topic_count) (i32.const 32))
            )
        )

        ;; Calculate data length: args_len - data_offset
        (local.set $data_len
            (i32.sub (local.get $args_len) (local.get $data_offset))
        )

        ;; emit_log expects: (data_ptr, data_len, topic_count)
        ;; The data_ptr should point to: topics (topic_count * 32 bytes) + log_data
        ;; So we pass offset 1 (start of topics) and the total length
        (call $emit_log
            (i32.const 1)  ;; ptr to topics + data (starts at offset 1)
            (i32.sub (local.get $args_len) (i32.const 1))  ;; total len minus first byte
            (local.get $topic_count)
        )

        ;; Return empty result
        (call $write_result (i32.const 0) (i32.const 0))

        ;; Return success
        (i32.const 0)
    )
)
