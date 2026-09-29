use super::super::{
    configuration,
    coordination::Permit,
    installed::Phase as BodyPhase,
    startup::{StartupDecision, StartupReady},
};
use super::*;

pub(super) fn run(control: &Path, recovering: bool, rescue: bool) -> Result<(), AppError> {
    let mut state = load(control)?;
    let (source, _installer_bytes, _runner_bytes) = verify(control, &state)?;
    let _stage_bytes = source.stage.lock()?;
    let _permit = Permit::update()?;
    if let Some(previous) = &state.runner {
        if ProcessFence::probe(previous)?.is_some() {
            return Err(error());
        }
    }
    let parent = if rescue {
        None
    } else {
        Some(ProcessFence::open(
            &state.parent,
            &state.request.root.join("Rela.exe"),
        )?)
    };
    state.runner = Some(ProcessFence::capture(
        std::process::id(),
        &control.join("runner.exe"),
    )?);
    state.request.controller = state.runner.clone().ok_or_else(error)?;
    save(control, &state)?;
    if let Some(parent) = parent {
        let mut channel = Channel::connect(&state.handoff)?;
        channel.send(&Handoff::Ready)?;
        parent.wait(Duration::from_secs(60))?;
    }
    let mut runtime = Runtime {
        control: control.into(),
        state,
        source,
        lease: None,
        config: None,
        worker: None,
        child: None,
        candidate_channel: None,
        candidate_guard: None,
    };
    if recovering {
        runtime.wait_previous()?;
    } else if runtime.state.phase != Phase::Waiting {
        return Err(error());
    }
    runtime.acquire_lease()?;
    let result = if recovering {
        runtime.recover()
    } else {
        runtime.execute()
    };
    if let Err(failure) = result {
        runtime.state.failure = Some(failure.message.clone());
        runtime.save()?;
        runtime.quiesce()?;
        if let Some(mut worker) = runtime.worker.take() {
            worker.close_and_wait()?;
        }
        if recovering {
            return Err(failure);
        }
        // Reopening the privileged record is the only way to settle a lost
        // commit acknowledgement. It may roll forward; never assume rollback.
        runtime.wait_previous()?;
        runtime.recover()?;
    }
    Ok(())
}

struct Runtime {
    control: PathBuf,
    state: Control,
    source: VerifiedSource,
    lease: Option<InstanceLease>,
    config: Option<configuration::Transaction>,
    worker: Option<worker::Client>,
    child: Option<Child>,
    candidate_channel: Option<Channel>,
    candidate_guard: Option<ProgramGuard>,
}
impl Runtime {
    fn save(&self) -> Result<(), AppError> {
        save(&self.control, &self.state)
    }
    fn acquire_lease(&mut self) -> Result<(), AppError> {
        if self.lease.is_none() {
            self.lease = Some(InstanceLease::acquire(&self.state.config)?);
        }
        Ok(())
    }
    fn wait_previous(&self) -> Result<(), AppError> {
        for ticket in self
            .state
            .launcher
            .iter()
            .chain(self.state.hook.iter())
            .chain(self.state.installer.iter())
            .chain(self.state.candidate.iter())
        {
            if let Some(process) = ProcessFence::probe(ticket)? {
                process.wait(Duration::from_secs(180))?;
            }
        }
        Ok(())
    }
    fn execute(&mut self) -> Result<(), AppError> {
        self.state.phase = Phase::ConfigPreparing;
        self.save()?;
        self.config = Some(configuration::Transaction::prepare(
            &self.state.config,
            &self.state.request.id,
            self.lease.as_ref().ok_or_else(error)?,
        )?);
        self.state.phase = Phase::ConfigPrepared;
        self.save()?;
        let pre = Listener::bind(&self.state.request.id, Role::InstallerPre)?;
        let post = Listener::bind(&self.state.request.id, Role::InstallerPost)?;
        let launcher = Listener::bind(&self.state.request.id, Role::InstallerLauncher)?;
        self.state.request.pre = pre.endpoint();
        self.state.request.post = post.endpoint();
        self.state.request.recovery = None;
        self.state.launcher_endpoint = Some(launcher.endpoint());
        self.state.phase = Phase::Launching;
        self.save()?;
        nsis_worker::write_request(&self.control, &self.state.request)?;
        let executable = self.control.join("runner.exe");
        let mut child = spawn(
            &executable,
            "--rela-installed-launch",
            &self.state.request.id,
        )?;
        self.state.launcher = Some(ProcessFence::capture(child.id(), &executable)?);
        self.save()?;
        let mut channel =
            launcher.accept(child.id(), &executable, Duration::from_secs(60), || {
                child.try_wait().is_ok_and(|s| s.is_some())
            })?;
        if !matches!(
            channel.receive::<launcher::Response>()?,
            launcher::Response::Ready
        ) {
            return Err(error());
        }
        channel.send(&launcher::Decision::Launch)?;
        // Explicit launch failure (including UAC cancellation) can stop the PRE
        // wait immediately. EOF alone is not failure: successful install exits
        // the launcher before NSIS reports its exact process identity.
        let failed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let launch_failed = std::sync::Arc::clone(&failed);
        std::thread::spawn(move || {
            if matches!(
                channel.receive::<launcher::Response>(),
                Ok(launcher::Response::Failed)
            ) {
                launch_failed.store(true, std::sync::atomic::Ordering::Release);
            }
        });
        // Success is PRE/POST authentication, never the official launcher's exit.
        let mut worker =
            worker::Client::hook(&pre, &self.control, &mut self.state, &self.source, || {
                failed.load(std::sync::atomic::Ordering::Acquire)
            })?;
        worker.pre()?;
        self.worker = Some(worker::Client::hook(
            &post,
            &self.control,
            &mut self.state,
            &self.source,
            || false,
        )?);
        self.worker.as_mut().ok_or_else(error)?.post()?;
        self.start_candidate()?;
        self.worker.as_mut().ok_or_else(error)?.resolve(true)?;
        self.finish(true)
    }
    fn recover(&mut self) -> Result<(), AppError> {
        self.acquire_lease()?;
        if self.state.phase.terminal() && self.state.machine_done {
            return self.launch_terminal();
        }
        if !self.state.phase.machine_may_have_started() {
            // PRE Prepare was never authorized. Any late old installer is bound
            // to a dead controller, and switching the request to recovery closes
            // the PRE entry before resolving untouched user configuration.
            let endpoint = Listener::bind(&self.state.request.id, Role::InstallerRecovery)?;
            self.state.request.recovery = Some(endpoint.endpoint());
            nsis_worker::write_request(&self.control, &self.state.request)?;
            if guard::pending_update(&self.state.request.root)?
                || fingerprint(&self.state.request.root.join("Rela.exe"), MAX_PROGRAM)?.sha256
                    != self.state.request.runner_digest
            {
                return Err(error());
            }
            match self.state.phase {
                Phase::Waiting => {}
                Phase::ConfigPreparing => match configuration::Transaction::reopen(
                    &self.state.config,
                    &self.state.request.id,
                ) {
                    Ok(transaction) => self.config = Some(transaction),
                    Err(_) => configuration::retain_incomplete(
                        &self.state.config,
                        &self.state.request.id,
                        self.lease.as_ref().ok_or_else(error)?,
                    )?,
                },
                _ => {
                    self.config = Some(configuration::Transaction::reopen(
                        &self.state.config,
                        &self.state.request.id,
                    )?)
                }
            }
            return self.finish(false);
        }
        self.config = Some(configuration::Transaction::reopen(
            &self.state.config,
            &self.state.request.id,
        )?);
        let (worker, phase) =
            worker::Client::recover(&self.control, &mut self.state, &self.source)?;
        self.worker = Some(worker);
        let commit = matches!(phase, Some(BodyPhase::Committing | BodyPhase::Committed));
        if phase.is_some() {
            self.worker.as_mut().ok_or_else(error)?.resolve(commit)?;
        }
        self.finish(commit)
    }
    fn finish(&mut self, commit: bool) -> Result<(), AppError> {
        if !commit {
            self.quiesce()?;
        }
        let confirmed = if commit {
            BodyPhase::Committed
        } else {
            BodyPhase::RolledBack
        };
        finish_configuration(&mut self.config, self.lease.as_ref(), confirmed, |phase| {
            if let Some(worker) = &mut self.worker {
                worker.finalize(phase)?;
            }
            Ok(())
        })?;
        self.worker.take();
        if guard::pending_update(&self.state.request.root)? {
            return Err(error());
        }
        self.state.phase = if commit {
            Phase::Committed
        } else {
            Phase::RolledBack
        };
        self.state.machine_done = true;
        self.save()?;
        self.launch_terminal()
    }
    fn launch_terminal(&mut self) -> Result<(), AppError> {
        if self.state.phase == Phase::Committed {
            if self.child.is_none() {
                self.start_candidate()?;
            }
            self.candidate_channel
                .as_mut()
                .ok_or_else(error)?
                .send(&StartupDecision::Release)?;
            self.candidate_channel.take();
            // Drop Child without killing. The visible GUI waits for this runner
            // to exit and retires the sealed pointer before enabling writes.
            self.child.take();
            Ok(())
        } else if self.state.phase == Phase::RolledBack {
            if fingerprint(&self.state.request.root.join("Rela.exe"), MAX_PROGRAM)?.sha256
                != self.state.request.runner_digest
            {
                return Err(error());
            }
            self.lease.take();
            spawn(
                &self.state.request.root.join("Rela.exe"),
                "--rela-installed-restored",
                &self.state.request.id,
            )?;
            Ok(())
        } else {
            Err(error())
        }
    }
    fn start_candidate(&mut self) -> Result<(), AppError> {
        self.candidate_guard = Some(lock_program(
            &self.state.request.root,
            self.source.stage.index(),
        )?);
        let listener = Listener::bind(&self.state.request.id, Role::Candidate)?;
        write_private(
            &self.control.join("startup.dat"),
            &StartupRequest {
                endpoint: listener.endpoint(),
                controller: self.state.runner.clone().ok_or_else(error)?,
                expected_version: self.source.manifest.version.clone(),
            },
        )?;
        self.lease.take();
        let executable = self.state.request.root.join("Rela.exe");
        self.child = Some(spawn(
            &executable,
            "--rela-installed-updated",
            &self.state.request.id,
        )?);
        let child = self.child.as_mut().ok_or_else(error)?;
        self.state.candidate = Some(ProcessFence::capture(child.id(), &executable)?);
        save(&self.control, &self.state)?;
        let mut channel =
            listener.accept(child.id(), &executable, Duration::from_secs(120), || {
                child.try_wait().is_ok_and(|s| s.is_some())
            })?;
        let ready: StartupReady = channel.receive()?;
        if ready.version != self.source.manifest.version {
            return Err(error());
        }
        self.candidate_channel = Some(channel);
        Ok(())
    }
    fn quiesce(&mut self) -> Result<(), AppError> {
        if let Some(mut channel) = self.candidate_channel.take() {
            let _ = channel.send(&StartupDecision::Abort);
        }
        if let Some(mut child) = self.child.take() {
            // Only the handle returned by this controller's own spawn may be
            // terminated. A PID from disk is waited on, never killed.
            if child.try_wait().map_err(|_| error())?.is_none() {
                child.kill().map_err(|_| error())?;
            }
            child.wait().map_err(|_| error())?;
        } else if let Some(ticket) = &self.state.candidate {
            if let Some(process) = ProcessFence::probe(ticket)? {
                process.wait(Duration::from_secs(180))?;
            }
        }
        self.candidate_guard.take();
        self.acquire_lease()
    }
}

/// Called after a matching durable machine decision (or proven unstarted PRE).
/// A hidden Ready candidate has no remaining configuration writes. Finalize is
/// deliberately last: failed user recovery must leave machine backups available.
pub(super) fn finish_configuration(
    config: &mut Option<configuration::Transaction>,
    lease: Option<&InstanceLease>,
    confirmed: BodyPhase,
    finalize: impl FnOnce(BodyPhase) -> Result<(), AppError>,
) -> Result<(), AppError> {
    if !matches!(confirmed, BodyPhase::Committed | BodyPhase::RolledBack) {
        return Err(error());
    }
    if let Some(config) = config {
        if confirmed == BodyPhase::Committed {
            config.commit()?;
        } else {
            config.rollback(lease.ok_or_else(error)?)?;
        }
        config.cleanup()?;
        config.archive()?;
    } else if confirmed == BodyPhase::Committed {
        return Err(error());
    }
    finalize(confirmed)
}
impl Drop for Runtime {
    fn drop(&mut self) {
        if let Some(mut channel) = self.candidate_channel.take() {
            let _ = channel.send(&StartupDecision::Abort);
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
