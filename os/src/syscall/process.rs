//! Process management syscalls

use crate::{
    config::MAX_SYSCALL_NUM,
    mm::save_program_data,
    task::{
        change_program_brk, current_task_info, current_task_mmap, current_task_munmap,
        current_user_token, exit_current_and_run_next, suspend_current_and_run_next, TaskStatus,
    },
    timer::{get_time_ms, get_time_us},
};
use core::{mem::size_of, slice::from_raw_parts};

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

/// Task information
#[allow(dead_code)]
pub struct TaskInfo {
    /// Task status in it's life cycle
    status: TaskStatus,
    /// The numbers of syscall called by task
    syscall_times: [u32; MAX_SYSCALL_NUM],
    /// Total running time of task
    time: usize,
}

/// task exits and submit an exit code
pub fn sys_exit(_exit_code: i32) -> ! {
    trace!("kernel: sys_exit");
    exit_current_and_run_next();
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
}

/// YOUR JOB: get time with second and microsecond
/// HINT: You might reimplement it with virtual memory management.
/// HINT: What if [`TimeVal`] is splitted by two pages ?
pub fn sys_get_time(_ts: *mut TimeVal, _tz: usize) -> isize {
    let us = get_time_us();
    let time_val = TimeVal {
        sec: us / 1_000_000,
        usec: us % 1_000_000,
    };
    let data: &[u8] = unsafe {
        from_raw_parts(
            (&time_val as *const TimeVal) as *const u8,
            size_of::<TimeVal>(),
        )
    };
    save_program_data(current_user_token(), _ts as *const u8, data);

    0
}

/// YOUR JOB: Finish sys_task_info to pass testcases
/// HINT: You might reimplement it with virtual memory management.
/// HINT: What if [`TaskInfo`] is splitted by two pages ?
pub fn sys_task_info(_ti: *mut TaskInfo) -> isize {
    let task_info = {
        let tmp = current_task_info();
        TaskInfo {
            status: tmp.0,
            syscall_times: tmp.1,
            time: get_time_ms() - tmp.2,
        }
    };

    let data: &[u8] = unsafe {
        from_raw_parts(
            (&task_info as *const TaskInfo) as *const u8,
            size_of::<TaskInfo>(),
        )
    };
    save_program_data(current_user_token(), _ti as *const u8, data);

    0
}

// YOUR JOB: Implement mmap.
pub fn sys_mmap(_start: usize, _len: usize, _port: usize) -> isize {
    // start 没有按页大小对齐

    // port & !0x7 != 0 (port 其余位必须为0) port & 0x7 = 0 (这样的内存无意义)

    // [start, start + len) 中存在已经被映射的页

    // 物理内存不足
    current_task_mmap(_start, _len, _port)
}

// YOUR JOB: Implement munmap.
pub fn sys_munmap(_start: usize, _len: usize) -> isize {
    // [start, start + len) 中存在未被映射的虚存。
    trace!("kernel: sys_munmap NOT IMPLEMENTED YET!");
    current_task_munmap(_start, _len)
}

/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel: sys_sbrk");
    if let Some(old_brk) = change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}
