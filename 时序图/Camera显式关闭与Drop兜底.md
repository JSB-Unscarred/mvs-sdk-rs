# Camera 显式关闭与 Drop 兜底

本图记录 `Camera` 显式关闭及 `Drop` 兜底时，callback、native handle 与资源账本的清理顺序。

```mermaid
sequenceDiagram
    autonumber
    actor Caller as 调用方
    participant Camera as Camera cleanup
    participant Slots as Callback slots
    participant Native as MVS SDK
    participant Ledger as ResourceLedger

    Caller->>Camera: close_detailed() / close() / Drop
    alt 当前位于 image 或 event callback
        Camera->>Slots: stop_accepting()
        Camera->>Slots: non-blocking drain
        Camera->>Camera: 保留 native handle 与 callback backing
        Camera->>Ledger: live_cameras -= 1，orphaned_handles += 1
        Camera-->>Caller: CleanupError(CallOrder)
    else 可执行 native teardown
        Camera->>Slots: 所有 slot stop_accepting()
        Camera->>Slots: blocking drain 已进入的 callback
        alt 当前位于 exception callback
            Note over Camera,Native: 按厂商断线重连示例，仅执行 close + destroy
        else 普通调用上下文
            opt acquisition 为 Callback、Polling 或 Unknown
                Camera->>Native: MV_CC_StopGrabbing(handle)
            end
            opt image callback 为 Registered 或 Unknown
                Camera->>Native: RegisterImageCallBackEx(handle, NULL, NULL)
            end
            opt exception callback 为 Registered 或 Unknown
                Camera->>Native: RegisterExceptionCallBack(handle, NULL, NULL)
            end
            loop 每个 Registered 或 Unknown event callback
                Camera->>Native: RegisterEventCallBackEx(handle, name, NULL, NULL)
            end
        end
        Camera->>Native: MV_CC_CloseDevice(handle)
        Camera->>Native: MV_CC_DestroyHandle(handle)
        alt DestroyHandle 成功
            Camera->>Slots: 释放 callback backing
            Camera->>Ledger: live_cameras -= 1
        else DestroyHandle 失败
            Camera->>Camera: 保留 callback backing，raw handle 不再重试
            Camera->>Ledger: live_cameras -= 1，orphaned_handles += 1
        end
        Camera-->>Caller: Ok 或按调用顺序聚合的 CleanupError
    end
```

清理会在单步失败后继续执行，尽量到达 `DestroyHandle`。一旦 native handle 的销毁结果
无法确认，wrapper 会保留 SDK 可能继续引用的内存，并阻止进程级 finalize，防止 callback
访问已释放内存。

