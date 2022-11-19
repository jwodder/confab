use rustyline_async::{Readline, ReadlineError};
use std::io::Write;
use std::time::Duration;
use tokio::time::sleep;

#[tokio::main]
async fn main() -> Result<(), ReadlineError> {
    let (mut rl, mut stdout) = Readline::new("> ".into())?;
    loop {
        tokio::select! {
            _ = sleep(Duration::from_secs(3)) => {
                writeln!(stdout, "Message received!")?;
            }
            cmd = rl.readline() => match cmd {
                Ok(line) => {
                    writeln!(stdout, "{line:?}")?;
                    rl.add_history_entry(line.clone());
                    if line == "quit" {
                        break;
                    }
                }
                Err(ReadlineError::Eof) => {
                    writeln!(stdout, "<EOF>")?;
                    break;
                }
                Err(ReadlineError::Interrupted) => {
                    writeln!(stdout, "<Ctrl-C>")?;
                    break;
                }
                Err(e) => {
                    writeln!(stdout, "Error: {e:?}")?;
                    break;
                }
            }
        }
    }
    Ok(())
}
