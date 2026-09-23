//! Bounded, allocation-free suspension. Re-enumerate after each suspension so
//! threads created during enumeration cannot escape the safe point. Concurrent
//! remote-thread injection by another process is outside this contract.
//! Closures must not allocate, panic, log, or acquire application/loader locks.
use crate::win;
use std::sync::Mutex;
static SERIALIZE: Mutex<()> = Mutex::new(());
const LIMIT: usize = 4096;
#[derive(Debug, Clone, Copy)]
struct Failure(&'static str, u32);
trait Backend {
    type Handle: Copy;
    fn next(&mut self, previous: Option<Self::Handle>) -> Result<Option<Self::Handle>, Failure>;
    fn id(&mut self, handle: Self::Handle) -> Result<u32, Failure>;
    fn suspend(&mut self, handle: Self::Handle) -> Result<(), Failure>;
    /// Whether the thread has begun exiting. Such a thread cannot be
    /// suspended (STATUS_THREAD_IS_TERMINATING) and runs no more game code.
    fn terminating(&mut self, handle: Self::Handle) -> bool;
    fn rip(&mut self, handle: Self::Handle) -> Result<usize, Failure>;
    fn resume(&mut self, handle: Self::Handle);
    fn close(&mut self, handle: Self::Handle);
}
struct Frozen<'a, B: Backend> {
    backend: &'a mut B,
    handles: Vec<B::Handle>,
    ids: Vec<u32>,
    ips: Vec<usize>,
}
impl<B: Backend> Drop for Frozen<'_, B> {
    fn drop(&mut self) {
        // Resume ALL peers before closing handles or dropping allocations.
        for &handle in &self.handles {
            self.backend.resume(handle);
        }
        for &handle in &self.handles {
            self.backend.close(handle);
        }
    }
}
fn freeze<B: Backend, R>(
    backend: &mut B,
    caller: u32,
    limit: usize,
    f: impl FnOnce(&[usize]) -> R,
) -> Result<R, Failure> {
    let mut frozen = Frozen {
        backend,
        handles: Vec::with_capacity(limit),
        ids: Vec::with_capacity(limit),
        ips: Vec::with_capacity(limit),
    };
    loop {
        let before = frozen.handles.len();
        let mut cursor = None;
        loop {
            let next = frozen.backend.next(cursor);
            if let Some(handle) = cursor {
                frozen.backend.close(handle);
            }
            let Some(handle) = next? else { break };
            cursor = Some(handle);
            let mut retained = false;
            let result = (|| {
                let id = frozen.backend.id(handle)?;
                if id == caller || frozen.ids.contains(&id) {
                    return Ok(false);
                }
                if frozen.handles.len() == limit {
                    return Err(Failure("thread limit exceeded", 0));
                }
                if let Err(error) = frozen.backend.suspend(handle) {
                    // An exiting thread is skipped, not a failed safe point:
                    // threads come and go at startup, and refusing here makes a
                    // plugin's patch fail at random.
                    return if frozen.backend.terminating(handle) {
                        Ok(false)
                    } else {
                        Err(error)
                    };
                }
                frozen.handles.push(handle);
                frozen.ids.push(id);
                retained = true;
                // Context retrieval waits for suspension to take effect.
                frozen.ips.push(frozen.backend.rip(handle)?);
                Ok(true)
            })();
            match result {
                // Keep the retained handle open and restart enumeration.
                Ok(true) => break,
                Ok(false) => {}
                Err(error) => {
                    if !retained {
                        frozen.backend.close(handle);
                    }
                    return Err(error);
                }
            }
        }
        if frozen.handles.len() == before {
            return Ok(f(&frozen.ips));
        }
    }
}
struct Native;
impl Backend for Native {
    type Handle = win::Handle;
    fn next(&mut self, previous: Option<win::Handle>) -> Result<Option<win::Handle>, Failure> {
        let mut next = core::ptr::null_mut();
        let status = unsafe {
            win::NtGetNextThread(
                win::GetCurrentProcess(),
                previous.unwrap_or(core::ptr::null_mut()),
                win::THREAD_SUSPEND_RESUME
                    | win::THREAD_GET_CONTEXT
                    | win::THREAD_QUERY_INFORMATION,
                0,
                0,
                &mut next,
            )
        };
        match status as u32 {
            0 => Ok(Some(next)),
            0x8000001a => Ok(None), // STATUS_NO_MORE_ENTRIES
            error => Err(Failure("enumerating threads", error)),
        }
    }
    fn id(&mut self, handle: win::Handle) -> Result<u32, Failure> {
        let id = unsafe { win::GetThreadId(handle) };
        if id == 0 {
            Err(Failure("identifying thread", unsafe {
                win::GetLastError()
            }))
        } else {
            Ok(id)
        }
    }
    fn suspend(&mut self, handle: win::Handle) -> Result<(), Failure> {
        if unsafe { win::SuspendThread(handle) } == u32::MAX {
            Err(Failure("suspending thread", unsafe { win::GetLastError() }))
        } else {
            Ok(())
        }
    }
    fn terminating(&mut self, handle: win::Handle) -> bool {
        const THREAD_IS_TERMINATED: u32 = 20;
        let mut flag = 0u32;
        let status = unsafe {
            win::NtQueryInformationThread(
                handle,
                THREAD_IS_TERMINATED,
                (&mut flag as *mut u32).cast(),
                4,
                core::ptr::null_mut(),
            )
        };
        status == 0 && flag != 0
    }
    fn rip(&mut self, handle: win::Handle) -> Result<usize, Failure> {
        let mut context = crate::crash::Context([0; 1232]);
        context.0[48..52].copy_from_slice(&0x0010_0001u32.to_le_bytes());
        if unsafe { win::GetThreadContext(handle, context.0.as_mut_ptr().cast()) } == 0 {
            return Err(Failure("reading thread context", unsafe {
                win::GetLastError()
            }));
        }
        Ok(u64::from_le_bytes(context.0[248..256].try_into().unwrap()) as usize)
    }
    fn resume(&mut self, handle: win::Handle) {
        if unsafe { win::ResumeThread(handle) } == u32::MAX {
            // Continuing could allocate behind a permanently suspended peer.
            unsafe {
                win::TerminateProcess(win::GetCurrentProcess(), 0xc0000001);
            }
        }
    }
    fn close(&mut self, handle: win::Handle) {
        unsafe {
            win::CloseHandle(handle);
        }
    }
}
pub fn suspend_all<R>(f: impl FnOnce(&[usize]) -> R) -> Result<R, String> {
    let _serialize = SERIALIZE.lock().unwrap_or_else(|p| p.into_inner());
    let outcome = freeze(&mut Native, unsafe { win::GetCurrentThreadId() }, LIMIT, f);
    // All peers resumed before formatting errors.
    outcome
        .map_err(|Failure(operation, code)| format!("safe point: {operation} failed ({code:#x})"))
}
pub fn stop_the_world<R>(f: impl FnOnce(&[usize]) -> R) -> Result<R, String> {
    #[cfg(test)]
    {
        Ok(f(&[]))
    }
    #[cfg(not(test))]
    {
        suspend_all(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Fake {
        born: bool,
        fail: Option<&'static str>,
        exiting: bool,
        stopped: [bool; 4],
        resumed: usize,
    }
    impl Backend for Fake {
        type Handle = u32;
        fn next(&mut self, previous: Option<u32>) -> Result<Option<u32>, Failure> {
            if self.fail == Some("enumeration") && self.stopped[2] {
                return Err(Failure("enumeration", 5));
            }
            let next = previous.unwrap_or(0) + 1;
            Ok((next <= if self.born { 3 } else { 2 }).then_some(next))
        }
        fn id(&mut self, h: u32) -> Result<u32, Failure> {
            Ok(h)
        }
        fn suspend(&mut self, h: u32) -> Result<(), Failure> {
            if (self.fail == Some("suspend") || self.exiting) && h == 3 {
                return Err(Failure("suspend", 5));
            }
            assert!(!self.stopped[h as usize], "suspended the same thread twice");
            self.stopped[h as usize] = true;
            self.born = true; // A worker was created as the initial snapshot finished.
            Ok(())
        }
        fn terminating(&mut self, h: u32) -> bool {
            self.exiting && h == 3
        }
        fn rip(&mut self, h: u32) -> Result<usize, Failure> {
            if self.fail == Some("context") && h == 3 {
                Err(Failure("context", 5))
            } else {
                Ok(h as usize * 100)
            }
        }
        fn resume(&mut self, h: u32) {
            assert!(self.stopped[h as usize]);
            self.stopped[h as usize] = false;
            self.resumed += 1;
        }
        fn close(&mut self, _: u32) {}
    }
    #[test]
    fn a_thread_created_during_enumeration_is_also_suspended() {
        let mut fake = Fake::default();
        let result = freeze(&mut fake, 1, 8, |ips| {
            assert_eq!(ips, &[200, 300]);
            42
        });
        assert_eq!(result.unwrap(), 42);
        assert_eq!(fake.resumed, 2);
        assert!(!fake.stopped.iter().any(|&v| v));
    }
    #[test]
    fn an_exiting_thread_is_skipped_rather_than_failing_the_safe_point() {
        let mut fake = Fake {
            exiting: true,
            ..Default::default()
        };
        let result = freeze(&mut fake, 1, 8, |ips| {
            assert_eq!(ips, &[200]);
            7
        });
        assert_eq!(result.unwrap(), 7);
        assert!(!fake.stopped.iter().any(|&v| v));
    }
    #[test]
    fn every_failure_resumes_peers_without_running_the_writer() {
        for failure in ["context", "suspend", "enumeration", "limit"] {
            let mut fake = Fake {
                fail: Some(failure),
                ..Default::default()
            };
            let result = freeze(&mut fake, 1, if failure == "limit" { 1 } else { 8 }, |_| {
                panic!("must refuse the write")
            });
            assert!(result.is_err(), "{failure}");
            assert!(!fake.stopped.iter().any(|&v| v), "{failure}");
            assert!(fake.resumed > 0);
        }
    }
    #[test]
    fn native_safe_point_runs_in_a_bounded_child_process() {
        if std::env::var_os("DEFIANCE_FREEZE_CHILD").is_some() {
            let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let c = count.clone();
            let s = stop.clone();
            let worker = std::thread::spawn(move || {
                while !s.load(std::sync::atomic::Ordering::Relaxed) {
                    c.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            });
            while count.load(std::sync::atomic::Ordering::Relaxed) == 0 {
                std::thread::yield_now();
            }
            let result = suspend_all(|_| {
                let first = count.load(std::sync::atomic::Ordering::Relaxed);
                unsafe {
                    win::Sleep(20);
                }
                (first, count.load(std::sync::atomic::Ordering::Relaxed))
            });
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            worker.join().unwrap();
            let (before, after) = result.unwrap();
            assert_eq!(before, after);
            return;
        }
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "threads::tests::native_safe_point_runs_in_a_bounded_child_process",
            ])
            .env("DEFIANCE_FREEZE_CHILD", "1")
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            if std::time::Instant::now() > deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("safe point timed out");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}
