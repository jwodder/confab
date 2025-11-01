use crate::errors::InterfaceError;
use async_stream::stream;
use futures_util::Stream;
use pin_project_lite::pin_project;
use rustyline_async::{Readline, ReadlineError, ReadlineEvent};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use std::time::Duration;
use tokio::fs::File as TokioFile;
use tokio::io::{AsyncBufReadExt, BufReader, Lines};
use tokio::time::{Sleep, sleep};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Input {
    Line(String),
    CtrlC,
}

pin_project! {
    #[derive(Debug)]
    pub(crate) struct StartupScript {
        #[pin]
        lines: Lines<BufReader<TokioFile>>,
        #[pin]
        nap: Option<Sleep>,
        next_line: Option<Input>,
        delay: Duration,
    }
}

impl StartupScript {
    pub(crate) fn new(reader: BufReader<TokioFile>, delay: Duration) -> StartupScript {
        StartupScript {
            lines: reader.lines(),
            nap: Some(sleep(delay)),
            next_line: None,
            delay,
        }
    }
}

impl Stream for StartupScript {
    type Item = Result<Input, InterfaceError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        if this.next_line.is_none() {
            match ready!(this.lines.as_mut().poll_next_line(cx)) {
                Ok(Some(line)) => {
                    *this.next_line = Some(Input::Line(line));
                    this.nap.set(Some(sleep(*this.delay)));
                }
                Ok(None) => return None.into(),
                Err(e) => return Some(Err(InterfaceError::ReadScript(e))).into(),
            }
        }
        if let Some(nap) = this.nap.as_mut().as_pin_mut() {
            ready!(nap.poll(cx));
            this.nap.set(None);
        }
        this.next_line.take().map(Ok).into()
    }
}

pub(crate) fn readline_stream(
    rl: &mut Readline,
) -> impl Stream<Item = Result<Input, InterfaceError>> + Send + '_ {
    stream! {
        loop {
            match rl.readline().await {
                Ok(ReadlineEvent::Line(line)) => {
                    rl.add_history_entry(line.clone());
                    yield Ok(Input::Line(line));
                }
                Ok(ReadlineEvent::Eof) | Err(ReadlineError::Closed) => break,
                Ok(ReadlineEvent::Interrupted) => yield Ok(Input::CtrlC),
                Err(ReadlineError::IO(e)) => yield Err(InterfaceError::ReadLine(e)),
            }
        }
    }
}
