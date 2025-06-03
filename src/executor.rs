use crate::pipe_io;
use crate::pipe_io::{PipeReader, Record};
use nix::sys::stat::Mode;
use nix::unistd::mkfifo;
use std::ffi::OsStr;
use std::fs::{remove_file, OpenOptions};
use std::io;
use std::path::Path;
use std::process::{Child, Command, ExitStatus};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to execute command")]
    CmdFailed(ExitStatus),
    #[error("IO error")]
    CmdError(#[from] io::Error),
    #[error("pipe error")]
    PipeError(#[from] pipe_io::Error),
}

pub fn exec_cmd<S, P>(
    program: S,
    args: impl IntoIterator<Item = S>,
    cwd: P,
    lib_path: &str,
) -> ExecResult
where
    S: AsRef<OsStr>,
    P: AsRef<Path>,
{
    let pid = std::process::id();
    let pipe_file_path = format!("/tmp/{}.pipe", pid);

    mkfifo(pipe_file_path.as_str(), Mode::S_IRUSR | Mode::S_IWUSR).unwrap();

    let envs = [
        ("PIPE_FILEPATH", pipe_file_path.as_str()),
        ("DYLD_INSERT_LIBRARIES", lib_path),
    ];

    let mut cmd = Command::new(program);
    cmd.args(args);
    cmd.envs(envs);
    cmd.current_dir(cwd);

    let child = cmd.spawn().unwrap();

    ExecResult::new(child, pipe_file_path)
}

pub struct ExecResult {
    child: Child,
    pipe_filepath: String,
    reader: Option<PipeReader>,
}

impl ExecResult {
    pub fn new(child: Child, pipe_filepath: String) -> Self {
        Self {
            child,
            pipe_filepath,
            reader: None,
        }
    }

    pub fn next(&mut self) -> Option<Result<Record, Error>> {
        loop {
            match &mut self.reader {
                None => {
                    let pipe_file = OpenOptions::new()
                        .read(true)
                        .open(&self.pipe_filepath)
                        .unwrap();

                    let reader = PipeReader::new(pipe_file);
                    self.reader = Some(reader);
                }
                Some(reader) => {
                    return match self.child.try_wait() {
                        Ok(result) => {
                            if let Some(exit) = result {
                                if !exit.success() {
                                    return Some(Err(Error::CmdFailed(exit)));
                                }
                            }

                            Some(reader.read_record()?.map_err(Error::from))
                        }
                        Err(e) => Some(Err(e.into())),
                    }
                }
            }
        }
    }
}

impl Drop for ExecResult {
    fn drop(&mut self) {
        _ = remove_file(&self.pipe_filepath);
    }
}
