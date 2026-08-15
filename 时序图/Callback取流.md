# Callback 取流

```mermaid
sequenceDiagram
    autonumber
    actor App as 应用
    participant Sdk as Sdk owner
    participant Camera as Camera
    participant Slot as Arc callback slot
    participant Native as MVS SDK

    App->>Sdk: Sdk::init()
    Sdk->>Native: MV_CC_Initialize()
    App->>Sdk: enumerate_devices(layers)
    Sdk->>Native: MV_CC_EnumDevices(...)
    Native-->>Sdk: SDK-owned 临时列表
    Sdk-->>App: 深拷贝 DeviceList<'sdk>

    App->>Camera: DeviceInfo::open(mode, key)
    Camera->>Native: CreateHandle → OpenDevice
    Note over Camera,Native: MV_OK + NULL → MvsError::NullHandleAfterCreate；回滚失败 → MvsError::OpenRollback
    App->>Camera: register_image_callback(closure)
    Camera->>Slot: 保存 Arc(closure) 与 native Arc token
    Camera->>Native: RegisterImageCallBackEx2(trampoline, slot, true)
    App->>Camera: start_grabbing()
    Camera->>Native: MV_CC_StartGrabbing()
    Note over Camera,Native: 仅 MV_OK 提交 Camera 本地状态；失败返回原错误，不调用 Stop/NULL callback
    Note over Camera,Slot: 注册失败清空 closure；native Arc token 在 DestroyHandle 成功后回收

    loop 每帧
        Native-->>Slot: image_trampoline(MV_FRAME_OUT, slot, true)
        Slot->>Slot: 临时 clone slot 与 closure Arc
        Slot->>App: closure(&Frame)
        Note over Slot,App: Frame 仅在本次 callback 有效；跨调用使用 to_owned()
        opt callback 内请求停止、关闭或改注册
            App->>Camera: 生命周期变更
            Camera-->>App: 本地 MvsError::InvalidState(..)（close 通过 CleanupError 报告）
            Note over Camera,Native: 不调用 native；通过 channel 通知 owner 线程处理
        end
        App-->>Slot: 返回
    end

    App->>Camera: stop_grabbing()
    Camera->>Native: MV_CC_StopGrabbing()
    opt 显式注销
        App->>Camera: unregister_image_callback()
        Camera->>Native: RegisterImageCallBackEx2(NULL, NULL, true)
        Camera->>Slot: 清除 stored closure
        Note over Camera,Slot: 已在途 closure 由临时 Arc 保活，可短暂继续
    end

    App->>Camera: close()
    Camera->>Native: Stop → 注销 → Close → Destroy
    alt Destroy 成功
        Camera->>Slot: 回收 native Arc token
    else Destroy 失败
        Camera-->>App: CleanupError（保留 Destroy 错误）
    end
    App->>Sdk: shutdown()
    alt handle 已确认 Destroy
        Sdk->>Native: MV_CC_Finalize()
    else handle 仍为 live
        Sdk-->>App: MvsError::NativeHandlesLive，不调用 Finalize
    end
```

callback 只做通知或复制数据，停止、关闭和重连交给 `Camera` owner。callback 内的
生命周期变更以本地 `MvsError::InvalidState(..)` 拒绝，不会进入 native SDK；业务错误通过 channel
交给 owner。closure panic 在 trampoline 最外层截获并静默该 closure，不进入 `MvsResult`；
owner 注销后可重新注册，Arc 的析构不会跨越 FFI 边界。Camera 本地状态只在调用返回
`MV_OK` 后更新；Start、Stop、注册和注销失败时 owner 仍在，由调用方决定是否重试。
`close(self)` 与 `shutdown(self)` 都会消费 owner 并只尝试一次，错误只用于诊断和宿主策略。
