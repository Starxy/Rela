//! Original-user view of an authenticated NSIS hook/recovery participant.
use super::super::installed::Phase as BodyPhase;
use super::super::nsis_worker::{Command as Action, Hello, Response};
use super::*;

pub(super) struct Client {
    channel: Option<Channel>,
    process: ProcessFence,
    installer: Option<ProcessFence>,
    hook_bytes: Option<File>,
    installer_bytes: Option<VerifiedPackage>,
    elevated: Option<platform::ElevatedProcess>,
}
impl Client {
    pub fn hook(
        listener: &Listener,
        control: &Path,
        state: &mut Control,
        source: &VerifiedSource,
        exited: impl FnMut() -> bool,
    ) -> Result<Self, AppError> {
        let mut observed = None;
        let (channel, ticket) =
            listener.accept_verified(Duration::from_secs(180), exited, |ticket| {
                let (process, image) = ProcessFence::inspect(ticket)?;
                let bytes = lock_file(&image, &source.stage.index().files["Rela.exe"])?;
                observed = Some((process, bytes));
                Ok(())
            })?;
        let (process, bytes) = observed.ok_or_else(error)?;
        let mut client = Self {
            channel: Some(channel),
            process,
            installer: None,
            hook_bytes: Some(bytes),
            installer_bytes: None,
            elevated: None,
        };
        let hello: Hello = client.channel()?.receive()?;
        let nsis = hello.installer.ok_or_else(error)?;
        let image = hello.installer_image.ok_or_else(error)?;
        let installer = ProcessFence::open(&nsis, &image)?;
        client.process.is_child_of(ticket.pid, &nsis)?;
        let installer_bytes = VerifiedPackage::open_versioned(
            &image,
            &source.manifest.installer,
            &super::super::package_public_key()?,
            &source.manifest.version,
        )?;
        if let Some(previous) = &state.installer {
            if previous != &nsis || state.installer_image.as_ref() != Some(&image) {
                return Err(error());
            }
        }
        if installer.has_exited()? {
            return Err(error());
        }
        state.hook = Some(ticket);
        state.installer = Some(nsis);
        state.installer_image = Some(image);
        // Durable before PRE Prepare: earlier phases prove NSIS was never
        // authorized to alter program files, even if its process was launched.
        state.phase = Phase::Active;
        save(control, state)?;
        client.installer = Some(installer);
        client.installer_bytes = Some(installer_bytes);
        Ok(client)
    }
    pub fn pre(&mut self) -> Result<(), AppError> {
        self.channel()?.send(&Action::Prepare)?;
        if !matches!(self.channel()?.receive::<Response>()?, Response::BackedUp) {
            return Err(error());
        }
        self.channel()?.send(&Action::AllowInstall)?;
        self.channel.take();
        // NSIS removes the extracted hook at shutdown, so release its read lock
        // before waiting. Exact process handles remain valid until exit.
        self.hook_bytes.take();
        self.installer_bytes.take();
        self.process.wait(Duration::from_secs(30))
    }
    pub fn post(&mut self) -> Result<BodyPhase, AppError> {
        self.channel()?.send(&Action::Prepare)?;
        match self.channel()?.receive::<Response>()? {
            Response::Ready {
                phase: BodyPhase::Installed,
            } => Ok(BodyPhase::Installed),
            _ => Err(error()),
        }
    }
    pub fn recover(
        control: &Path,
        state: &mut Control,
        source: &VerifiedSource,
    ) -> Result<(Self, Option<BodyPhase>), AppError> {
        let listener = Listener::bind(&state.request.id, Role::InstallerRecovery)?;
        state.request.recovery = Some(listener.endpoint());
        save(control, state)?;
        nsis_worker::write_request(control, &state.request)?;
        let executable = control.join("candidate/Rela.exe");
        let elevated = platform::launch_installed_recovery_helper(
            &executable,
            &control.join(nsis_worker::REQUEST_FILE),
        )?;
        let ticket = ProcessFence::capture(elevated.id(), &executable)?;
        let process = ProcessFence::open(&ticket, &executable)?;
        state.hook = Some(ticket);
        save(control, state)?;
        let channel =
            listener.accept(elevated.id(), &executable, Duration::from_secs(180), || {
                elevated.has_exited()
            })?;
        let bytes = lock_file(&executable, &source.stage.index().files["Rela.exe"])?;
        let mut client = Self {
            channel: Some(channel),
            process,
            installer: None,
            hook_bytes: Some(bytes),
            installer_bytes: None,
            elevated: Some(elevated),
        };
        let hello: Hello = client.channel()?.receive()?;
        if hello.installer.is_some() || hello.installer_image.is_some() {
            return Err(error());
        }
        client.channel()?.send(&Action::Prepare)?;
        let phase = match client.channel()?.receive::<Response>()? {
            Response::Unstarted => None,
            Response::Ready { phase } => Some(phase),
            _ => return Err(error()),
        };
        Ok((client, phase))
    }
    fn channel(&mut self) -> Result<&mut Channel, AppError> {
        self.channel.as_mut().ok_or_else(error)
    }
    pub fn resolve(&mut self, commit: bool) -> Result<BodyPhase, AppError> {
        self.channel()?.send(&if commit {
            Action::Commit
        } else {
            Action::Rollback
        })?;
        let expected = if commit {
            BodyPhase::Committed
        } else {
            BodyPhase::RolledBack
        };
        match self.channel()?.receive::<Response>()? {
            Response::Resolved { phase } if phase == expected => Ok(phase),
            _ => Err(error()),
        }
    }
    pub fn finalize(&mut self, expected: BodyPhase) -> Result<(), AppError> {
        self.channel()?.send(&Action::Finalize)?;
        match self.channel()?.receive::<Response>()? {
            Response::Finished { phase } if phase == expected => {}
            _ => return Err(error()),
        }
        self.close_and_wait()
    }
    pub fn close_and_wait(&mut self) -> Result<(), AppError> {
        self.channel.take();
        self.hook_bytes.take();
        self.installer_bytes.take();
        self.process.wait(Duration::from_secs(60))?;
        if let Some(process) = &self.installer {
            process.wait(Duration::from_secs(60))?;
        }
        if let Some(process) = &self.elevated {
            process.wait(Duration::from_secs(60))?;
        }
        Ok(())
    }
}
