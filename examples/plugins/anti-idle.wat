(module
  (import "nbmcbot" "tick" (func $tick (result i64)))
  (import "nbmcbot" "look" (func $look (param f32 f32)))
  (memory (export "memory") 1 1)
  (func (export "on_tick")
    call $tick
    i64.const 200
    i64.rem_u
    i64.eqz
    if
      call $tick
      i64.const 360
      i64.rem_u
      f32.convert_i64_u
      f32.const 0
      call $look
    end))
