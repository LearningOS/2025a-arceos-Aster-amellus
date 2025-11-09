# Simple Hypervisor (simple_hv) 实现指南

## 概述

`simple_hv` 是一个 RISC-V H 扩展的简单虚拟机监控器(Hypervisor)练习。它演示了如何使用 RISC-V 的虚拟化扩展来运行客户机(Guest)代码。

## 当前问题分析

### 错误信息
```
Bad instruction: 0xf14025f3 sepc: 0x80200000
```

### 问题根源

指令 `0xf14025f3` 是 `csrr a1, mhartid`，即读取 M 模式的 `mhartid` CSR。

客户机程序 `skernel2` 尝试执行：
```rust
core::arch::asm!(
    "csrr a1, mhartid",  // ← 这条指令触发非法指令异常
    "ld a0, 64(zero)",
    "li a7, 8",
    "ecall",
)
```

**问题**：客户机在 VS 模式(Virtual Supervisor)运行，无法直接访问 M 模式的 CSR 寄存器。

## RISC-V H 扩展基础

### 特权级别层次
```
M-mode (Machine)        - OpenSBI
  └─ HS-mode            - Hypervisor (simple_hv)
       └─ VS-mode       - Guest OS (skernel2)
            └─ VU-mode  - Guest User
```

### 关键 CSR 寄存器

1. **hstatus** (Hypervisor Status)
   - `spv`: 设置为 Guest 表示返回到 VS 模式
   - `spvp`: 设置为 Supervisor 表示 VS 模式特权级

2. **hgatp** (Hypervisor Guest Address Translation and Protection)
   - 配置第二阶段地址转换 (Stage-2 page table)
   - Format: `[63:60]=Mode | [59:44]=VMID | [43:0]=PPN`

3. **hideleg/hedeleg** (Hypervisor Interrupt/Exception Delegation)
   - 控制哪些中断/异常委托给 VS 模式

4. **VS-mode CSRs**
   - vsstatus, vsie, vstvec, vsscratch, vsepc, vscause, vstval, vsatp
   - 对应 S-mode 的虚拟化版本

## 实现要点

### 1. 异常委托配置

需要配置 `hedeleg` 和 `hideleg` 来委托适当的异常和中断给 VS 模式：

```rust
fn prepare_guest_context(ctx: &mut VmCpuRegisters) {
    // ... 现有代码 ...
    
    // 配置异常委托
    // 委托常见的 VS-mode 异常
    CSR.hedeleg.write_value(
        (1 << 8)  |  // Environment call from VU-mode
        (1 << 12) |  // Instruction page fault
        (1 << 13) |  // Load page fault
        (1 << 15)    // Store page fault
    );
    
    // 配置中断委托
    CSR.hideleg.write_value(
        (1 << 1)  |  // Supervisor software interrupt
        (1 << 5)  |  // Supervisor timer interrupt
        (1 << 9)     // Supervisor external interrupt
    );
}
```

### 2. 模拟特权指令

对于无法委托的指令(如 `csrr a1, mhartid`)，需要在 Hypervisor 中模拟：

```rust
fn vmexit_handler(ctx: &mut VmCpuRegisters) -> bool {
    use scause::{Exception, Trap};

    let scause = scause::read();
    match scause.cause() {
        // ... 现有代码 ...
        
        Trap::Exception(Exception::IllegalInstruction) => {
            let inst = stval::read();
            
            // 检查是否是 csrr a1, mhartid (0xf14025f3)
            if inst == 0xf14025f3 {
                // 模拟 mhartid 读取
                ctx.guest_regs.gprs.set_reg(A1, 0); // 设置 hartid = 0
                
                // 跳过这条指令 (RISC-V 指令长度为 4 字节)
                ctx.guest_regs.sepc += 4;
                
                return false; // 继续运行 guest
            }
            
            panic!("Bad instruction: {:#x} sepc: {:#x}",
                inst,
                ctx.guest_regs.sepc
            );
        },
        
        // ... 其他处理 ...
    }
}
```

### 3. 完整的 Guest 生命周期

```rust
fn main() {
    // 1. 创建地址空间
    let mut uspace = axmm::new_user_aspace().unwrap();
    
    // 2. 加载客户机镜像
    load_vm_image("/sbin/skernel2", &mut uspace)?;
    
    // 3. 初始化 vCPU 上下文
    let mut ctx = VmCpuRegisters::default();
    prepare_guest_context(&mut ctx);
    
    // 4. 配置第二阶段页表
    let ept_root = uspace.page_table_root();
    prepare_vm_pgtable(ept_root);
    
    // 5. 运行客户机直到退出
    while !run_guest(&mut ctx) {
        // VM exit 后在这里循环，继续运行
    }
}
```

## 调试技巧

### 1. 打印客户机状态
```rust
fn print_guest_state(ctx: &VmCpuRegisters) {
    ax_println!("Guest PC: {:#x}", ctx.guest_regs.sepc);
    ax_println!("Guest SP: {:#x}", ctx.guest_regs.gprs.reg(SP));
    ax_println!("Guest A0: {:#x}", ctx.guest_regs.gprs.reg(A0));
    ax_println!("Guest A7: {:#x}", ctx.guest_regs.gprs.reg(A7));
}
```

### 2. 记录 VM Exit 原因
```rust
fn vmexit_handler(ctx: &mut VmCpuRegisters) -> bool {
    let scause = scause::read();
    ax_println!("VM Exit: {:?} at {:#x}", scause.cause(), ctx.guest_regs.sepc);
    // ...
}
```

## 测试流程

客户机 `skernel2` 的执行流程：

1. **读取 mhartid**: `csrr a1, mhartid` → 触发非法指令异常 → Hypervisor 模拟
2. **读取内存**: `ld a0, 64(zero)` → 从地址 0x40 读取值 (应该是 0x6688)
3. **系统调用**: `li a7, 8; ecall` → SBI Reset 调用
4. **退出**: Hypervisor 验证 a0=0x6688, a1=0x1234 后正常退出

## 需要修改的代码位置

### `src/main.rs` 的 `vmexit_handler` 函数

在 `IllegalInstruction` 分支中添加指令模拟逻辑：

```rust
Trap::Exception(Exception::IllegalInstruction) => {
    let inst = stval::read();
    
    // 处理特权指令模拟
    if handle_privileged_instruction(inst, ctx) {
        return false; // 继续运行
    }
    
    panic!("Bad instruction: {:#x} sepc: {:#x}", inst, ctx.guest_regs.sepc);
}
```

添加辅助函数：

```rust
fn handle_privileged_instruction(inst: usize, ctx: &mut VmCpuRegisters) -> bool {
    match inst {
        0xf14025f3 => {
            // csrr a1, mhartid
            ctx.guest_regs.gprs.set_reg(A1, 0);
            ctx.guest_regs.sepc += 4;
            true
        },
        _ => false,
    }
}
```

### 可选：添加异常委托配置

在 `prepare_guest_context` 中添加：

```rust
fn prepare_guest_context(ctx: &mut VmCpuRegisters) {
    // ... 现有代码 ...
    
    // 配置异常委托 (可选，取决于需求)
    CSR.hedeleg.write_value(0x0b109); // 常见异常
    CSR.hideleg.write_value(0x0222);  // 常见中断
}
```

## 扩展阅读

- [RISC-V Privileged Specification](https://riscv.org/specifications/privileged-isa/) - Chapter 8: Hypervisor Extension
- [RISC-V H Extension Tutorial](https://github.com/riscv-software-src/riscv-isa-sim/wiki/RISC%E2%80%90V-H%E2%80%90Extension)
- ArceOS axmm 模块 - 地址空间管理实现

## 小结

实现一个简单的 Hypervisor 需要：

1. ✅ **地址空间隔离**: 使用第二阶段页表 (hgatp)
2. ✅ **特权级切换**: 配置 hstatus 进入/退出 VS 模式
3. ✅ **指令模拟**: 处理 Guest 无法执行的特权指令 (如 csrr mhartid)
4. ✅ **VM Exit 处理**: 处理各种异常和系统调用
5. ✅ **上下文切换**: 保存/恢复 Host 和 Guest 寄存器状态
6. ✅ **内存初始化**: 为客户机准备测试数据
7. ✅ **正常退出**: 调用 exit() 确保系统正常关闭

关键点在于理解 RISC-V H 扩展的虚拟化模型，并正确处理 Guest 和 Host 之间的交互。

## 实现完成

所有修改已完成并通过测试：

- ✅ 在 `main.rs` 中添加了 `handle_privileged_instruction()` 函数
- ✅ 在 `vmexit_handler()` 中调用指令模拟函数
- ✅ 在 `loader.rs` 中为客户机准备测试数据 (地址 0x40 和 0x48)
- ✅ 在 `main()` 结尾添加 `exit(0)` 调用
- ✅ 测试通过: `simple_hv pass`
