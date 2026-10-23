(component
  ;; A connector tool's component: it forwards its input to the hub's `fetch` and returns the reply, so the
  ;; request's secret is the hub's to add. It keeps nothing and imports nothing else.
  (import "fetch" (func $host-fetch (param "request" string) (result string)))

  (core module $libc
    (memory (export "memory") 2)
    (global $next (mut i32) (i32.const 8192))
    (func (export "realloc") (param $old i32) (param $oldsize i32) (param $align i32) (param $newsize i32) (result i32)
      (local $p i32)
      (local.set $p (global.get $next))
      (global.set $next (i32.add (global.get $next) (local.get $newsize)))
      (local.get $p)))
  (core instance $libc-i (instantiate $libc))

  (core func $fetch-low
    (canon lower (func $host-fetch) (memory (core memory $libc-i "memory")) (realloc (core func $libc-i "realloc"))))
  (core instance $host
    (export "fetch" (func $fetch-low)))

  (core module $guest
    (import "host" "fetch" (func $fetch (param i32 i32 i32)))
    (import "libc" "memory" (memory 2))
    (func (export "run") (param $ptr i32) (param $len i32) (result i32)
      (call $fetch (local.get $ptr) (local.get $len) (i32.const 32))
      (i32.const 32)))
  (core instance $guest
    (instantiate $guest
      (with "host" (instance $host))
      (with "libc" (instance $libc-i))))

  (type $sig (func (param "input" string) (result string)))
  (func $run (type $sig)
    (canon lift (core func $guest "run") (memory (core memory $libc-i "memory")) (realloc (core func $libc-i "realloc"))))
  (export "run" (func $run)))
