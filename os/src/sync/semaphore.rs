//! Semaphore

use crate::sync::UPSafeCell;
use crate::task::{
    block_current_and_run_next, current_process, current_task, wakeup_task, TaskControlBlock,
};
use alloc::{collections::VecDeque, sync::Arc};

/// semaphore structure
pub struct Semaphore {
    /// semaphore inner
    pub inner: UPSafeCell<SemaphoreInner>,
}

pub struct SemaphoreInner {
    pub count: isize,
    pub wait_queue: VecDeque<Arc<TaskControlBlock>>,
    pub sem_id: usize,
}

impl Semaphore {
    /// Create a new semaphore
    pub fn new(res_count: usize, sem_id: usize) -> Self {
        trace!("kernel: Semaphore::new");
        Self {
            inner: unsafe {
                UPSafeCell::new(SemaphoreInner {
                    count: res_count as isize,
                    wait_queue: VecDeque::new(),
                    sem_id,
                })
            },
        }
    }

    /// up operation of semaphore
    pub fn up(&self, tid: usize) {
        trace!("kernel: Semaphore::up");
        let mut inner = self.inner.exclusive_access();
        inner.count += 1;
        let sem_id = inner.sem_id;
        let mut task = None;
        if inner.count <= 0 {
            task = inner.wait_queue.pop_front();
        }
        drop(inner);

        current_process()
            .inner_exclusive_access()
            .release_sem_resource(tid, sem_id);

        if let Some(task) = task {
            let next_tid = task.inner_exclusive_access().res.as_ref().unwrap().tid;
            current_process()
                .inner_exclusive_access()
                .get_sem_resource(next_tid, sem_id);
            current_process().inner_exclusive_access().sem_need_list[next_tid][sem_id] -= 1;
            wakeup_task(task);
        }
    }

    /// down operation of semaphore
    pub fn down(&self, tid: usize) {
        trace!("kernel: Semaphore::down");
        let mut inner = self.inner.exclusive_access();
        inner.count -= 1;
        let sem_id = inner.sem_id;
        if inner.count < 0 {
            inner.wait_queue.push_back(current_task().unwrap());
            drop(inner);
            current_process().inner_exclusive_access().sem_need_list[tid][sem_id] += 1;
            block_current_and_run_next();
        } else {
            drop(inner);
            current_process()
                .inner_exclusive_access()
                .get_sem_resource(tid, sem_id);
        }
    }
}
