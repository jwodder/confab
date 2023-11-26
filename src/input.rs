use crate::util::InterfaceError;
use rustyline_async::{Readline, ReadlineError, ReadlineEvent};
use std::time::Duration;
use tokio::fs::File as TokioFile;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc::Sender;
use tokio::time::sleep;

#[derive(Debug)]
pub(crate) struct StartupScript {
    pub(crate) fp: BufReader<TokioFile>,
    pub(crate) delay: Duration,
}

pub(crate) struct InputSource {
    pub(crate) startup_script: Option<StartupScript>,
    pub(crate) rl: Readline,
}

impl InputSource {
    pub(crate) async fn run(mut self, sender: Sender<Result<String, InterfaceError>>) {
        if let Some(script) = self.startup_script.take() {
            let mut first = true;
            let mut lines = script.fp.lines();
            while let Some(nl) = lines.next_line().await.transpose() {
                let r = match nl {
                    Ok(line) => {
                        if !std::mem::replace(&mut first, false) {
                            sleep(script.delay).await;
                        }
                        Ok(line)
                    }
                    Err(e) => Err(InterfaceError::ReadScript(e)),
                };
                if sender.send(r).await.is_err() {
                    return;
                }
            }
        }
        loop {
            let r = match self.rl.readline().await {
                Ok(ReadlineEvent::Line(line)) => {
                    self.rl.add_history_entry(line.clone());
                    Ok(line)
                }
                Ok(ReadlineEvent::Eof) | Err(ReadlineError::Closed) => break,
                Ok(ReadlineEvent::Interrupted) => {
                    // TODO: writeln!(self.stdout, "^C")?;
                    continue;
                }
                Err(ReadlineError::IO(e)) => Err(InterfaceError::ReadLine(e)),
            };
            if sender.send(r).await.is_err() {
                return;
            }
        }
    }
}

impl Drop for InputSource {
    fn drop(&mut self) {
        let _ = self.rl.flush();
    }
}
