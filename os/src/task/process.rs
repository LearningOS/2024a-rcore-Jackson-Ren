//! Implementation of  [`ProcessControlBlock`]

use super::id::RecycleAllocator;
use super::manager::insert_into_pid2process;
use super::TaskControlBlock;
use super::{add_task, SignalFlags};
use super::{pid_alloc, PidHandle};
use crate::fs::{File, Stdin, Stdout};
use crate::mm::{translated_refmut, MemorySet, KERNEL_SPACE};
use crate::sync::{Condvar, Mutex, Semaphore, UPSafeCell};
use crate::trap::{trap_handler, TrapContext};
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefMut;

/// Process Control Block
pub struct ProcessControlBlock {
    /// immutable
    pub pid: PidHandle,
    /// mutable
    inner: UPSafeCell<ProcessControlBlockInner>,
}

/// Inner of Process Control Block
pub struct ProcessControlBlockInner {
    /// is zombie?
    pub is_zombie: bool,
    /// memory set(address space)
    pub memory_set: MemorySet,
    /// parent process
    pub parent: Option<Weak<ProcessControlBlock>>,
    /// children process
    pub children: Vec<Arc<ProcessControlBlock>>,
    /// exit code
    pub exit_code: i32,
    /// file descriptor table
    pub fd_table: Vec<Option<Arc<dyn File + Send + Sync>>>,
    /// signal flags
    pub signals: SignalFlags,
    /// tasks(also known as threads)
    pub tasks: Vec<Option<Arc<TaskControlBlock>>>,
    /// task resource allocator
    pub task_res_allocator: RecycleAllocator,
    /// mutex list
    pub mutex_list: Vec<Option<Arc<dyn Mutex>>>,
    /// semaphore list
    pub semaphore_list: Vec<Option<Arc<Semaphore>>>,
    /// condvar list
    pub condvar_list: Vec<Option<Arc<Condvar>>>,
    /// enable deadlock check
    pub dead_lock_check: bool,

    /// resource available list
    pub mutex_resource_list: Vec<isize>,
    /// need list
    pub mutex_need_list: Vec<Vec<isize /*count*/>>,
    /// allocation list
    pub mutex_alloc_list: Vec<Vec<isize /*count*/>>,

    /// resource available list
    pub sem_resource_list: Vec<isize>,
    /// need list
    pub sem_need_list: Vec<Vec<isize /*count*/>>,
    /// allocation list
    pub sem_alloc_list: Vec<Vec<isize /*count*/>>,
}

impl ProcessControlBlockInner {
    #[allow(unused)]
    /// get the address of app's page table
    pub fn get_user_token(&self) -> usize {
        self.memory_set.token()
    }
    /// allocate a new file descriptor
    pub fn alloc_fd(&mut self) -> usize {
        if let Some(fd) = (0..self.fd_table.len()).find(|fd| self.fd_table[*fd].is_none()) {
            fd
        } else {
            self.fd_table.push(None);
            self.fd_table.len() - 1
        }
    }
    /// allocate a new task id
    pub fn alloc_tid(&mut self) -> usize {
        self.task_res_allocator.alloc()
    }
    /// deallocate a task id
    pub fn dealloc_tid(&mut self, tid: usize) {
        self.task_res_allocator.dealloc(tid)
    }
    /// the count of tasks(threads) in this process
    pub fn thread_count(&self) -> usize {
        self.tasks.len()
    }
    /// get a task with tid in this process
    pub fn get_task(&self, tid: usize) -> Arc<TaskControlBlock> {
        self.tasks[tid].as_ref().unwrap().clone()
    }
    /// alloc deadlock check graph
    pub fn alloc_dead_lock_graph(&mut self) {
        self.mutex_need_list.push(Vec::new());
        self.mutex_alloc_list.push(Vec::new());
        self.sem_need_list.push(Vec::new());
        self.sem_alloc_list.push(Vec::new());
    }
    /// alloc deadlock check graph
    pub fn alloc_dead_lock_graph_add_task(&mut self, tid: usize) {
        let mutex_n = self.mutex_need_list[0].len();
        let sem_n = self.sem_need_list[0].len();

        while self.mutex_need_list.len() < tid + 1 {
            let i = self.mutex_need_list.len();
            self.mutex_need_list.push(Vec::new());
            while self.mutex_need_list[i].len() < mutex_n {
                self.mutex_need_list[i].push(0);
            }
            self.mutex_alloc_list.push(Vec::new());
            while self.mutex_alloc_list[i].len() < mutex_n {
                self.mutex_alloc_list[i].push(0);
            }
            self.sem_need_list.push(Vec::new());
            while self.sem_need_list[i].len() < sem_n {
                self.sem_need_list[i].push(0);
            }
            self.sem_alloc_list.push(Vec::new());
            while self.sem_alloc_list[i].len() < sem_n {
                self.sem_alloc_list[i].push(0);
            }
        }
    }
    /// alloc deadlock check graph
    pub fn alloc_dead_lock_graph_add_sem(&mut self, sem_id: usize, count: isize) {
        self.sem_need_list.iter_mut().for_each(|vec| {
            while vec.len() < sem_id + 1 {
                vec.push(0);
            }
        });
        self.sem_alloc_list.iter_mut().for_each(|vec| {
            while vec.len() < sem_id + 1 {
                vec.push(0);
            }
        });
        while self.sem_resource_list.len() < sem_id + 1 {
            self.sem_resource_list.push(0);
        }
        self.sem_resource_list[sem_id] = count;
    }

    /// alloc deadlock check graph
    pub fn alloc_dead_lock_graph_add_mutex(&mut self, mutex_id: usize, count: isize) {
        self.mutex_need_list.iter_mut().for_each(|vec| {
            while vec.len() < mutex_id + 1 {
                vec.push(0);
            }
        });
        self.mutex_alloc_list.iter_mut().for_each(|vec| {
            while vec.len() < mutex_id + 1 {
                vec.push(0);
            }
        });
        while self.mutex_resource_list.len() < mutex_id + 1 {
            self.mutex_resource_list.push(0);
        }
        self.mutex_resource_list[mutex_id] = count;
    }
    /// mutex lock
    pub fn dead_lock_graph_lock_mutex(&mut self, tid: usize, mutex_id: usize) -> isize {
        self.mutex_need_list[tid][mutex_id] += 1;
        let ret = self.check_dead_lock_mutex();
        self.mutex_resource_list[mutex_id] -= 1;
        self.mutex_need_list[tid][mutex_id] -= 1;
        self.mutex_alloc_list[tid][mutex_id] += 1;
        ret
    }
    /// mutex unlock
    pub fn dead_lock_graph_unlock_mutex(&mut self, tid: usize, mutex_id: usize) {
        self.mutex_resource_list[mutex_id] += 1;
        self.mutex_alloc_list[tid][mutex_id] -= 1;
    }
    /// sem down
    pub fn dead_lock_graph_down_sem(&mut self, tid: usize, sem_id: usize) -> isize {
        self.sem_need_list[tid][sem_id] += 1;
        let ret = self.check_dead_lock();
        self.sem_need_list[tid][sem_id] -= 1;

        ret
    }
    /// get sem
    pub fn get_sem_resource(&mut self, tid: usize, sem_id: usize) {
        self.sem_resource_list[sem_id] -= 1;
        self.sem_alloc_list[tid][sem_id] += 1;
    }
    /// release sem
    pub fn release_sem_resource(&mut self, tid: usize, sem_id: usize) {
        self.sem_resource_list[sem_id] += 1;
        self.sem_alloc_list[tid][sem_id] -= 1;
    }
    /// check deadlock if set enabled
    pub fn check_dead_lock(&mut self) -> isize {
        if !self.dead_lock_check {
            // Dont need to check
            return 0;
        }
        let mut finish_list = vec![false; self.sem_need_list.len()];
        let mut work = self.sem_resource_list.clone();

        let n = finish_list.len();
        let m = work.len();
        loop {
            let mut finish_count = 0;
            for i in 0..n {
                if finish_list[i] {
                    continue;
                }

                let mut check_finish = true;
                for j in 0..m {
                    // println!("len:{}", self.sem_need_list[i].len());
                    // println!(
                    //     "check uid:{}, sem:{},need:{},work:{}",
                    //     i, j, self.sem_need_list[i][j], work[j]
                    // );
                    if self.sem_need_list[i][j] > work[j] {
                        check_finish = false;
                        break;
                    }
                }
                // println!("check uid:{}, status:{}", i, check_finish);
                if check_finish {
                    for j in 0..m {
                        work[j] += self.sem_alloc_list[i][j];
                    }
                    finish_list[i] = true;
                    finish_count += 1;
                }
            }

            if finish_list.iter().fold(true, |res, &status| res & status) {
                // not dead lock
                return 0;
            }

            if finish_count == 0 {
                // check dead lock
                return -0xDEAD;
            }
        }
    }

    pub fn check_dead_lock_mutex(&mut self) -> isize {
        if !self.dead_lock_check {
            // Dont need to check
            return 0;
        }
        let mut finish_list = vec![false; self.mutex_need_list.len()];
        let mut work = self.mutex_resource_list.clone();

        let n = finish_list.len();
        let m = work.len();
        loop {
            let mut finish_count = 0;
            for i in 0..n {
                if finish_list[i] {
                    continue;
                }

                let mut check_finish = true;
                for j in 0..m {
                    // println!(
                    //     "check uid:{}, mutex:{},new:{},work:{}",
                    //     i, j, self.mutex_need_list[i][j], work[j]
                    // );
                    if self.mutex_need_list[i][j] > work[j] {
                        check_finish = false;
                        break;
                    }
                }
                if check_finish {
                    for j in 0..m {
                        work[j] += self.mutex_alloc_list[i][j];
                    }
                    finish_list[i] = true;
                    finish_count += 1;
                }
            }

            if finish_list.iter().fold(true, |res, &status| res & status) {
                // not dead lock
                return 0;
            }

            if finish_count == 0 {
                // check dead lock
                return -0xDEAD;
            }
        }
    }
}

impl ProcessControlBlock {
    /// inner_exclusive_access
    pub fn inner_exclusive_access(&self) -> RefMut<'_, ProcessControlBlockInner> {
        self.inner.exclusive_access()
    }
    /// new process from elf file
    pub fn new(elf_data: &[u8]) -> Arc<Self> {
        trace!("kernel: ProcessControlBlock::new");
        // memory_set with elf program headers/trampoline/trap context/user stack
        let (memory_set, ustack_base, entry_point) = MemorySet::from_elf(elf_data);
        // allocate a pid
        let pid_handle = pid_alloc();
        let process = Arc::new(Self {
            pid: pid_handle,
            inner: unsafe {
                UPSafeCell::new(ProcessControlBlockInner {
                    is_zombie: false,
                    memory_set,
                    parent: None,
                    children: Vec::new(),
                    exit_code: 0,
                    fd_table: vec![
                        // 0 -> stdin
                        Some(Arc::new(Stdin)),
                        // 1 -> stdout
                        Some(Arc::new(Stdout)),
                        // 2 -> stderr
                        Some(Arc::new(Stdout)),
                    ],
                    signals: SignalFlags::empty(),
                    tasks: Vec::new(),
                    task_res_allocator: RecycleAllocator::new(),
                    mutex_list: Vec::new(),
                    semaphore_list: Vec::new(),
                    condvar_list: Vec::new(),
                    dead_lock_check: false,
                    mutex_resource_list: Vec::new(),
                    mutex_need_list: Vec::new(),
                    mutex_alloc_list: Vec::new(),
                    sem_resource_list: Vec::new(),
                    sem_need_list: Vec::new(),
                    sem_alloc_list: Vec::new(),
                })
            },
        });
        // create a main thread, we should allocate ustack and trap_cx here
        let task = Arc::new(TaskControlBlock::new(
            Arc::clone(&process),
            ustack_base,
            true,
        ));
        // prepare trap_cx of main thread
        let task_inner = task.inner_exclusive_access();
        let trap_cx = task_inner.get_trap_cx();
        let ustack_top = task_inner.res.as_ref().unwrap().ustack_top();
        let kstack_top = task.kstack.get_top();
        drop(task_inner);
        *trap_cx = TrapContext::app_init_context(
            entry_point,
            ustack_top,
            KERNEL_SPACE.exclusive_access().token(),
            kstack_top,
            trap_handler as usize,
        );
        // add main thread to the process
        let mut process_inner = process.inner_exclusive_access();
        process_inner.tasks.push(Some(Arc::clone(&task)));
        process_inner.alloc_dead_lock_graph();
        drop(process_inner);
        insert_into_pid2process(process.getpid(), Arc::clone(&process));
        // add main thread to scheduler
        add_task(task);
        process
    }

    /// Only support processes with a single thread.
    pub fn exec(self: &Arc<Self>, elf_data: &[u8], args: Vec<String>) {
        trace!("kernel: exec");
        assert_eq!(self.inner_exclusive_access().thread_count(), 1);
        // memory_set with elf program headers/trampoline/trap context/user stack
        trace!("kernel: exec .. MemorySet::from_elf");
        let (memory_set, ustack_base, entry_point) = MemorySet::from_elf(elf_data);
        let new_token = memory_set.token();
        // substitute memory_set
        trace!("kernel: exec .. substitute memory_set");
        self.inner_exclusive_access().memory_set = memory_set;
        // then we alloc user resource for main thread again
        // since memory_set has been changed
        trace!("kernel: exec .. alloc user resource for main thread again");
        let task = self.inner_exclusive_access().get_task(0);
        let mut task_inner = task.inner_exclusive_access();
        task_inner.res.as_mut().unwrap().ustack_base = ustack_base;
        task_inner.res.as_mut().unwrap().alloc_user_res();
        task_inner.trap_cx_ppn = task_inner.res.as_mut().unwrap().trap_cx_ppn();
        // push arguments on user stack
        trace!("kernel: exec .. push arguments on user stack");
        let mut user_sp = task_inner.res.as_mut().unwrap().ustack_top();
        user_sp -= (args.len() + 1) * core::mem::size_of::<usize>();
        let argv_base = user_sp;
        let mut argv: Vec<_> = (0..=args.len())
            .map(|arg| {
                translated_refmut(
                    new_token,
                    (argv_base + arg * core::mem::size_of::<usize>()) as *mut usize,
                )
            })
            .collect();
        *argv[args.len()] = 0;
        for i in 0..args.len() {
            user_sp -= args[i].len() + 1;
            *argv[i] = user_sp;
            let mut p = user_sp;
            for c in args[i].as_bytes() {
                *translated_refmut(new_token, p as *mut u8) = *c;
                p += 1;
            }
            *translated_refmut(new_token, p as *mut u8) = 0;
        }
        // make the user_sp aligned to 8B for k210 platform
        user_sp -= user_sp % core::mem::size_of::<usize>();
        // initialize trap_cx
        trace!("kernel: exec .. initialize trap_cx");
        let mut trap_cx = TrapContext::app_init_context(
            entry_point,
            user_sp,
            KERNEL_SPACE.exclusive_access().token(),
            task.kstack.get_top(),
            trap_handler as usize,
        );
        trap_cx.x[10] = args.len();
        trap_cx.x[11] = argv_base;
        *task_inner.get_trap_cx() = trap_cx;
    }

    /// Only support processes with a single thread.
    pub fn fork(self: &Arc<Self>) -> Arc<Self> {
        trace!("kernel: fork");
        let mut parent = self.inner_exclusive_access();
        assert_eq!(parent.thread_count(), 1);
        // clone parent's memory_set completely including trampoline/ustacks/trap_cxs
        let memory_set = MemorySet::from_existed_user(&parent.memory_set);
        // alloc a pid
        let pid = pid_alloc();
        // copy fd table
        let mut new_fd_table: Vec<Option<Arc<dyn File + Send + Sync>>> = Vec::new();
        for fd in parent.fd_table.iter() {
            if let Some(file) = fd {
                new_fd_table.push(Some(file.clone()));
            } else {
                new_fd_table.push(None);
            }
        }
        // create child process pcb
        let child = Arc::new(Self {
            pid,
            inner: unsafe {
                UPSafeCell::new(ProcessControlBlockInner {
                    is_zombie: false,
                    memory_set,
                    parent: Some(Arc::downgrade(self)),
                    children: Vec::new(),
                    exit_code: 0,
                    fd_table: new_fd_table,
                    signals: SignalFlags::empty(),
                    tasks: Vec::new(),
                    task_res_allocator: RecycleAllocator::new(),
                    mutex_list: Vec::new(),
                    semaphore_list: Vec::new(),
                    condvar_list: Vec::new(),
                    dead_lock_check: false,
                    mutex_resource_list: Vec::new(),
                    mutex_need_list: Vec::new(),
                    mutex_alloc_list: Vec::new(),
                    sem_resource_list: Vec::new(),
                    sem_need_list: Vec::new(),
                    sem_alloc_list: Vec::new(),
                })
            },
        });
        // add child
        parent.children.push(Arc::clone(&child));
        // create main thread of child process
        let task = Arc::new(TaskControlBlock::new(
            Arc::clone(&child),
            parent
                .get_task(0)
                .inner_exclusive_access()
                .res
                .as_ref()
                .unwrap()
                .ustack_base(),
            // here we do not allocate trap_cx or ustack again
            // but mention that we allocate a new kstack here
            false,
        ));
        // attach task to child process
        let mut child_inner = child.inner_exclusive_access();
        child_inner.tasks.push(Some(Arc::clone(&task)));
        child_inner.alloc_dead_lock_graph();
        drop(child_inner);
        // modify kstack_top in trap_cx of this thread
        let task_inner = task.inner_exclusive_access();
        let trap_cx = task_inner.get_trap_cx();
        trap_cx.kernel_sp = task.kstack.get_top();
        drop(task_inner);
        insert_into_pid2process(child.getpid(), Arc::clone(&child));
        // add this thread to scheduler
        add_task(task);
        child
    }
    /// get pid
    pub fn getpid(&self) -> usize {
        self.pid.0
    }
}
