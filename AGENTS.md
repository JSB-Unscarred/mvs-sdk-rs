<!--  mvs-sdk-rs -->
# 项目功能

- 本项目是对海康威视工业相机的MVS SDK的Rust安全封装

# 语言

- 主体用中文，但是英文术语不需要翻译成中文
- 保持简洁，不要使用“没有xx”"保持xx"的句式

# 编码规则

- 一切从简，不要Overdesign，优先给出能实现厂商说明文档中所有接口的安全包装的最小设计。
- 安全与否需要你根据厂商的文档、示例程序和头文件等进行适当的推导，厂商的SDK的安全性足以用于生产环境
- 不要为“理论上可能出现的问题”引入过度复杂的安全设计；每个安全设计都要在方案和注释中精简地给出理由
- 每个组件只能有一个owner，只负责自己局部资源的清理
- 函数、模块都要用注释描述其功能，说明为了防止什么问题，引入了什么安全设计；但要保持简洁，特别是不记录修改历史
- 测试必须要精简，必要测试要注释说明针对的功能或约定
- 修改代码后要同步更新注释和测试
- 尽可能减少模块、类型、字段、线程和中间状态
- 新增一个 SDK 接口要同步四处：facade（`src/camera.rs` 或 `src/library.rs`）、`src/backend/windows/`、`src/backend/unsupported.rs`、README 接口表；改动生命周期时一并更新对应时序图

# Git

- 修改代码后要给出详细的commit message，使用英文前缀加中文说明，描述修改的内容；正文记录对每个部分的修改

# README文档

- 维护一个SDK接口对应的安全Rust接口定义表格
- 维护一个SDK结构体对应的Rust结构体定义表格
- 本项目暂时不会发布到Crates.io

# 索引

- SDK的环境变量： MVCAM_COMMON_RUNENV
- SDK说明文档：C:\Program Files (x86)\MVS\Development\Documentations\工业相机Windows SDK开发指南V4.7.0（C）.chm
- SDK的示例程序说明文档：C:\Program Files (x86)\MVS\Development\Documentations\工业相机Windows SDK C++示例程序说明.pdf
- SDK的头文件目录：C:\Program Files (x86)\MVS\Development\Includes
- SDK的示例程序目录：C:\Program Files (x86)\MVS\Development\Samples\C++
- 生命周期与调用时序总览：README.md#生命周期约束
- Callback取流主流程：时序图/Callback取流.md
- 轮询取图与buffer归还：时序图/轮询取图与buffer归还.md
- Camera显式关闭与Drop兜底：时序图/Camera显式关闭与Drop兜底.md
- Sdk shutdown的终态约束：时序图/Sdk-shutdown的终态约束.md
