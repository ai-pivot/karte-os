//! P2.1 L1 能力描述层（CapDesc）— MCP Device Fabric
//!
//! 每个内核驱动/设备声明一组能力描述符（CapDesc），系统据此自动生成
//! MCP tool 定义（name / description / inputSchema / outputSchema /
//! 权限位），使设备出厂即带工具，无需人工编写 MCP 适配层。
//!
//! ABI（ROADMAP 附录 A v1）：
//!   CapDesc { device_id, device_type, version, tools: &[ToolDesc] }
//!   ToolDesc { name, desc, inputs: &[FieldDesc], outputs: &[FieldDesc], perm }
//!   FieldDesc { name, ty: Ty, desc, required }
//!   Ty: Bool/I32/I64/F64/Str/Bytes/Json
//!   perm: bit0=EXEC bit1=READ bit2=WRITE bit3=CONFIG（保留至 P2.4）

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

/// 字段类型（wire：u8 序号；JSON schema：kind 映射）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ty {
    Bool,
    I32,
    I64,
    F64,
    Str,
    Bytes,
    Json,
}

impl Ty {
    pub fn json_kind(self) -> &'static str {
        match self {
            Ty::Bool => "boolean",
            Ty::I32 | Ty::I64 => "integer",
            Ty::F64 => "number",
            Ty::Str => "string",
            Ty::Bytes => "string", // base64
            Ty::Json => "object",
        }
    }
}

/// 工具入/出参字段
#[derive(Clone)]
pub struct FieldDesc {
    pub name: &'static str,
    pub ty: Ty,
    pub desc: &'static str,
    pub required: bool,
}

/// 权限位（P2.4 安全层消费）
pub mod perm {
    pub const EXEC: u8 = 1 << 0;
    pub const READ: u8 = 1 << 1;
    pub const WRITE: u8 = 1 << 2;
    pub const CONFIG: u8 = 1 << 3;
}

/// 单个工具描述
#[derive(Clone)]
pub struct ToolDesc {
    pub name: &'static str,
    pub desc: &'static str,
    pub inputs: &'static [FieldDesc],
    pub outputs: &'static [FieldDesc],
    pub perm: u8,
}

/// 设备能力描述符（附录 A）
#[derive(Clone)]
pub struct CapDesc {
    pub device_id: &'static str,   // 稳定唯一 id（如 "vfs0", "gpio0"）
    pub device_type: &'static str, // "vfs" | "timer" | "gpio" | ...
    pub version: u32,
    pub tools: &'static [ToolDesc],
}

// ── 注册表（L2 DRT 的种子数据） ──

struct Registry {
    devices: Vec<(CapDesc, bool)>, // bool = online（心跳由 P2.2 DRT 维护）
}

static REGISTRY: Mutex<Registry> = Mutex::new(Registry {
    devices: Vec::new(),
});

/// 注册设备能力（幂等：同 device_id 重复注册返回 Err）
pub fn register_device(desc: CapDesc) -> Result<(), &'static str> {
    let mut reg = REGISTRY.lock();
    if reg
        .devices
        .iter()
        .any(|(d, _)| d.device_id == desc.device_id)
    {
        return Err("device already registered");
    }
    reg.devices.push((desc, true));
    Ok(())
}

/// 设备上线/离线（P2.2 DRT 状态机的内核侧入口）
pub fn set_online(device_id: &str, online: bool) -> bool {
    let mut reg = REGISTRY.lock();
    for (d, o) in reg.devices.iter_mut() {
        if d.device_id == device_id {
            *o = online;
            return true;
        }
    }
    false
}

/// 在线设备总数
pub fn online_devices() -> usize {
    REGISTRY.lock().devices.iter().filter(|(_, o)| *o).count()
}

/// 按 device_id 查找能力描述符（DRT wire 分发用）
pub fn lookup_desc(device_id: &str) -> Option<CapDesc> {
    REGISTRY
        .lock()
        .devices
        .iter()
        .find(|(d, _)| d.device_id == device_id)
        .map(|(d, _)| d.clone())
}

/// 已注册设备总数
pub fn device_count() -> usize {
    REGISTRY.lock().devices.len()
}

/// 全部工具名（脑端工具表种子；稳定排序保证快照可比）
pub fn tool_names() -> Vec<String> {
    let reg = REGISTRY.lock();
    let mut v: Vec<String> = reg
        .devices
        .iter()
        .flat_map(|(d, _)| {
            d.tools
                .iter()
                .map(|t| alloc::format!("{}_{}", d.device_type, t.name))
        })
        .collect();
    v.sort();
    v
}

/// CapDesc → MCP tool JSON（name/description/inputSchema/outputSchema）
/// 生成器是 P2.1 的验收核心：snapshot 测试锁定输出。
pub fn tool_json(device_type: &str, tool: &ToolDesc) -> String {
    let mut s = String::new();
    s.push_str("{\"name\":\"");
    s.push_str(device_type);
    s.push('_');
    s.push_str(tool.name);
    s.push_str("\",\"description\":\"");
    s.push_str(tool.desc);
    s.push_str("\",\"inputSchema\":{\"type\":\"object\",\"properties\":{");
    write_fields(&mut s, tool.inputs);
    s.push_str("},\"required\":[");
    write_required(&mut s, tool.inputs);
    s.push_str("]},\"outputSchema\":{\"type\":\"object\",\"properties\":{");
    write_fields(&mut s, tool.outputs);
    s.push_str("}},\"perm\":");
    s.push_str(&alloc::format!("{}", tool.perm));
    s.push('}');
    s
}

fn write_fields(s: &mut String, fields: &'static [FieldDesc]) {
    for (i, f) in fields.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push('"');
        s.push_str(f.name);
        s.push_str("\":{\"type\":\"");
        s.push_str(f.ty.json_kind());
        s.push_str("\",\"description\":\"");
        s.push_str(f.desc);
        s.push_str("\"}");
    }
}

fn write_required(s: &mut String, fields: &'static [FieldDesc]) {
    let mut first = true;
    for f in fields.iter().filter(|f| f.required) {
        if !first {
            s.push(',');
        }
        first = false;
        s.push('"');
        s.push_str(f.name);
        s.push('"');
    }
}

// ── 示范设备注册（VFS / 定时器 / 虚拟 GPIO，验收：每类 ≥2 工具） ──

static VFS_TOOLS: [ToolDesc; 3] = [
    ToolDesc {
        name: "read",
        desc: "read bytes from a file",
        inputs: &[
            FieldDesc {
                name: "path",
                ty: Ty::Str,
                desc: "absolute path",
                required: true,
            },
            FieldDesc {
                name: "len",
                ty: Ty::I32,
                desc: "max bytes",
                required: false,
            },
        ],
        outputs: &[FieldDesc {
            name: "data",
            ty: Ty::Bytes,
            desc: "file bytes",
            required: true,
        }],
        perm: perm::READ | perm::EXEC,
    },
    ToolDesc {
        name: "write",
        desc: "write bytes to a file (create if missing)",
        inputs: &[
            FieldDesc {
                name: "path",
                ty: Ty::Str,
                desc: "absolute path",
                required: true,
            },
            FieldDesc {
                name: "data",
                ty: Ty::Bytes,
                desc: "bytes to write",
                required: true,
            },
        ],
        outputs: &[FieldDesc {
            name: "written",
            ty: Ty::I32,
            desc: "bytes written",
            required: true,
        }],
        perm: perm::WRITE,
    },
    ToolDesc {
        name: "ls",
        desc: "list directory entries",
        inputs: &[FieldDesc {
            name: "path",
            ty: Ty::Str,
            desc: "directory path",
            required: false,
        }],
        outputs: &[FieldDesc {
            name: "entries",
            ty: Ty::Json,
            desc: "name list",
            required: true,
        }],
        perm: perm::READ,
    },
];

static TIMER_TOOLS: [ToolDesc; 2] = [
    ToolDesc {
        name: "sleep_until",
        desc: "yield until wallclock reaches unix_ms",
        inputs: &[FieldDesc {
            name: "unix_ms",
            ty: Ty::I64,
            desc: "target time",
            required: true,
        }],
        outputs: &[FieldDesc {
            name: "woke",
            ty: Ty::Bool,
            desc: "always true on return",
            required: true,
        }],
        perm: perm::EXEC,
    },
    ToolDesc {
        name: "sleep_ms",
        desc: "relative delay in milliseconds",
        inputs: &[FieldDesc {
            name: "ms",
            ty: Ty::I32,
            desc: "delay",
            required: true,
        }],
        outputs: &[FieldDesc {
            name: "woke",
            ty: Ty::Bool,
            desc: "always true on return",
            required: true,
        }],
        perm: perm::EXEC,
    },
];

static GPIO_TOOLS: [ToolDesc; 2] = [
    ToolDesc {
        name: "write",
        desc: "set virtual gpio level",
        inputs: &[
            FieldDesc {
                name: "pin",
                ty: Ty::I32,
                desc: "pin number",
                required: true,
            },
            FieldDesc {
                name: "value",
                ty: Ty::Bool,
                desc: "level",
                required: true,
            },
        ],
        outputs: &[],
        perm: perm::WRITE,
    },
    ToolDesc {
        name: "read",
        desc: "read virtual gpio level",
        inputs: &[FieldDesc {
            name: "pin",
            ty: Ty::I32,
            desc: "pin number",
            required: true,
        }],
        outputs: &[FieldDesc {
            name: "value",
            ty: Ty::Bool,
            desc: "level",
            required: true,
        }],
        perm: perm::READ,
    },
];

/// 注册三类示范设备（boot 时调用；P2.2 DRT 起来后改为驱动自注册）
pub fn register_builtin_devices() {
    let _ = register_device(CapDesc {
        device_id: "vfs0",
        device_type: "vfs",
        version: 1,
        tools: &VFS_TOOLS,
    });
    let _ = register_device(CapDesc {
        device_id: "timer0",
        device_type: "timer",
        version: 1,
        tools: &TIMER_TOOLS,
    });
    let _ = register_device(CapDesc {
        device_id: "gpio0",
        device_type: "gpio",
        version: 1,
        tools: &GPIO_TOOLS,
    });
}

#[cfg(feature = "test_mode")]
pub fn run_tests() {
    crate::console_println!("");
    crate::console_println!("── Capability (CapDesc) Tests ──");

    crate::test::run_test("capdesc_register_builtin", || {
        register_builtin_devices();
        device_count() >= 3 && online_devices() == 3
    });

    crate::test::run_test("capdesc_register_duplicate_fails", || {
        let dup = CapDesc {
            device_id: "vfs0",
            device_type: "vfs",
            version: 1,
            tools: &VFS_TOOLS,
        };
        register_device(dup).is_err()
    });

    crate::test::run_test("capdesc_offline_online_transition", || {
        set_online("gpio0", false)
            && online_devices() == 2
            && set_online("gpio0", true)
            && online_devices() == 3
    });

    crate::test::run_test("capdesc_offline_unknown_noop", || {
        !set_online("nope", false)
    });

    crate::test::run_test("capdesc_tool_names_sorted", || {
        let names = tool_names();
        names.len() == 7
            && names[0] == "gpio_read"
            && names[1] == "gpio_write"
            && names.contains(&alloc::format!("vfs_ls"))
            && names.contains(&alloc::format!("timer_sleep_ms"))
    });

    // snapshot：tool JSON 生成器输出锁定（schema 字段/类型/required/perm）
    crate::test::run_test("capdesc_tool_json_snapshot_gpio_read", || {
        let j = tool_json("gpio", &GPIO_TOOLS[1]);
        j == "{\"name\":\"gpio_read\",\"description\":\"read virtual gpio level\",\"inputSchema\":{\"type\":\"object\",\"properties\":{\"pin\":{\"type\":\"integer\",\"description\":\"pin number\"}},\"required\":[\"pin\"]},\"outputSchema\":{\"type\":\"object\",\"properties\":{\"value\":{\"type\":\"boolean\",\"description\":\"level\"}}},\"perm\":2}"
    });

    crate::test::run_test("capdesc_tool_json_snapshot_vfs_read", || {
        let j = tool_json("vfs", &VFS_TOOLS[0]);
        // 两个 input：path required、len optional；perm = READ|EXEC = 3
        j.contains("\"name\":\"vfs_read\"")
            && j.contains("\"path\":{\"type\":\"string\",\"description\":\"absolute path\"}")
            && j.contains("\"len\":{\"type\":\"integer\"")
            && j.contains("\"required\":[\"path\"]")
            && j.ends_with("\"perm\":3}")
    });

    crate::test::run_test("capdesc_tool_json_optional_only_inputs", || {
        // timer_sleep_ms：唯一 input required=true → required 数组非空
        let j = tool_json("timer", &TIMER_TOOLS[1]);
        j.contains("\"required\":[\"ms\"]") && j.contains("\"woke\":{\"type\":\"boolean\"")
    });
}
