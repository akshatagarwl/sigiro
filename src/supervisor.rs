//! Process supervisor wrapper using watchexec-supervisor.

use std::{collections::HashMap, path::PathBuf, sync::Arc};

use tokio::process::Command as TokioCommand;
use tokio::task::JoinHandle;
use watchexec_supervisor::{
    command::{Command, Program, SpawnOptions},
    job::{start_job, Job as SupervisorJob},
    Signal,
};

/// A wrapper around a watchexec-supervisor [`Job`] that takes binary name, args, and env vars.
///
/// The environment variables are applied via a custom spawn function since the
/// underlying `Command` does not expose an env field directly.
#[derive(Debug)]
pub struct Supervisor {
    /// The underlying watchexec-supervisor Job.
    job: SupervisorJob,
    /// Handle to the job task, retained for lifecycle management.
    task: JoinHandle<()>,
}

impl Supervisor {
    /// Create a new [`Supervisor`] for the given binary with args and env vars.
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

        let (job, task) = start_job(cmd.clone());

        // Apply environment variables via custom spawn function.
        // set_spawn_fn lets us mutate the tokio::Command before it is spawned.
        if !envs.is_empty() {
            let envs_clone = envs;
            job.set_spawn_fn(move |cmd: &mut TokioCommand| {
                // Clear inherited envs and set only the ones provided.
                cmd.env_clear();
                for (key, val) in &envs_clone {
                    cmd.env(key, val);
                }
                cmd.spawn()
            });
        }

        Self { job, task }
    }

    /// Start the command if it is not running.
    pub fn start(&self) {
        self.job.start();
    }

    /// Stop the command if it is running and wait for completion.
    ///
    /// If you don't want to wait for completion, use [`Self::signal`] with
    /// [`Signal::ForceStop`] instead.
    pub fn stop(&self) {
        self.job.stop();
    }

    /// Send a signal to the running command.
    ///
    /// This sends a signal to the current program, if there is one. If there isn't,
    /// this is a no-op.
    ///
    /// On Windows, only [`Signal::ForceStop`] has any effect.
    pub fn signal(&self, sig: Signal) {
        self.job.signal(sig);
    }

    /// Check if the supervised command is currently running.
    pub fn is_running(&self) -> bool {
        self.job.is_running()
    }

    /// Check if the job task has stopped entirely.
    pub fn is_dead(&self) -> bool {
        self.job.is_dead()
    }

    /// Wait for the supervised command to end.
    pub async fn wait(&self) {
        self.job.to_wait().await;
    }

    /// Gracefully stop the command with a signal and grace period.
    ///
    /// The command will be sent `signal` and then given `grace` time before being
    /// forcefully terminated. On Windows this is equivalent to [`Self::stop`].
    pub fn stop_with_signal(&self, signal: Signal, grace: std::time::Duration) {
        self.job.stop_with_signal(signal, grace);
    }
}
