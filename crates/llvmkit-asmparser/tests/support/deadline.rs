//! A deadline for tests that pin a resource bound: a cost that grew with a
//! declared size (lanes, bits) or quadratically with the input fails here
//! instead of running until CI's job limit.

/// Runs `body` and fails if it has not finished within a minute: it exits the
/// test binary, naming the test and `cost` (what a regression would be doing),
/// so a walk proportional to a declared size stops there. The bounded paths
/// answer in well under a second, so the minute is a wide margin against a
/// loaded machine; a walk that allocates nothing would otherwise run until
/// CI's job limit rather than fail here.
///
/// Exiting drops the other tests' results in this binary. That is the trade:
/// a worker thread cannot be stopped, and one left walking (or allocating)
/// behind a failed test would take the binary, or the machine, down later
/// with no failure attributed to it. The message names the test.
///
/// A deadline does not see memory. Where a test can assert a size or a count
/// it should, and keep this as the backstop.
pub fn within_a_minute(cost: &'static str, body: impl FnOnce() + Send + 'static) {
    let (done, finished) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        body();
        // The receiver is gone only if the deadline passed first.
        done.send(()).ok();
    });
    match finished.recv_timeout(std::time::Duration::from_secs(60)) {
        Ok(()) => {}
        // The worker cannot be stopped, and a walk that allocates would go on
        // allocating while the rest of the binary runs, so the binary exits
        // here, naming the test, rather than fail the test and keep it.
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            // Straight to the process's stderr: the test harness captures
            // `eprintln!`, and the exit below would drop what it captured.
            use std::io::Write as _;
            let _ = writeln!(
                std::io::stderr(),
                "{}: still running after a minute: {cost}",
                std::thread::current().name().unwrap_or("deadline")
            );
            std::process::exit(1);
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            if let Err(panic) = worker.join() {
                std::panic::resume_unwind(panic);
            }
        }
    }
}
