use anyhow::{Context as _, Result};
use std::{
    any::Any,
    fmt::Debug,
    marker::PhantomData,
    ops::{Deref, DerefMut, Index, IndexMut},
    thread::{self, JoinHandle},
};

use crossbeam_channel::{Receiver, Sender};

use crate::ResultExt;

#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct UnsafeRefMut<T>(*mut T);

unsafe impl<T> Send for UnsafeRefMut<T> {}
unsafe impl<T> Sync for UnsafeRefMut<T> {}

impl<T> UnsafeRefMut<T> {
    pub fn new(v: &mut T) -> Self {
        Self(v as _)
    }

    pub fn as_mut(&self) -> &mut T {
        unsafe { self.0.as_mut_unchecked() }
    }
}

impl<T> Deref for UnsafeRefMut<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { self.0.as_ref_unchecked() }
    }
}

impl<T> DerefMut for UnsafeRefMut<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.0.as_mut_unchecked() }
    }
}

impl<T: Debug> std::fmt::Debug for UnsafeRefMut<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        <T as Debug>::fmt(self, f)
    }
}

impl<Idx, T: Index<Idx>> Index<Idx> for UnsafeRefMut<T> {
    type Output = T::Output;

    fn index(&self, index: Idx) -> &Self::Output {
        <T as Index<Idx>>::index(self, index)
    }
}

impl<Idx, T: IndexMut<Idx>> IndexMut<Idx> for UnsafeRefMut<T> {
    fn index_mut(&mut self, index: Idx) -> &mut Self::Output {
        <T as IndexMut<Idx>>::index_mut(self, index)
    }
}

pub struct Worker {
    thread: JoinHandle<()>,
    tx: Sender<Box<dyn Fn() -> Box<dyn Any + Send> + Send>>,
    rx: Receiver<Box<dyn Any + Send>>,
}

impl Worker {
    pub fn new() -> Self {
        let (work_tx, work_rx) =
            crossbeam_channel::bounded::<Box<dyn Fn() -> Box<dyn Any + Send> + Send>>(1);
        let (result_tx, result_rx) = crossbeam_channel::bounded::<Box<dyn Any + Send>>(1);
        let handle = thread::spawn(move || loop {
            let Ok(work) = work_rx.recv() else {
                continue;
            };
            let res = work();
            _ = result_tx.send(res);
        });
        Self {
            thread: handle,
            tx: work_tx,
            rx: result_rx,
        }
    }

    pub fn exec<T: Any + Send>(&mut self, work: impl 'static + Send + Fn() -> T) -> T {
        let work = Box::new(move || {
            let res = work();
            Box::new(res) as Box<dyn Any + Send>
        });
        self.tx.send(work).into_log();
        let res = self.rx.recv().log_assert();
        *res.downcast::<T>().unwrap()
    }

    pub fn exec_drop<T: Any + Send>(
        &mut self,
        work: impl 'static + Send + Fn() -> T,
    ) -> JobHandle<T> {
        let work = Box::new(move || {
            let res = work();
            Box::new(res) as Box<dyn Any + Send>
        });
        self.tx.send(work).into_log();
        JobHandle {
            rx: self.rx.clone(),
            _data: PhantomData {},
        }
    }
}

pub struct JobHandle<T: Any + Send> {
    rx: Receiver<Box<dyn Any + Send>>,
    _data: PhantomData<T>,
}

impl<T: Any + Send> JobHandle<T> {
    fn join(self) -> T {
        let res = self.rx.recv().log_assert();
        *res.downcast::<T>().unwrap()
    }
}

struct WorkerPool {
    workers: Vec<Worker>,
    job_handles: Vec<JobHandle<()>>,
    current_worker: usize,
}

impl WorkerPool {
    pub fn new(workers: usize) -> Self {
        let job_handles = Vec::with_capacity(workers);
        let workers = (0..workers).map(|_| Worker::new()).collect::<Vec<_>>();
        Self {
            workers,
            job_handles,
            current_worker: 0,
        }
    }

    pub fn resize(&mut self, new_size: usize) {
        self.job_handles = Vec::with_capacity(new_size);
        self.workers = (0..new_size).map(|_| Worker::new()).collect::<Vec<_>>();
        self.current_worker = 0;
    }

    pub fn exec(&mut self, work: impl 'static + Send + Fn()) {
        self.job_handles
            .push(self.workers[self.current_worker].exec_drop(work));
        self.current_worker += 1;
    }

    pub fn join_all(&mut self) {
        for handle in std::mem::take(&mut self.job_handles) {
            handle.join();
        }
        self.current_worker = 0;
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn worker() {
        let mut worker = Worker::new();
        let mut vec = Vec::new();
        {
            let vec = UnsafeRefMut::new(&mut vec);
            worker.exec(move || {
                for i in 0..100 {
                    vec.as_mut().push(i);
                }
            });
        }
        assert!(vec.len() == 100);
    }

    #[test]
    fn workers() {
        let mut workers = [Worker::new(), Worker::new(), Worker::new(), Worker::new()];
        let mut handles = Vec::new();
        let mut vec = vec![0; 40];
        for (i, worker) in workers.iter_mut().enumerate() {
            let vec = UnsafeRefMut::new(&mut vec);
            handles.push(worker.exec_drop(move || {
                let start = 10 * i;
                for i in start..start + 10 {
                    vec.as_mut()[i] = i;
                }
            }));
        }
        for handle in handles {
            handle.join();
        }
        assert_eq!(vec.len(), 40);
    }

    #[test]
    fn pool() {
        let mut pool = WorkerPool::new(10);
        let mut vec = vec![0; 100];
        for i in 0..10 {
            let vec = UnsafeRefMut::new(&mut vec);
            pool.exec(move || {
                let start = 10 * i;
                for i in start..start + 10 {
                    vec.as_mut()[i] = i;
                }
            });
        }
        pool.join_all();
        println!("vec = {vec:?}");
        assert_eq!(vec.len(), 101);
    }
}
