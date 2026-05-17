//! Process supervisor wrapper using watchexec-supervisor.

use std::{collections::HashMap, path::PathBuf, sync::Arc};

use tokio::process::Command as TokioCommand;
use tokio::task::JoinHandle;
use watchexec_supervisor::{
    Signal,
    command::{Command, Program, SpawnOptions},
    job::{Job as SupervisorJob, start_job},
};

/// A wrapper around a watchexec-supervisor [`Job`] that takes binary name, args, and env vars.
///
/// The environment variables are applied via a custom spawn function since the
/// underlying `Command` does not expose an env field directly.
#[derive(Debug)]
pub struct Supervisor {
    /// The command definition (binary, args).
    cmd: Arc<Command>,
    /// Environment variables to apply on spawn.
    envs: HashMap<String, String>,
    /// The underlying watchexec-supervisor Job (created on first `start()`).
    job: Option<SupervisorJob>,
    /// Handle to the job task, retained for lifecycle management.
    task: Option<JoinHandle<()>>,
}

impl Supervisor {
    /// Create a new [`Supervisor`] for the given binary with args and env vars.
    /// Does not spawn the process until [`Self::start()`] is called.
    ///
    /// # Arguments
    /// * `binary` - Path or name of the program to run.
    /// * `args` - Command-line arguments to pass to the program.
    /// * `envs` - Environment variables to set for the program.
    pub fn new(
        binary: impl Into<PathBuf>,
        args: Vec<String>,
        envs: HashMap<String, String>,
    ) -> Self {
        let cmd = Arc::new(Command {
            program: Program::Exec {
                prog: binary.into(),
                args,
            },
            options: SpawnOptions::default(),
        });

        Self {
            cmd,
            envs,
            job: None,
            task: None,
        }
    }

    /// Start the supervised process. Spawns the supervisor task on first call.
    pub fn start(&mut self) {
        if self.job.is_none() {
            let (job, task) = start_job(self.cmd.clone());

            // Apply environment variables via custom spawn function.
            if !self.envs.is_empty() {
                let envs_clone = self.envs.clone();
                job.set_spawn_fn(move |cmd: &mut TokioCommand| {
                    cmd.env_clear();
                    for (key, val) in &envs_clone {
                        cmd.env(key, val);
                    }
                    cmd.spawn()
                });
            }

            self.job = Some(job);
            self.task = Some(task);
        }

        if let Some(ref job) = self.job {
            job.start();
        }
    }

    /// Stop the command if it is running and wait for completion.
    ///
    /// If you don't want to wait for completion, use [`Self::signal`] with
    /// [`Signal::ForceStop`] instead.
    pub fn stop(&self) {
        if let Some(ref job) = self.job {
            job.stop();
        }
    }

    /// Send a signal to the running command.
    ///
    /// This sends a signal to the current program, if there is one. If there isn't,
    /// this is a no-op.
    ///
    /// On Windows, only [`Signal::ForceStop`] has any effect.
    pub fn signal(&self, sig: Signal) {
        if let Some(ref job) = self.job {
            job.signal(sig);
        }
    }

    pub fn is_running(&self) -> bool {
        self.job
            .as_ref()
            .is_some_and(watchexec_supervisor::job::Job::is_running)
    }

    pub fn is_dead(&self) -> bool {
        self.job
            .as_ref()
            .is_none_or(watchexec_supervisor::job::Job::is_dead)
    }

    pub async fn wait(&self) {
        if let Some(ref job) = self.job {
            job.to_wait().await;
        }
    }

    /// Gracefully stop the command with a signal and grace period.
    ///
    /// The command will be sent `signal` and then given `grace` time before being
    /// forcefully terminated. On Windows this is equivalent to [`Self::stop`].
    pub fn stop_with_signal(&self, signal: Signal, grace: std::time::Duration) {
        if let Some(ref job) = self.job {
            job.stop_with_signal(signal, grace);
        }
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        if let Some(ref job) = self.job {
            job.stop();
        }
        if let Some(ref task) = self.task {
            task.abort();
        }
    }
}
