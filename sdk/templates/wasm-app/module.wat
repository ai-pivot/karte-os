// karte-sdk wasm-app template — 最小可运行模块
// 解释器 v0 指令集：i32.const(0x41) / i32.add(0x6A) / drop(0x1A) / end(0x0B)
// 本模块导出 main = 7 + 35 = 42
(module
  (func (export "main") (result i32)
    i32.const 7
    i32.const 35
    i32.add
  )
)
