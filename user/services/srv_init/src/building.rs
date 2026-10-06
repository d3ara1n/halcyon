use libkernel::process;
use libkernel::shared::{
    call::SystemCallError,
    object::{Handle, Rights},
    proc::{
        ExecutionProfile, PROCESS_PAGE_SIZE, PROCESS_USER_TOP, ProcessCreateResult,
        ProcessMapFlags, ThreadCreateResult, ThreadStartContext,
    },
};

/// 入口页指令：`j .`（`0x0000006f`，JAL x0,0 自我跳转）。不用 wfi——
/// wfi 是特权指令，U-mode 执行会触发 illegal instruction 异常，污染
/// Running 后被 kill 收束场景的终因。
const SPIN_FOREVER: [u8; 4] = [0x6f, 0x00, 0x00, 0x00];

/// 手工 Building：入口页（自旋）+ 栈顶页 + 首线程。失败时自行收束 builder/control。
pub(crate) fn build_spin_building(
    job: Handle,
    control_rights: Rights,
) -> Result<ProcessCreateResult, SystemCallError> {
    let created = process::create(job, crate::root_memory_pool(), control_rights)?;
    let built = (|| {
        process::map(
            created.builder,
            0x1000,
            PROCESS_PAGE_SIZE,
            ProcessMapFlags::READ | ProcessMapFlags::EXECUTE,
        )?;
        process::bind_execution(created.builder, ExecutionProfile::Base64 as u32)?;
        process::write(created.builder, 0x1000, &SPIN_FOREVER)?;
        process::map(
            created.builder,
            PROCESS_USER_TOP - PROCESS_PAGE_SIZE,
            PROCESS_PAGE_SIZE,
            ProcessMapFlags::READ | ProcessMapFlags::WRITE,
        )?;
        let mut thread = ThreadCreateResult {
            tid: 0,
            reserved: 0,
            control: Handle::INVALID,
            start: Handle::INVALID,
        };
        process::thread_create(
            created.builder,
            &ThreadStartContext {
                entry: 0x1000,
                stack_pointer: PROCESS_USER_TOP as u64,
                arg1: 0,
                arg2: 0,
            },
            &mut thread,
        )
    })();
    match built {
        Ok(_) => Ok(created),
        Err(error) => {
            unsafe { process::abandon_to_completion(created) }?;
            Err(error)
        }
    }
}
