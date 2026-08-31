use crate::SUPERVISOR_RIGHTS;
use rinlib::shared::{
    call::SystemCallError,
    object::{Handle, Rights},
    proc::{
        PROCESS_PAGE_SIZE, PROCESS_USER_TOP, ProcessCreateResult, ProcessMapFlags,
        ThreadStartContext,
    },
};
use rinlib::{
    ipc::object::{close, duplicate},
    process,
};

/// 入口页指令：`j .`（`0x0000006f`，JAL x0,0 自我跳转）。不用 wfi——
/// wfi 是特权指令，U-mode 执行会触发 illegal instruction 异常，污染
/// Running 后被 kill 收束场景的终因。
const SPIN_FOREVER: [u8; 4] = [0x6f, 0x00, 0x00, 0x00];

/// 手工 Building：入口页（自旋）+ 栈顶页 + 首线程。失败时自行收束 builder/control。
pub(crate) fn build_spin_building(job: Handle) -> Result<ProcessCreateResult, SystemCallError> {
    let created = process::create(job, SUPERVISOR_RIGHTS)?;
    let built = (|| {
        let pool = duplicate(crate::root_memory_pool(), Rights::GRANT)?;
        if let Err(error) = process::bind_memory(created.builder, pool) {
            let _ = close(pool);
            return Err(error);
        }
        process::map(
            created.builder,
            0x1000,
            PROCESS_PAGE_SIZE,
            ProcessMapFlags::READ | ProcessMapFlags::EXECUTE,
        )?;
        process::write(created.builder, 0x1000, &SPIN_FOREVER)?;
        process::map(
            created.builder,
            PROCESS_USER_TOP - PROCESS_PAGE_SIZE,
            PROCESS_PAGE_SIZE,
            ProcessMapFlags::READ | ProcessMapFlags::WRITE,
        )?;
        process::attach(
            created.builder,
            &ThreadStartContext {
                entry: 0x1000,
                stack_pointer: PROCESS_USER_TOP as u64,
                arg1: 0,
                arg2: 0,
            },
        )
    })();
    match built {
        Ok(_) => Ok(created),
        Err(error) => {
            process::abandon_to_completion(created)?;
            Err(error)
        }
    }
}
