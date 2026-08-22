# 轮询取图与 buffer 归还

```mermaid
sequenceDiagram
    autonumber
    actor App as 应用
    participant Camera as Camera
    participant Native as MVS SDK
    participant Guard as FrameGuard

    Note over App,Camera: Camera 为 Stopped，image callback 未注册
    App->>Camera: start_grabbing()
    Camera->>Native: MV_CC_StartGrabbing()
    Camera-->>App: Polling

    loop 按需取图
        alt 零拷贝 guard
            App->>Camera: get_image_buffer(Timeout::Finite 或 Timeout::Infinite)
            Camera->>Native: MV_CC_GetImageBuffer(...)
            Native-->>Guard: MV_FRAME_OUT
            Guard-->>App: FrameGuard<'cam>
            App->>Guard: frame() / to_owned()
            alt 显式归还并观察错误
                App->>Guard: release(self)
                Guard->>Native: MV_CC_FreeImageBuffer(...)
                Native-->>Guard: Ok 或 native MvsError
                Guard-->>App: 返回本次结果
            else RAII 兜底
                Guard->>Native: Drop → MV_CC_FreeImageBuffer(...)
            end
        else owned 便捷入口
            App->>Camera: get_owned_frame(Timeout::Finite 或 Timeout::Infinite)
            Camera->>Native: MV_CC_GetImageBuffer(...)
            Native-->>Camera: MV_FRAME_OUT
            Camera->>Camera: 复制 OwnedFrame
            Camera->>Native: MV_CC_FreeImageBuffer(...)
            Camera-->>App: OwnedFrame 或 release MvsError
        end
    end

    App->>Camera: stop_grabbing()
    Camera->>Native: MV_CC_StopGrabbing()
```

每个 `FrameGuard` 唯一负责一份 `MV_FRAME_OUT` 的归还凭据，并借用 `Camera`，防止 buffer
仍在使用时停止或关闭相机。等待时长由 `Timeout` 表达：`Finite(ms)` 为有限等待，`Infinite`
映射到 SDK 的无限等待哨兵。`release(self)` 会消费 guard 并只尝试一次，错误用于诊断和宿主策略；`Drop` 只做一次
兜底并忽略错误。归还结束后，`Camera` 再按 Stop → callback 注销 → Close → Destroy 顺序清理。
`get_owned_frame` 复用相同 guard，复制后显式归还并传播 release 错误。Destroy 未确认成功时，
live handle 会阻止 SDK Finalize。
