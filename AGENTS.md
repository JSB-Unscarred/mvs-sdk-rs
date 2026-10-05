<!--  mvs-sdk-rs -->
# 语言

- 主体用中文，但是英文术语不需要翻译成中文
- 保持简洁，不要使用“没有xx”“保持xx"的句式

# 编码规则

- 一切从简，不要Overdesign，优先给出能实现厂商说明文档中所有接口的安全包装的最小设计。
- 安全与否需要你根据厂商的文档、示例程序和头文件等进行适当的推导，厂商的SDK的安全性足以用于生产环境
- 不要为“理论上可能出现的问题”引入过度复杂的安全设计；每个安全设计都要在方案和注释中精简地给出理由
- 每个组件只能有一个owner，只负责自己局部资源的清理
- 函数、模块都要用注释描述其功能，说明为了防止什么问题，引入了什么安全设计；但要保持简洁，特别是不记录修改历史
- 测试只保留涉及算法或内存安全约定的用例，每个用例注释说明针对的约定
- 修改代码后要同步更新注释和测试
- 尽可能减少模块、类型、字段、线程和中间状态
- 结构参照 realsense-rust，并与 3dmvs-sdk-rs（`mv3d-lp`）保持一致：每种 native 资源由一个类型拥有并在 `Drop` 中释放，方法直接调用 `sys`；调用顺序用类型与借用表达，不用运行时状态检查
- 新增一个 SDK 接口时同步 README 接口表；改动生命周期或所有权时同步 `src/docs/architecture.rs`

# Git

- 修改代码后要给出详细的commit message，使用英文前缀加中文说明，描述修改的内容；正文记录对每个部分的修改

# README文档

- 维护一个SDK接口对应的安全Rust接口定义表格
- 维护一个SDK结构体对应的Rust结构体定义表格
- README.md（中文）是主版本，README.en.md 是它的英文翻译，改动时同步
- 两份 README 中的 Rust 示例参与 doctest，改动 API 时同步

# 发布

- `mvs-sdk-sys` 与 `mvs-sdk` 同版本发布，先发 sys；安全 crate 再导出 sys，sys 的破坏性变更视同安全 crate 的破坏性变更
- 发布前运行 `cargo package --workspace --list` 检查包内容，并联网运行 `cargo publish --workspace --dry-run`

# 索引

- SDK 开发目录（生成 bindings 时使用）：环境变量 MVCAM_COMMON_RUNENV，默认 C:\Program Files (x86)\MVS\Development
- SDK的头文件目录：C:\Program Files (x86)\MVS\Development\Includes
- SDK说明文档：C:\Program Files (x86)\MVS\Development\Documentations\工业相机Windows SDK开发指南V4.7.0（C）.chm
- SDK的示例程序说明文档：C:\Program Files (x86)\MVS\Development\Documentations\工业相机Windows SDK C++示例程序说明.pdf
- SDK的示例程序目录：C:\Program Files (x86)\MVS\Development\Samples\C++
- bindings 生成脚本：tools/generate-bindings-windows-x64.ps1，用法见 mvs-sdk-sys/README.md
- 所有权、会话、类型状态、清理失败与 callback 约定：src/docs/architecture.rs
- 使用示例：examples/
- 真机测试：环境变量 MVS_SDK_TEST_SERIAL，`cargo test --test hardware_smoke -- --ignored`
