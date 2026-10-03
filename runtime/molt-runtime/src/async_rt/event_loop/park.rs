//! Loop-thread parking for the asyncio idle wait.
//!
//! Every event loop owns one parker, created with the loop (as CPython creates
//! a selector loop's self-pipe at construction) and released at close. A
//! parker carries a sticky wake token: `unpark` never blocks, and a token
//! posted before `park` blocks makes that park return at once. Whether to park
//! is decided under the loop registry lock (`EventLoopState::begin_park`); the
//! parker only blocks, holding no runtime lock, and native parks release the
//! GIL for the whole wait.
//!
//! On native targets a park by the registered main thread also publishes the
//! parker as the signal authority's wake route for exactly that park
//! (`signal_ext::AsyncWorkParkRoute`), so a delivery recorded on any thread — a
//! raw OS signal, simulated `PyErr_SetInterruptEx`, or C pending call — ends
//! the wait. Signal flags and the pending-call ring remain the work facts. The
//! route is withdrawn, and in-flight deliveries are waited out, before `park`
//! returns and before the GIL is reacquired.
//!
//! - Unix: a non-blocking self-pipe waited with `poll(2)`. `unpark` is one
//!   `write(2)` and is async-signal-safe.
//! - Windows: an auto-reset kernel event. Low-level publishers never acquire
//!   a Rust mutex, including C pending calls and simulated signal deliveries.
//! - Other native targets require an explicit nonblocking notification backend.
//! - wasm32: one guest thread. Nothing can publish loop work while it waits, so
//!   the park is the WASM I/O poller's host idle wait.

#[cfg(all(unix, not(target_arch = "wasm32")))]
mod imp {
    use std::io;
    use std::os::fd::{AsRawFd, RawFd};
    use std::sync::atomic::{AtomicI32, Ordering};
    use std::time::Duration;

    pub(crate) struct LoopParker {
        reader: os_pipe::PipeReader,
        writer: os_pipe::PipeWriter,
        /// errno of a token write that failed other than on a full pipe.
        /// Cannot happen while this parker owns both ends; if it ever does,
        /// the next park fails loudly instead of losing the wake silently.
        unpark_errno: AtomicI32,
    }

    impl LoopParker {
        pub(crate) fn new() -> io::Result<Self> {
            let (reader, writer) = os_pipe::pipe()?;
            set_nonblocking(reader.as_raw_fd())?;
            // Signal deliveries write this end: it must never block.
            set_nonblocking(writer.as_raw_fd())?;
            Ok(Self {
                reader,
                writer,
                unpark_errno: AtomicI32::new(0),
            })
        }

        /// Post the wake token. Async-signal-safe: one `write(2)` plus atomics.
        /// A full pipe already holds an unconsumed token.
        pub(crate) fn unpark(&self) {
            let token = 0u8;
            loop {
                let written = unsafe {
                    libc::write(
                        self.writer.as_raw_fd(),
                        (&token as *const u8).cast::<libc::c_void>(),
                        1,
                    )
                };
                if written >= 0 {
                    return;
                }
                match io::Error::last_os_error().raw_os_error() {
                    Some(libc::EINTR) => continue,
                    Some(code) if code == libc::EAGAIN || code == libc::EWOULDBLOCK => return,
                    code => {
                        let _ = self.unpark_errno.compare_exchange(
                            0,
                            code.unwrap_or(libc::EIO),
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        );
                        return;
                    }
                }
            }
        }

        /// Block until a token, `timeout`, or (for the signal owner) a recorded
        /// delivery. Returns without blocking when a delivery is already
        /// recorded. Callers re-evaluate their wait condition after every
        /// return, so an interrupted or spurious return is harmless.
        pub(crate) fn park(
            &self,
            py: &crate::PyToken<'_>,
            timeout: Option<Duration>,
            signal_owner: bool,
        ) -> io::Result<()> {
            let errno = self.unpark_errno.swap(0, Ordering::AcqRel);
            if errno != 0 {
                return Err(io::Error::from_raw_os_error(errno));
            }
            let route = if signal_owner {
                match crate::builtins::signal_ext::AsyncWorkParkRoute::publish(py, self) {
                    Some(route) => Some(route),
                    // A delivery is recorded: its handlers must run, not wait.
                    None => return Ok(()),
                }
            } else {
                None
            };
            let mut wake = libc::pollfd {
                fd: self.reader.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let _release = crate::GilReleaseGuard::suspend();
            let ready = unsafe { libc::poll(&mut wake, 1, poll_timeout_ms(timeout)) };
            let outcome = if ready < 0 {
                let err = io::Error::last_os_error();
                // A signal handler ran on this thread; the caller re-checks.
                if err.kind() == io::ErrorKind::Interrupted {
                    Ok(())
                } else {
                    Err(err)
                }
            } else if wake.revents & (libc::POLLERR | libc::POLLNVAL) != 0 {
                Err(io::Error::other("event loop wake pipe failed"))
            } else if ready > 0 {
                drain(self.reader.as_raw_fd())
            } else {
                Ok(())
            };
            // Withdraw the route and wait out in-flight deliveries while the
            // GIL is still released, so that wait never blocks Python threads.
            drop(route);
            outcome
        }
    }

    fn drain(fd: RawFd) -> io::Result<()> {
        let mut tokens = [0u8; 64];
        loop {
            let read =
                unsafe { libc::read(fd, tokens.as_mut_ptr().cast::<libc::c_void>(), tokens.len()) };
            if read > 0 {
                continue;
            }
            if read == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "event loop wake pipe closed",
                ));
            }
            let err = io::Error::last_os_error();
            match err.raw_os_error() {
                Some(libc::EINTR) => continue,
                Some(code) if code == libc::EAGAIN || code == libc::EWOULDBLOCK => return Ok(()),
                _ => return Err(err),
            }
        }
    }

    fn set_nonblocking(fd: RawFd) -> io::Result<()> {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Round up: waking before the deadline would only park again.
    fn poll_timeout_ms(timeout: Option<Duration>) -> libc::c_int {
        timeout.map_or(-1, |timeout| {
            timeout
                .as_nanos()
                .div_ceil(1_000_000)
                .min(libc::c_int::MAX as u128) as libc::c_int
        })
    }

    #[cfg(test)]
    impl LoopParker {
        pub(crate) fn token_pending(&self) -> bool {
            let mut probe = libc::pollfd {
                fd: self.reader.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            unsafe { libc::poll(&mut probe, 1, 0) == 1 }
        }

        /// Fill the pipe so every further token write fails with EAGAIN.
        pub(crate) fn fill_for_test(&self) {
            let chunk = [0u8; 4096];
            loop {
                let written = unsafe {
                    libc::write(
                        self.writer.as_raw_fd(),
                        chunk.as_ptr().cast::<libc::c_void>(),
                        chunk.len(),
                    )
                };
                if written < 0 {
                    return;
                }
            }
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::io;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;
    use windows_sys::Win32::Foundation::{GetLastError, SetLastError, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        CreateEventW, INFINITE, SetEvent, WaitForSingleObject,
    };

    pub(crate) struct LoopParker {
        event: OwnedHandle,
        unpark_error: AtomicU32,
    }

    impl LoopParker {
        pub(crate) fn new() -> io::Result<Self> {
            // Auto-reset preserves one sticky token, coalesces publishers,
            // and consumes the token atomically with releasing one waiter.
            let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
            if event.is_null() {
                return Err(io::Error::last_os_error());
            }
            Ok(Self {
                event: unsafe { OwnedHandle::from_raw_handle(event) },
                unpark_error: AtomicU32::new(0),
            })
        }

        /// Kernel notification only: no Rust mutex, allocation, or GIL. Safe
        /// against reentrant C publishers on a thread that is about to wait.
        pub(crate) fn unpark(&self) {
            unsafe {
                let saved = GetLastError();
                if SetEvent(self.event.as_raw_handle()) == 0 {
                    let _ = self.unpark_error.compare_exchange(
                        0,
                        GetLastError(),
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    );
                }
                SetLastError(saved);
            }
        }

        pub(crate) fn park(
            &self,
            py: &crate::PyToken<'_>,
            timeout: Option<Duration>,
            async_work_owner: bool,
        ) -> io::Result<()> {
            let error = self.unpark_error.swap(0, Ordering::AcqRel);
            if error != 0 {
                return Err(io::Error::from_raw_os_error(error as i32));
            }
            let route = if async_work_owner {
                match crate::builtins::signal_ext::AsyncWorkParkRoute::publish(py, self) {
                    Some(route) => Some(route),
                    None => return Ok(()),
                }
            } else {
                None
            };
            let timeout_ms = timeout.map_or(INFINITE, |timeout| {
                timeout
                    .as_nanos()
                    .div_ceil(1_000_000)
                    .min((INFINITE - 1) as u128) as u32
            });
            let _release = crate::GilReleaseGuard::suspend();
            let result = unsafe { WaitForSingleObject(self.event.as_raw_handle(), timeout_ms) };
            let outcome = match result {
                WAIT_OBJECT_0 | WAIT_TIMEOUT => Ok(()),
                _ => Err(io::Error::last_os_error()),
            };
            drop(route);
            outcome
        }
    }

    #[cfg(test)]
    impl LoopParker {
        pub(crate) fn token_pending(&self) -> bool {
            // A zero-duration wait consumes the auto-reset token; put it back
            // so this quiescent test observation does not change park state.
            if unsafe { WaitForSingleObject(self.event.as_raw_handle(), 0) } == WAIT_OBJECT_0 {
                self.unpark();
                true
            } else {
                false
            }
        }
    }
}

#[cfg(all(not(unix), not(windows), not(target_arch = "wasm32")))]
compile_error!("event-loop parking needs a nonblocking low-level notifier for this native target");

#[cfg(target_arch = "wasm32")]
mod imp {
    use std::io;
    use std::time::Duration;

    pub(crate) struct LoopParker;

    impl LoopParker {
        pub(crate) fn new() -> io::Result<Self> {
            Ok(Self)
        }

        /// The only guest thread is the one that parks: publishers run only
        /// while it is not waiting, and it re-reads the queues after the wait.
        pub(crate) fn unpark(&self) {}

        pub(crate) fn park(
            &self,
            py: &crate::PyToken<'_>,
            timeout: Option<Duration>,
            _signal_owner: bool,
        ) -> io::Result<()> {
            crate::runtime_state(py).io_poller().idle_wait(py, timeout);
            Ok(())
        }
    }
}

pub(crate) use imp::LoopParker;
