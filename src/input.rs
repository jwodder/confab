use crate::util::InterfaceError;
use async_stream::stream;
use futures::stream::Stream;
use pin_project_lite::pin_project;
use rustyline_async::{Readline, ReadlineError, ReadlineEvent};
use std::future::Future;
use std::pin::Pin;
use std::task::{ready, Context, Poll};
use std::time::Duration;
use tokio::fs::File as TokioFile;
use tokio::io::{AsyncBufReadExt, BufReader, Lines};
use tokio::time::{sleep, Sleep};

pin_project! {
    #[derive(Debug)]
    pub(crate) struct StartupScript {
        #[pin]
        lines: Lines<BufReader<TokioFile>>,
        #[pin]
        nap: Option<Sleep>,
        delay: Duration,
    }
}

impl StartupScript {
    pub(crate) fn new(reader: BufReader<TokioFile>, delay: Duration) -> StartupScript {
        StartupScript {
            lines: reader.lines(),
            nap: None,
            delay,
        }
    }
}

impl Stream for StartupScript {
    type Item = Result<String, InterfaceError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        if let Some(nap) = this.nap.as_mut().as_pin_mut() {
            ready!(nap.poll(cx));
            this.nap.set(None);
        }
        let r = ready!(this.lines.as_mut().poll_next_line(cx))
            .map_err(InterfaceError::ReadScript)
            .transpose();
        // TODO: Should the stream be forcibly fused when we're about to return
        // None?
        this.nap.set(Some(sleep(*this.delay)));
        r.into()
    }
}

#[allow(clippy::needless_pass_by_ref_mut)] // False positive
pub(crate) async fn readline_stream(
    rl: &mut Readline,
) -> impl Stream<Item = Result<String, InterfaceError>> + Send + '_ {
    stream! {
        loop {
            match rl.readline().await {
                Ok(ReadlineEvent::Line(line)) => {
                    rl.add_history_entry(line.clone());
                    yield Ok(line);
                }
                Ok(ReadlineEvent::Eof) | Err(ReadlineError::Closed) => break,
                Ok(ReadlineEvent::Interrupted) => {
                    // TODO: writeln!(self.stdout, "^C")?;
                    continue;
                }
                Err(ReadlineError::IO(e)) => yield Err(InterfaceError::ReadLine(e)),
            }
        }
    }
}
